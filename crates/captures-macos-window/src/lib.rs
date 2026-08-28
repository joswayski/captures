#![cfg(target_os = "macos")]

use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, define_class,
    ffi::{OBJC_ASSOCIATION_RETAIN_NONATOMIC, objc_getAssociatedObject, objc_setAssociatedObject},
    msg_send,
    rc::Retained,
    runtime::AnyObject,
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSBezierPath, NSBezierPathElement, NSColor,
    NSCursor, NSEvent, NSEventMask, NSEventType, NSPasteboard, NSRunningApplication, NSScreen,
    NSSound, NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSViewLayerContentsPlacement, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{
    NSNumber, NSObject, NSObjectProtocol, NSPoint, NSProcessInfo, NSRect, NSSize, NSString,
};
use tauri::WebviewWindow;
use tauri_nspanel::WebviewWindowExt;

mod interactive_hud_panel {
    use tauri::Manager;
    use tauri_nspanel::tauri_panel;

    tauri_panel! {
        panel!(InteractiveHudPanel {
            config: {
                can_become_key_window: true,
                can_become_main_window: false,
                is_floating_panel: true,
                becomes_key_only_if_needed: true,
                hides_on_deactivate: false,
                works_when_modal: true,
            }
        })
    }
}

mod thumbnail_panel {
    use tauri::Manager;
    use tauri_nspanel::tauri_panel;

    tauri_panel! {
        panel!(ThumbnailPanel {
            config: {
                // A nonactivating panel may become key without activating the
                // Captures application. WebKit/AppKit need that key status to
                // display hover cursors while another app remains frontmost.
                can_become_key_window: true,
                can_become_main_window: false,
                is_floating_panel: true,
                becomes_key_only_if_needed: true,
                hides_on_deactivate: false,
                works_when_modal: true,
            }
        })
    }
}

use interactive_hud_panel::InteractiveHudPanel;
use thumbnail_panel::ThumbnailPanel;

const LEGACY_WINDOW_CORNER_RADIUS_POINTS: f64 = 10.0;
const LIQUID_GLASS_WINDOW_CORNER_RADIUS_POINTS: f64 = 25.0;
const LIQUID_GLASS_MACOS_MAJOR_VERSION: isize = 26;
/// Imperceptible alpha that still keeps WKWebView compositing. Fully transparent
/// windows (`0.0`) can suspend painting and flash black on the first opaque frame.
const WINDOW_REVEAL_PRIME_ALPHA: f64 = 0.01;
const _: () = {
    assert!(WINDOW_REVEAL_PRIME_ALPHA > 0.0);
    assert!(WINDOW_REVEAL_PRIME_ALPHA < 0.05);
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum CursorMode {
    Arrow = 0,
    Crosshair = 1,
    PointingHand = 2,
    OpenHand = 3,
    WebView = 4,
}

impl CursorMode {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Crosshair,
            2 => Self::PointingHand,
            3 => Self::OpenHand,
            4 => Self::WebView,
            _ => Self::Arrow,
        }
    }
}

/// Cursor shown over the always-on-top capture previews.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThumbnailCursorKind {
    Default,
    Pointer,
    Grab,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorSurface {
    CaptureOverlay,
    InactiveHud,
    Thumbnail,
}

struct CursorTrackingIvars {
    mode: Cell<CursorMode>,
    surface: CursorSurface,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - AppKit invokes these tracking callbacks on its main event thread.
    // - `CursorTrackingOwner` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "CESCursorTrackingOwner"]
    #[ivars = CursorTrackingIvars]
    struct CursorTrackingOwner;

    impl CursorTrackingOwner {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.rearm_thumbnail_key_window_if_needed();
            self.activate_window_if_needed(event);
            self.apply_cursor(Some(event));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.activate_window_if_needed(event);
            self.apply_cursor(Some(event));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
            self.rearm_thumbnail_key_window_if_needed();
            self.resign_window_if_needed(event);
            if should_reset_cursor_on_exit(
                self.ivars().surface,
                capture_overlay_owns_cursor(),
            ) {
                NSCursor::arrowCursor().set();
            }
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, event: &NSEvent) {
            self.activate_window_if_needed(event);
            self.apply_cursor(Some(event));
        }
    }
);

impl CursorTrackingOwner {
    fn new(mode: CursorMode, surface: CursorSurface) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CursorTrackingIvars {
            mode: Cell::new(mode),
            surface,
        });
        // SAFETY: `NSObject`'s `init` method has this signature.
        let owner = unsafe { msg_send![super(this), init] };
        if surface == CursorSurface::Thumbnail {
            publish_thumbnail_cursor_mode(mode);
        }
        owner
    }

    fn set_mode(&self, mode: CursorMode) {
        self.ivars().mode.set(mode);
        if self.ivars().surface == CursorSurface::Thumbnail {
            publish_thumbnail_cursor_mode(mode);
        }
    }

    fn rearm_thumbnail_key_window_if_needed(&self) {
        if self.ivars().surface == CursorSurface::Thumbnail {
            THUMBNAIL_KEY_WINDOW_ALLOWED.store(true, Ordering::Release);
        }
    }

    fn activate_window_if_needed(&self, event: &NSEvent) {
        if !cursor_surface_can_take_key_window(self.ivars().surface)
            || !cursor_surface_can_apply(self.ivars().surface, capture_overlay_owns_cursor())
        {
            return;
        }
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        if let Some(window) = event.window(main_thread)
            && !window.isKeyWindow()
        {
            window.makeKeyWindow();
            // Becoming key lets WebKit re-enable cursor rectangles. Keep them
            // off while AppKit owns an interactive grab/pointer cursor so the
            // arrow and CSS pointer cannot alternate every mouse event.
            if cursor_mode_is_interactive(self.ivars().mode.get()) {
                set_cursor_rects_enabled(&window, false);
            }
        }
    }

    fn resign_window_if_needed(&self, event: &NSEvent) {
        if !cursor_surface_uses_key_window(self.ivars().surface)
            || !cursor_surface_can_apply(self.ivars().surface, capture_overlay_owns_cursor())
        {
            return;
        }
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        if let Some(window) = event.window(main_thread)
            && window.isKeyWindow()
        {
            window.resignKeyWindow();
        }
    }

    fn apply_cursor(&self, event: Option<&NSEvent>) {
        if !cursor_surface_can_apply(self.ivars().surface, capture_overlay_owns_cursor()) {
            return;
        }
        let mode = self.ivars().mode.get();
        // WebKit may re-enable cursor rectangles after focus or layer updates.
        // Re-disable on every interactive apply so CSS cannot flash the default
        // arrow between AppKit open-hand / pointing-hand updates.
        if cursor_mode_is_interactive(mode)
            && let Some(event) = event
            && let Some(main_thread) = MainThreadMarker::new()
            && let Some(window) = event.window(main_thread)
        {
            set_cursor_rects_enabled(&window, false);
        }
        // Thumbnail: JS poll owns the cursor kind (grab on the image, pointer
        // on buttons). Forcing the arrow on every mouseMoved while mode is
        // still Arrow races the first open-hand update and also suppresses CSS
        // `grab` over the drag source before that poll. Capture overlay still
        // needs an explicit arrow when requested.
        if matches!(mode, CursorMode::Arrow)
            && self.ivars().surface == CursorSurface::Thumbnail
            && event.is_some()
        {
            return;
        }
        apply_cursor_mode(mode);
    }
}

fn cursor_mode_is_interactive(mode: CursorMode) -> bool {
    matches!(
        mode,
        CursorMode::PointingHand | CursorMode::OpenHand | CursorMode::Crosshair
    )
}

/// Last AppKit cursor mode published for the always-on-top thumbnail stack.
///
/// Click handling lives in WebKit, which resets `NSCursor` to the arrow on
/// primary-button down/up. JS reassert arrives one IPC hop too late and flashes
/// the default arrow. A process-local event monitor re-applies this mode on the
/// same run loop as the mouse event (and once more on the next turn).
static THUMBNAIL_CURSOR_MODE: AtomicU8 = AtomicU8::new(CursorMode::Arrow as u8);
// Mouse-up releases thumbnail key status so the frontmost app cannot remain
// visually inactive under a stationary pointer. Leaving/re-entering the card or
// moving to a different cursor region rearms key-on-hover.
static THUMBNAIL_KEY_WINDOW_ALLOWED: AtomicBool = AtomicBool::new(true);
static THUMBNAIL_CLICK_CURSOR_MONITOR: Mutex<Option<MainThreadMonitor>> = Mutex::new(None);

/// Retains an AppKit event monitor installed only on the main thread.
///
/// `Retained<AnyObject>` is neither `Send` nor `Sync`; the monitor is only
/// created/held from AppKit's main thread and never used from other threads.
/// The retained object is intentionally unread after install — dropping it
/// would unregister the monitor.
#[allow(dead_code)]
struct MainThreadMonitor(Retained<AnyObject>);

// SAFETY: The wrapped monitor is only installed and retained on AppKit's main
// thread. We never call into it from other threads; the Mutex only guards the
// Option so install races are serialized.
unsafe impl Send for MainThreadMonitor {}
// SAFETY: Same as `Send` — access is main-thread-only via AppKit callbacks.
unsafe impl Sync for MainThreadMonitor {}

fn publish_thumbnail_cursor_mode(mode: CursorMode) {
    THUMBNAIL_CURSOR_MODE.store(mode as u8, Ordering::Release);
    if cursor_mode_is_interactive(mode) {
        ensure_thumbnail_click_cursor_monitor();
    }
}

fn thumbnail_cursor_mode() -> CursorMode {
    CursorMode::from_u8(THUMBNAIL_CURSOR_MODE.load(Ordering::Acquire))
}

fn apply_cursor_mode(mode: CursorMode) {
    match mode {
        CursorMode::Arrow => NSCursor::arrowCursor().set(),
        CursorMode::Crosshair => NSCursor::crosshairCursor().set(),
        CursorMode::PointingHand => NSCursor::pointingHandCursor().set(),
        CursorMode::OpenHand => NSCursor::openHandCursor().set(),
        CursorMode::WebView => {}
    }
}

/// Re-apply the interactive thumbnail cursor after a click/key-window handoff.
///
/// Returns whether an interactive cursor was reasserted (for tests).
fn reassert_thumbnail_cursor_after_click() -> bool {
    if capture_overlay_owns_cursor() {
        return false;
    }
    let mode = thumbnail_cursor_mode();
    if !cursor_mode_is_interactive(mode) {
        return false;
    }
    apply_cursor_mode(mode);
    true
}

fn should_release_thumbnail_key_after_event(
    surface: Option<CursorSurface>,
    event_type: NSEventType,
) -> bool {
    surface == Some(CursorSurface::Thumbnail) && event_type == NSEventType::LeftMouseUp
}

fn thumbnail_key_window_for_mouse_up(event: &NSEvent) -> Option<usize> {
    if event.r#type() != NSEventType::LeftMouseUp {
        return None;
    }
    let main_thread = MainThreadMarker::new()?;
    let window = event.window(main_thread)?;
    if !should_release_thumbnail_key_after_event(cursor_surface_for_window(&window), event.r#type())
        || !window.isKeyWindow()
    {
        return None;
    }
    // Transfer this retain to the next main-queue turn. The integer is only a
    // transport container; it is reconstructed and released on that same
    // AppKit thread after WebKit finishes dispatching the click.
    Some(Retained::into_raw(window) as usize)
}

fn release_thumbnail_key_window(window_address: usize) {
    // SAFETY: `thumbnail_key_window_for_mouse_up` produced this address with
    // `Retained::into_raw`, and this function is called exactly once on the
    // next main-queue turn. Reconstructing the retain keeps the window alive
    // across the handoff and releases it when this scope ends.
    let Some(window) = (unsafe { Retained::from_raw(window_address as *mut NSWindow) }) else {
        return;
    };
    if cursor_surface_for_window(&window) == Some(CursorSurface::Thumbnail) && window.isKeyWindow()
    {
        window.resignKeyWindow();
    }
}

fn ensure_thumbnail_click_cursor_monitor() {
    let Ok(mut guard) = THUMBNAIL_CLICK_CURSOR_MONITOR.lock() else {
        return;
    };
    if guard.is_some() {
        return;
    }
    // SAFETY: The block only reads process-local atomics and sets NSCursor on
    // the main AppKit thread (local monitors run there). Returning the event
    // pointer unchanged leaves delivery intact.
    let block = RcBlock::new(|event: ptr::NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit supplies a live NSEvent for the duration of the local
        // monitor callback.
        let event_ref = unsafe { event.as_ref() };
        let thumbnail_window = thumbnail_key_window_for_mouse_up(event_ref);
        if thumbnail_window.is_some() {
            THUMBNAIL_KEY_WINDOW_ALLOWED.store(false, Ordering::Release);
        }
        let reasserted = reassert_thumbnail_cursor_after_click();
        if reasserted || thumbnail_window.is_some() {
            // WebKit installs the arrow while handling the click. Reassert again
            // on the next main-queue turn, then release thumbnail key status so
            // a Copy/Save/Delete click cannot leave the app underneath inactive.
            DispatchQueue::main().exec_async(move || {
                let _ = reassert_thumbnail_cursor_after_click();
                if let Some(window_address) = thumbnail_window {
                    release_thumbnail_key_window(window_address);
                }
            });
        }
        event.as_ptr()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::LeftMouseUp,
            &block,
        )
    };
    *guard = monitor.map(MainThreadMonitor);
}

// The address of this byte is used as the Objective-C association key.
static CURSOR_TRACKER_ASSOCIATION_KEY: u8 = 0;
// Associates the same tracker with its NSWindow so the click monitor can tell
// a nonactivating thumbnail panel from recording HUD and document windows.
static CURSOR_TRACKER_WINDOW_ASSOCIATION_KEY: u8 = 0;
// NSCursor is application-wide, so a hidden preview must not replace the
// cursor selected by the active capture overlay.
static CAPTURE_OVERLAY_OWNS_CURSOR: AtomicBool = AtomicBool::new(false);
// When a transient capture surface activates Captures (region/window overlay,
// recording selector, countdown), sibling document windows such as the
// screenshot editor are ordered front with the app. Remember the user's
// previous frontmost app so we can hand focus back after the surface dismisses.
static FRONTMOST_APP_BEFORE_CAPTURE: Mutex<Option<Retained<NSRunningApplication>>> =
    Mutex::new(None);
// Titled document windows ordered out for the duration of a capture UI session.
// Kept separate from frontmost-app restore so intermediate restores (overlay →
// countdown) do not put editors back on screen for a frame.
//
// Main-thread only: `NSWindow` is not `Send`, and every conceal/reveal path
// already requires the AppKit main thread.
thread_local! {
    static CONCEALED_DOCUMENT_WINDOWS: RefCell<Vec<Retained<NSWindow>>> =
        const { RefCell::new(Vec::new()) };
}

/// Returns whether a standard shortcut modifier is still physically held.
///
/// A registered macOS hotkey reports its primary key release before users
/// necessarily release its modifiers. Starting region capture during that gap
/// lets AppKit replace the crosshair with an arrow when the modifiers come up.
pub fn capture_shortcut_modifiers_pressed() -> bool {
    shortcut_modifiers_pressed(NSEvent::modifierFlags_class())
}

/// Returns the system pasteboard revision without reading its contents.
///
/// AppKit increments this value whenever any application replaces the
/// pasteboard, allowing Captures to notice that its last copied capture is no
/// longer current without inspecting the user's clipboard data.
pub fn clipboard_change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

/// Plays a short, low-volume system sound after a capture is confirmed.
pub fn play_capture_sound() -> Result<(), &'static str> {
    let sound = NSSound::soundNamed(&NSString::from_str("Tink"))
        .ok_or("macOS capture sound is unavailable")?;
    sound.setVolume(0.18);
    sound
        .play()
        .then_some(())
        .ok_or("macOS capture sound could not be played")
}

fn shortcut_modifiers_pressed(flags: objc2_app_kit::NSEventModifierFlags) -> bool {
    use objc2_app_kit::NSEventModifierFlags;

    flags.intersects(
        NSEventModifierFlags::Shift
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Command,
    )
}

/// Registers the panel manager used by the capture thumbnail window.
pub fn init_panel_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_nspanel::init()
}

/// Configures an interactive overlay as an inactive-app HUD with the native
/// cursor fallback used by capture previews and notices.
///
/// Tauri enables mouse-move events on its `NSWindow`, but WebKit's own tracking
/// can still become inactive when another application is frontmost. An
/// `ActiveAlways` tracking area keeps hover and pointer events alive.
pub fn configure_inactive_hover(window: &WebviewWindow) -> Result<(), &'static str> {
    configure_inactive_hover_with_cursor::<InteractiveHudPanel>(
        window,
        CursorMode::Arrow,
        CursorSurface::InactiveHud,
    )
}

/// Configures an inactive HUD whose CSS remains responsible for its cursor.
///
/// Making the non-activating panel key on hover lets WebKit refresh `:hover`
/// and cursor rectangles without bringing the Captures application forward.
pub fn configure_webview_inactive_hover(window: &WebviewWindow) -> Result<(), &'static str> {
    configure_inactive_hover_with_cursor::<InteractiveHudPanel>(
        window,
        CursorMode::WebView,
        CursorSurface::InactiveHud,
    )
}

/// Configures the mini-preview stack as a nonactivating mouse-interactive panel.
/// It becomes key only while the pointer is over live preview chrome so AppKit
/// can display its cursor, then releases key status after click delivery and on
/// exit so the application underneath stays active.
pub fn configure_thumbnail_inactive_hover(window: &WebviewWindow) -> Result<(), &'static str> {
    configure_inactive_hover_with_cursor::<ThumbnailPanel>(
        window,
        CursorMode::Arrow,
        CursorSurface::Thumbnail,
    )
}

fn configure_inactive_hover_with_cursor<P>(
    window: &WebviewWindow,
    initial_cursor: CursorMode,
    surface: CursorSurface,
) -> Result<(), &'static str>
where
    P: tauri_nspanel::FromWindow<tauri::Wry> + 'static,
{
    let _main_thread =
        MainThreadMarker::new().ok_or("inactive HUD setup must run on the main thread")?;
    let panel = window
        .to_panel::<P>()
        .map_err(|_| "failed to convert the inactive HUD to an NSPanel")?;
    panel.set_style_mask(panel.as_panel().styleMask() | NSWindowStyleMask::NonactivatingPanel);
    panel.set_level(NSStatusWindowLevel as i64);
    panel.set_floating_panel(true);
    panel.set_becomes_key_only_if_needed(true);
    panel.set_hides_on_deactivate(false);
    panel.set_works_when_modal(true);
    panel.set_accepts_mouse_moved_events(true);

    let native_window = native_window(window)?;
    native_window.setLevel(NSStatusWindowLevel);
    native_window.setAcceptsMouseMovedEvents(true);
    native_window.setAllowsToolTipsWhenApplicationIsInactive(true);

    window
        .as_ref()
        .with_webview(move |platform_webview| {
            let pointer = platform_webview.inner();
            // SAFETY: Tauri supplies the live WKWebView to this callback. A
            // WKWebView inherits from NSView and remains alive for the entire
            // callback.
            let Some(webview) = (unsafe { pointer.cast::<NSView>().as_ref() }) else {
                return;
            };
            // The preview stack and its window are both anchored to the
            // bottom of the screen. WKWebView can briefly reuse its cached
            // surface while AppKit shrinks the window after a card exits. If
            // that cache follows the moving top edge, the surviving card
            // travels below the screen before WebKit's new frame replaces it.
            // Keep the cached surface on the stable bottom edge as well.
            anchor_layer_contents_to_bottom(webview);
            let options = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect;
            // SAFETY: `webview` is the live WKWebView supplied by Tauri, and
            // AppKit retains the tracking area after attaching it to the view.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    NSRect::ZERO,
                    options,
                    Some(webview),
                    None,
                )
            };
            webview.addTrackingArea(&area);
            install_cursor_tracker(webview, initial_cursor, surface);
        })
        .map_err(|_| "macOS webview handle is unavailable")
}

fn anchor_layer_contents_to_bottom(view: &NSView) {
    view.setLayerContentsPlacement(NSViewLayerContentsPlacement::Bottom);
}

/// Shows the preview without making Captures the active application.
pub fn show_without_activating(window: &WebviewWindow) -> Result<(), &'static str> {
    let _main_thread =
        MainThreadMarker::new().ok_or("window reveal must run on the main thread")?;
    let native_window = native_window(window)?;
    native_window.setLevel(NSStatusWindowLevel);
    native_window.orderFrontRegardless();
    Ok(())
}

/// Returns the standard visible window-corner radius for the current macOS
/// design generation.
///
/// macOS 26 enlarged standard window corners from 10 to 25 points. Capture
/// selection and output masking use this value to follow the system window
/// edge instead of applying one radius to every macOS release.
pub fn standard_window_corner_radius_points() -> f64 {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    window_corner_radius_for_major_version(version.majorVersion)
}

fn window_corner_radius_for_major_version(major_version: isize) -> f64 {
    if major_version >= LIQUID_GLASS_MACOS_MAJOR_VERSION {
        LIQUID_GLASS_WINDOW_CORNER_RADIUS_POINTS
    } else {
        LEGACY_WINDOW_CORNER_RADIUS_POINTS
    }
}

/// One step above the status items so capture surfaces cover the menu bar
/// and still sit under the macOS screen-saver / shield levels.
fn capture_surface_window_level() -> objc2_app_kit::NSWindowLevel {
    NSStatusWindowLevel + 1
}

fn capture_surface_collection_behavior() -> NSWindowCollectionBehavior {
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle
}

/// Raises a fullscreen capture surface above the menu bar and keeps it there
/// across spaces and full-screen apps.
fn elevate_fullscreen_capture_window(native_window: &NSWindow) {
    native_window.setLevel(capture_surface_window_level());
    native_window.setHidesOnDeactivate(false);
    native_window.setAcceptsMouseMovedEvents(true);
    native_window.setCollectionBehavior(capture_surface_collection_behavior());
}

fn parse_display_id(display_id: &str) -> Option<u32> {
    display_id.parse().ok()
}

fn clamp_display_corner_radius(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        0.0
    } else {
        // Prefer half-point steps so CSS border-radius stays stable on Retina.
        (value * 2.0).round() / 2.0
    }
}

fn screen_display_id(screen: &NSScreen) -> Option<u32> {
    let key = NSString::from_str("NSScreenNumber");
    let value = screen.deviceDescription().objectForKey(&key)?;
    value
        .downcast_ref::<NSNumber>()
        .map(NSNumber::unsignedIntValue)
}

fn screen_for_display_id(mtm: MainThreadMarker, display_id: &str) -> Option<Retained<NSScreen>> {
    let requested = parse_display_id(display_id)?;
    NSScreen::screens(mtm)
        .into_iter()
        .find(|screen| screen_display_id(screen) == Some(requested))
}

fn screen_corner_radius(screen: &NSScreen) -> f64 {
    // Prefer the display outline path. Private `_displayCornerRadius` KVC keys
    // are missing on macOS 26+ hardware and `valueForKey:` raises
    // `NSUndefinedKeyException`, which aborts the process.
    if let Some(radius) = screen_bezel_corner_radius(screen) {
        let radius = clamp_display_corner_radius(radius);
        if radius > 0.0 {
            return radius;
        }
    }
    if let Some(value) = screen_legacy_corner_radius(screen) {
        let radius = clamp_display_corner_radius(value);
        if radius > 0.0 {
            return radius;
        }
    }
    0.0
}

fn screen_bezel_corner_radius(screen: &NSScreen) -> Option<f64> {
    if !screen.respondsToSelector(sel!(bezelPath)) {
        return None;
    }
    // SAFETY: `respondsToSelector` is true. `bezelPath` returns an
    // `NSBezierPath` (or nil) for the visible display outline.
    let path: Option<Retained<NSBezierPath>> = unsafe { msg_send![screen, bezelPath] };
    let path = path?;
    Some(corner_radius_from_bezel_path(&path, screen.frame()))
}

fn screen_legacy_corner_radius(screen: &NSScreen) -> Option<f64> {
    if screen.respondsToSelector(sel!(_displayCornerRadius)) {
        // SAFETY: selector exists. Older NSScreen builds return CGFloat.
        let value: f64 = unsafe { msg_send![screen, _displayCornerRadius] };
        return Some(value);
    }
    if screen.respondsToSelector(sel!(_cornerRadius)) {
        // SAFETY: selector exists. Older NSScreen builds return CGFloat.
        let value: f64 = unsafe { msg_send![screen, _cornerRadius] };
        return Some(value);
    }
    None
}

fn on_path_points(path: &NSBezierPath) -> Vec<NSPoint> {
    let count = path.elementCount();
    let mut points = Vec::new();
    let mut index = 0;
    while index < count {
        let mut associated = [NSPoint::ZERO; 3];
        // SAFETY: AppKit writes at most three points for a cubic element.
        let element =
            unsafe { path.elementAtIndex_associatedPoints(index, associated.as_mut_ptr()) };
        if element == NSBezierPathElement::MoveTo || element == NSBezierPathElement::LineTo {
            points.push(associated[0]);
        } else if element == NSBezierPathElement::CubicCurveTo {
            points.push(associated[2]);
        } else if element == NSBezierPathElement::QuadraticCurveTo {
            points.push(associated[1]);
        }
        index += 1;
    }
    points
}

fn consider_radius(radius: &mut f64, value: f64) {
    if value.is_finite() && value > *radius {
        *radius = value;
    }
}

/// How far the path's axis-aligned spines stop short of its bounds.
///
/// A rounded rectangle's left-edge points sit `radius` below the top; a
/// square path reaches the corners and yields 0. Control points are ignored
/// so squircles are not mistaken for a smaller radius.
fn corner_radius_from_bezel_path(path: &NSBezierPath, frame: NSRect) -> f64 {
    let points = on_path_points(path);
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for point in &points {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    if !min_x.is_finite() {
        return 0.0;
    }

    const SPINE: f64 = 0.5;
    let mut left_max_y = f64::NEG_INFINITY;
    let mut left_min_y = f64::INFINITY;
    let mut right_max_y = f64::NEG_INFINITY;
    let mut right_min_y = f64::INFINITY;
    let mut top_min_x = f64::INFINITY;
    let mut top_max_x = f64::NEG_INFINITY;
    let mut bottom_min_x = f64::INFINITY;
    let mut bottom_max_x = f64::NEG_INFINITY;
    for point in &points {
        if point.x <= min_x + SPINE {
            left_max_y = left_max_y.max(point.y);
            left_min_y = left_min_y.min(point.y);
        }
        if point.x >= max_x - SPINE {
            right_max_y = right_max_y.max(point.y);
            right_min_y = right_min_y.min(point.y);
        }
        if point.y >= max_y - SPINE {
            top_min_x = top_min_x.min(point.x);
            top_max_x = top_max_x.max(point.x);
        }
        if point.y <= min_y + SPINE {
            bottom_min_x = bottom_min_x.min(point.x);
            bottom_max_x = bottom_max_x.max(point.x);
        }
    }

    let mut radius = 0.0;
    consider_radius(&mut radius, max_y - left_max_y);
    consider_radius(&mut radius, left_min_y - min_y);
    consider_radius(&mut radius, max_y - right_max_y);
    consider_radius(&mut radius, right_min_y - min_y);
    consider_radius(&mut radius, top_min_x - min_x);
    consider_radius(&mut radius, max_x - top_max_x);
    consider_radius(&mut radius, bottom_min_x - min_x);
    consider_radius(&mut radius, max_x - bottom_max_x);

    let max_allowed = frame.size.width.min(frame.size.height) / 2.0;
    if !radius.is_finite() || radius <= 0.0 || !max_allowed.is_finite() {
        0.0
    } else {
        radius.min(max_allowed)
    }
}

fn clip_content_to_display_corners(native_window: &NSWindow, radius: f64) {
    let Some(view) = native_window.contentView() else {
        return;
    };
    view.setWantsLayer(true);
    // SAFETY: `layer` is the view's CALayer after `setWantsLayer:YES`.
    // `setCornerRadius:` / `setMasksToBounds:` / `setOpaque:` /
    // `setBackgroundColor:` are CALayer selectors.
    let layer: Option<Retained<AnyObject>> = unsafe { msg_send![&*view, layer] };
    let Some(layer) = layer else {
        return;
    };
    clear_layer_fill(&layer);
    let radius = radius.max(0.0);
    let _: () = unsafe { msg_send![&*layer, setCornerRadius: radius] };
    let _: () = unsafe { msg_send![&*layer, setMasksToBounds: true] };
}

fn clear_transparent_window_backing(native_window: &NSWindow) {
    native_window.setOpaque(false);
    native_window.setBackgroundColor(Some(&NSColor::clearColor()));
    if let Some(view) = native_window.contentView() {
        clear_transparent_view_backing(&view);
    }
}

fn clear_transparent_webview_backing(window: &WebviewWindow) {
    let _ = window.as_ref().with_webview(|platform_webview| {
        let pointer = platform_webview.inner();
        // SAFETY: Tauri supplies the live WKWebView, which inherits from NSView,
        // for the duration of this callback.
        let Some(webview) = (unsafe { pointer.cast::<NSView>().as_ref() }) else {
            return;
        };
        clear_transparent_view_backing(webview);
    });
}

fn clear_transparent_view_backing(view: &NSView) {
    view.setWantsLayer(true);
    // SAFETY: `layer` is the view's CALayer after `setWantsLayer:YES`.
    let layer: Option<Retained<AnyObject>> = unsafe { msg_send![view, layer] };
    if let Some(layer) = layer {
        clear_layer_fill(&layer);
    }
}

fn clear_layer_fill(layer: &AnyObject) {
    let clear = NSColor::clearColor();
    // SAFETY: CALayer `setOpaque:` / `setBackgroundColor:` match these
    // selectors. `CGColor` stays alive for the `setBackgroundColor:` call
    // because `clear` is still in scope; the layer retains it afterward.
    let _: () = unsafe { msg_send![layer, setOpaque: false] };
    let cg_color: *const c_void = unsafe { msg_send![&*clear, CGColor] };
    let _: () = unsafe { msg_send![layer, setBackgroundColor: cg_color] };
}

/// Visible display corner radius in logical points for the given CGDisplay id.
///
/// Runs on the main thread (hopping there when needed) so capture session
/// setup can stay off the UI thread.
pub fn display_corner_radius_points(display_id: &str) -> f64 {
    if MainThreadMarker::new().is_some() {
        return display_corner_radius_on_main(display_id);
    }
    let id = display_id.to_owned();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        let _ = sender.send(display_corner_radius_on_main(&id));
    });
    receiver
        .recv_timeout(std::time::Duration::from_millis(250))
        .unwrap_or(0.0)
}

fn display_corner_radius_on_main(display_id: &str) -> f64 {
    let Some(mtm) = MainThreadMarker::new() else {
        return 0.0;
    };
    screen_for_display_id(mtm, display_id)
        .map(|screen| screen_corner_radius(&screen))
        .unwrap_or(0.0)
}

/// Installs the capture overlay's native cursor tracker during app startup.
///
/// The overlay is created hidden and reused for every capture. Installing its
/// tracking areas here keeps the first capture on the same path as later ones,
/// instead of doing one-time AppKit setup while the overlay is being focused.
pub fn configure_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    let native = native_window(window)?;
    elevate_fullscreen_capture_window(native);
    clear_transparent_window_backing(native);
    clear_transparent_webview_backing(window);
    native.setAlphaValue(0.0);
    set_tracked_cursor(window, CursorMode::Arrow, CursorSurface::CaptureOverlay)
}

/// Keeps the interactive capture selector above system chrome so a selected
/// display can be outlined from physical edge to physical edge, including the
/// menu-bar strip at the top of a macOS display.
pub fn configure_capture_selector(window: &WebviewWindow) -> Result<(), &'static str> {
    let _main_thread =
        MainThreadMarker::new().ok_or("capture selector setup must run on the main thread")?;
    elevate_fullscreen_capture_window(native_window(window)?);
    Ok(())
}

/// Re-asserts the menu-bar-covering window level after Tauri show/focus.
pub fn elevate_capture_surface(window: &WebviewWindow) -> Result<(), &'static str> {
    elevate_fullscreen_capture_window(native_window(window)?);
    Ok(())
}

/// Pins a fullscreen capture surface to the physical display, including the
/// menu bar, and clips its content to the display's rounded corners.
pub fn cover_display(window: &WebviewWindow, display_id: &str) -> Result<(), &'static str> {
    let mtm =
        MainThreadMarker::new().ok_or("fullscreen capture coverage must run on the main thread")?;
    let native = native_window(window)?;
    elevate_fullscreen_capture_window(native);
    if let Some(screen) = screen_for_display_id(mtm, display_id) {
        native.setFrame_display(screen.frame(), true);
        clip_content_to_display_corners(native, screen_corner_radius(&screen));
    }
    Ok(())
}

/// Makes a reused capture overlay nearly transparent before bringing it onscreen.
///
/// Fully transparent (`0.0`) windows can suspend WKWebView, so the first
/// opaque frame is an unpainted black CALayer. Prime at a tiny alpha instead,
/// matching the recording selector, and clear native backing so that layer
/// cannot flash black while the frozen snapshot decodes.
pub fn prepare_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    elevate_fullscreen_capture_window(native_window);
    clear_transparent_window_backing(native_window);
    clear_transparent_webview_backing(window);
    prime_window_reveal(window)?;
    set_cursor_rects_enabled(native_window, true);
    set_tracked_cursor(window, CursorMode::WebView, CursorSurface::CaptureOverlay)?;
    Ok(())
}

/// Orders the primed overlay onscreen without making it key.
///
/// Tauri's `show()` uses `makeKeyAndOrderFront:`, which focuses the overlay
/// before the snapshot has painted and can flash a black WKWebView surface.
pub fn present_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    prepare_capture_overlay(window)?;
    native_window(window)?.orderFront(None);
    Ok(())
}

/// Applies the capture cursor after the overlay becomes the key window.
pub fn activate_capture_cursor(
    window: &WebviewWindow,
    use_crosshair: bool,
) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    if use_crosshair {
        set_tracked_cursor(window, CursorMode::Crosshair, CursorSurface::CaptureOverlay)?;
        CAPTURE_OVERLAY_OWNS_CURSOR.store(true, Ordering::Release);
        // WebKit and AppKit both rebuild cursor rectangles when focus or
        // modifier-key state changes. Disabling those rectangles while region
        // capture owns the cursor prevents the arrow from being installed for
        // a frame between two crosshair updates.
        set_cursor_rects_enabled(native_window, false);
        NSCursor::crosshairCursor().set();
    } else {
        // Window capture uses a custom CSS camera cursor, so WebKit remains the
        // cursor owner in this mode. Refresh its rectangles after the overlay
        // becomes key and after its fade-in completes.
        set_cursor_rects_enabled(native_window, true);
        set_tracked_cursor(window, CursorMode::WebView, CursorSurface::CaptureOverlay)?;
        CAPTURE_OVERLAY_OWNS_CURSOR.store(true, Ordering::Release);
        native_window.resetCursorRects();
    }
    Ok(())
}

/// Reveals the overlay after WebKit has painted its reset state.
pub fn reveal_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    reveal_window(window)
}

/// Makes a window transparent while keeping it onscreen so WebKit can paint.
pub fn prepare_window_reveal(window: &WebviewWindow) -> Result<(), &'static str> {
    native_window(window)?.setAlphaValue(0.0);
    Ok(())
}

/// Keeps a hidden WebView awake without visibly exposing its cached surface.
///
/// AppKit can suspend a fully transparent WKWebView. A tiny non-zero alpha is
/// enough to let it paint the next frame while remaining imperceptible until
/// `reveal_window` makes the finished surface visible.
pub fn prime_window_reveal(window: &WebviewWindow) -> Result<(), &'static str> {
    native_window(window)?.setAlphaValue(WINDOW_REVEAL_PRIME_ALPHA);
    Ok(())
}

/// Reveals a window after its WebKit surface has painted.
pub fn reveal_window(window: &WebviewWindow) -> Result<(), &'static str> {
    native_window(window)?.setAlphaValue(1.0);
    Ok(())
}

/// Activates an accessory app window and makes it key so keyboard cancellation
/// works even when the selector was launched while another app was frontmost.
pub fn focus_window(window: &WebviewWindow) -> Result<(), &'static str> {
    remember_frontmost_app_before_activation();
    make_key_and_activate(window)
}

/// Activates Captures and makes a document window key without recording a
/// capture frontmost-app anchor.
///
/// Editors and other intentional document surfaces call this after an Edit
/// click in the nonactivating thumbnail panel. Re-asserts on the next main-queue turn
/// so WebKit's asynchronous window creation cannot leave the document surface
/// visible but inactive.
pub fn activate_document_window(window: &WebviewWindow) -> Result<(), &'static str> {
    make_key_and_activate(window)?;
    let window = window.clone();
    DispatchQueue::main().exec_async(move || {
        let _ = make_key_and_activate(&window);
    });
    Ok(())
}

fn make_key_and_activate(window: &WebviewWindow) -> Result<(), &'static str> {
    let main_thread = MainThreadMarker::new().ok_or("window focus must run on the main thread")?;
    let app = NSApplication::sharedApplication(main_thread);
    app.activate();
    native_window(window)?.makeKeyAndOrderFront(None);
    Ok(())
}

/// Records the frontmost app before a transient Captures surface steals
/// activation. No-op when Captures is already frontmost, or when a capture
/// session already recorded an anchor (selector → countdown should not clobber
/// the original frontmost app).
///
/// When another app is frontmost, also conceals open titled document windows
/// (editors, history, preferences) so app activation cannot flash them above
/// the user's work. When Captures is already frontmost — the usual case with an
/// editor open — documents stay visible so region/window selection does not
/// hide and re-show them for the whole capture UI session. The always-on-top
/// overlay still covers them on the capture display.
///
/// Call [`reveal_concealed_document_windows`] only when the full capture UI
/// session ends — not on intermediate frontmost restores (for example overlay →
/// countdown).
///
/// Call this immediately before focusing the screenshot overlay or another
/// capture UI so document windows can be returned to the background afterward.
pub fn remember_frontmost_app_before_activation() {
    {
        let slot = FRONTMOST_APP_BEFORE_CAPTURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
    }
    let frontmost = NSWorkspace::sharedWorkspace().frontmostApplication();
    let current = NSRunningApplication::currentApplication();
    let previous = match frontmost {
        Some(app) if !app.isEqual(Some(&*current)) && !app.isTerminated() => Some(app),
        _ => None,
    };
    // Only order out documents when activation would pull them over another
    // app. Concealing while Captures is already key is what made open editors
    // vanish for the whole region/window selection and pop back at the end.
    if should_conceal_documents_for_capture_activation(previous.is_some()) {
        conceal_document_windows_for_capture();
    }
    let mut slot = FRONTMOST_APP_BEFORE_CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A concurrent activation may have recorded the anchor first; keep it.
    if slot.is_some() {
        return;
    }
    *slot = previous;
}

/// Whether titled documents should be ordered out for this capture activation.
///
/// `other_app_is_frontmost` is true when the workspace frontmost app is not
/// Captures (and is still running). Pure helper for unit tests.
pub(crate) fn should_conceal_documents_for_capture_activation(
    other_app_is_frontmost: bool,
) -> bool {
    other_app_is_frontmost
}

/// Drops a remembered frontmost app without restoring it.
///
/// Use when Captures intentionally keeps focus (for example opening an editor
/// after a recording finishes). Also reveals any capture-concealed documents so
/// an intentional open cannot leave a previous editor ordered out.
pub fn clear_frontmost_app_anchor() {
    let mut slot = FRONTMOST_APP_BEFORE_CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = None;
    // Editor open can arrive off the main thread; still clear concealment.
    if MainThreadMarker::new().is_some() {
        reveal_concealed_document_windows();
    } else {
        DispatchQueue::main().exec_async(|| {
            reveal_concealed_document_windows();
        });
    }
}

/// Hands activation back to the app that was frontmost before a transient
/// capture surface. Prevents open editors from remaining key after a screenshot
/// or cancelled selection while the user was working in another app.
///
/// Does **not** re-show concealed document windows — intermediate restores
/// (region overlay hide before a countdown) would otherwise flash editors for a
/// few frames. Call [`reveal_concealed_document_windows`] when the capture UI
/// session fully ends.
pub fn restore_frontmost_app_after_capture() {
    let previous = {
        let mut slot = FRONTMOST_APP_BEFORE_CAPTURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.take()
    };
    let Some(previous) = previous else {
        return;
    };
    if previous.isTerminated() {
        return;
    }
    if let Some(main_thread) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(main_thread);
        app.yieldActivationToApplication(&previous);
    }
    let _ = previous.activateWithOptions(NSApplicationActivationOptions::empty());
}

/// Orders out titled document windows so capture activation cannot flash them.
///
/// Borderless capture surfaces and nonactivating HUD panels are left alone.
/// Idempotent while a concealment session is already active.
pub fn conceal_document_windows_for_capture() {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    CONCEALED_DOCUMENT_WINDOWS.with(|concealed| {
        let mut concealed = concealed.borrow_mut();
        if !concealed.is_empty() {
            return;
        }
        let app = NSApplication::sharedApplication(main_thread);
        for window in app.windows().iter() {
            if !is_titled_document_window(&window) || !window.isVisible() {
                continue;
            }
            window.orderOut(None);
            concealed.push(window);
        }
    });
}

/// Restores document windows ordered out for capture without activating Captures.
///
/// When another app is frontmost, windows rejoin this app's inactive window list
/// and stay behind the active app. When Captures is frontmost, they reappear with
/// the rest of the app's documents.
pub fn reveal_concealed_document_windows() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let windows =
        CONCEALED_DOCUMENT_WINDOWS.with(|concealed| std::mem::take(&mut *concealed.borrow_mut()));
    for window in windows {
        // Destroyed webviews drop their NSWindow; skip anything already gone or
        // already visible from another path.
        if window.isVisible() {
            continue;
        }
        // orderFront (not orderFrontRegardless) keeps an inactive Captures
        // behind the restored frontmost app instead of floating above it.
        window.orderFront(None);
    }
}

fn is_titled_document_window(window: &NSWindow) -> bool {
    // Editors, history, preferences, and similar document surfaces use a title
    // bar. Capture overlays, countdowns, and floating HUD panels do not.
    style_mask_is_titled_document(window.styleMask())
}

fn style_mask_is_titled_document(mask: NSWindowStyleMask) -> bool {
    mask.contains(NSWindowStyleMask::Titled)
        && !mask.contains(NSWindowStyleMask::NonactivatingPanel)
}

/// Restores native overlay state after a capture ends.
pub fn reset_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    let result = (|| {
        let native_window = native_window(window)?;
        native_window.setAlphaValue(0.0);
        set_cursor_rects_enabled(native_window, true);
        set_tracked_cursor(window, CursorMode::Arrow, CursorSurface::CaptureOverlay)
    })();
    NSCursor::arrowCursor().set();
    CAPTURE_OVERLAY_OWNS_CURSOR.store(false, Ordering::Release);
    result
}

/// Resizes a visible preview stack in one AppKit update while preserving its
/// bottom edge. Callers should only grow a visible stack: shrinking WKWebView
/// blanks surviving cards. Re-asserts bottom layer placement before growing so
/// cached content stays anchored to the stable edge.
pub fn resize_from_bottom(
    window: &WebviewWindow,
    width: f64,
    height: f64,
) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    let current = native_window.frame();
    // Avoid a no-op setFrame, which can still force WKWebView to recompose.
    if (current.size.width - width).abs() < 0.5 && (current.size.height - height).abs() < 0.5 {
        return Ok(());
    }

    // WKWebView can recreate its backing layer; re-apply bottom anchoring when
    // the frame actually changes so growth does not shift painted cards.
    let _ = window.as_ref().with_webview(|platform_webview| {
        let pointer = platform_webview.inner();
        // SAFETY: Tauri supplies the live WKWebView for the duration of this callback.
        if let Some(webview) = unsafe { pointer.cast::<NSView>().as_ref() } {
            anchor_layer_contents_to_bottom(webview);
        }
    });

    let frame = NSRect::new(current.origin, NSSize::new(width, height));
    native_window.setFrame_display(frame, true);
    Ok(())
}

/// Updates the cursor even while another application remains frontmost.
pub fn set_pointing_cursor(window: &WebviewWindow, pointing: bool) -> Result<(), &'static str> {
    set_thumbnail_cursor(
        window,
        if pointing {
            ThumbnailCursorKind::Pointer
        } else {
            ThumbnailCursorKind::Default
        },
    )
}

/// Applies a thumbnail cursor kind even while Captures is not frontmost.
///
/// Preview cards use:
/// - `Pointer` over action buttons
/// - `Grab` over the image (file drag source)
/// - `Default` when the pointer is outside a live card
pub fn set_thumbnail_cursor(
    window: &WebviewWindow,
    kind: ThumbnailCursorKind,
) -> Result<(), &'static str> {
    if capture_overlay_owns_cursor() {
        return reset_pointing_cursor_state(window);
    }
    let native_window = native_window(window)?;
    let interactive = !matches!(kind, ThumbnailCursorKind::Default);
    let mode = match kind {
        ThumbnailCursorKind::Default => CursorMode::Arrow,
        ThumbnailCursorKind::Pointer => CursorMode::PointingHand,
        ThumbnailCursorKind::Grab => CursorMode::OpenHand,
    };
    if !interactive || mode != thumbnail_cursor_mode() {
        THUMBNAIL_KEY_WINDOW_ALLOWED.store(true, Ordering::Release);
    }
    // A nonactivating panel can become key without activating Captures. AppKit
    // only displays this app's NSCursor while the panel is key, so take key for
    // the live card and release it again over click-through/empty space.
    if interactive
        && cursor_surface_can_take_key_window(CursorSurface::Thumbnail)
        && !native_window.isKeyWindow()
    {
        native_window.makeKeyWindow();
    } else if !interactive && native_window.isKeyWindow() {
        native_window.resignKeyWindow();
    }
    set_cursor_rects_enabled(native_window, !interactive);
    set_tracked_cursor(window, mode, CursorSurface::Thumbnail)?;
    apply_thumbnail_ns_cursor(kind);
    // Becoming key can re-enable rectangles asynchronously after this returns.
    // Re-disable and re-set so grab survives a stationary entry onto the image.
    if interactive {
        set_cursor_rects_enabled(native_window, false);
        apply_thumbnail_ns_cursor(kind);
    }
    Ok(())
}

/// Reapplies the current interactive thumbnail cursor without rebuilding
/// WebKit cursor rectangles.
///
/// macOS restores the frontmost application's arrow when Captures becomes
/// inactive, even though the preview can still be hovering the same control.
/// Cursor rectangles remain disabled while a non-default cursor is active, so
/// setting the native cursor again restores the hand without flicker.
pub fn reassert_pointing_cursor(window: &WebviewWindow) -> Result<(), &'static str> {
    reassert_thumbnail_cursor(window, ThumbnailCursorKind::Pointer)
}

/// Reapplies a non-default thumbnail cursor (pointer or grab).
pub fn reassert_thumbnail_cursor(
    window: &WebviewWindow,
    kind: ThumbnailCursorKind,
) -> Result<(), &'static str> {
    if matches!(kind, ThumbnailCursorKind::Default) {
        return reset_pointing_cursor_state(window);
    }
    if capture_overlay_owns_cursor() {
        return reset_pointing_cursor_state(window);
    }
    let native_window = native_window(window)?;
    if cursor_surface_can_take_key_window(CursorSurface::Thumbnail) && !native_window.isKeyWindow()
    {
        native_window.makeKeyWindow();
    }
    set_cursor_rects_enabled(native_window, false);
    let mode = match kind {
        ThumbnailCursorKind::Default => CursorMode::Arrow,
        ThumbnailCursorKind::Pointer => CursorMode::PointingHand,
        ThumbnailCursorKind::Grab => CursorMode::OpenHand,
    };
    set_tracked_cursor(window, mode, CursorSurface::Thumbnail)?;
    apply_thumbnail_ns_cursor(kind);
    set_cursor_rects_enabled(native_window, false);
    apply_thumbnail_ns_cursor(kind);
    Ok(())
}

fn apply_thumbnail_ns_cursor(kind: ThumbnailCursorKind) {
    let mode = match kind {
        ThumbnailCursorKind::Default => CursorMode::Arrow,
        ThumbnailCursorKind::Pointer => CursorMode::PointingHand,
        ThumbnailCursorKind::Grab => CursorMode::OpenHand,
    };
    apply_cursor_mode(mode);
}

/// Clears the preview's stored pointing cursor without changing the cursor
/// currently owned by another window.
pub fn reset_pointing_cursor_state(window: &WebviewWindow) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    set_cursor_rects_enabled(native_window, true);
    set_tracked_cursor(window, CursorMode::Arrow, CursorSurface::Thumbnail)
}

fn set_tracked_cursor(
    window: &WebviewWindow,
    mode: CursorMode,
    surface: CursorSurface,
) -> Result<(), &'static str> {
    window
        .as_ref()
        .with_webview(move |platform_webview| {
            let pointer = platform_webview.inner();
            // SAFETY: Tauri supplies a live WKWebView, which inherits from
            // NSView, for the duration of this callback.
            let Some(webview) = (unsafe { pointer.cast::<NSView>().as_ref() }) else {
                return;
            };
            install_cursor_tracker(webview, mode, surface);
        })
        .map_err(|_| "macOS webview handle is unavailable")
}

fn install_cursor_tracker(webview: &NSView, mode: CursorMode, surface: CursorSurface) {
    if let Some(owner) = associated_cursor_tracker(webview) {
        owner.set_mode(mode);
        return;
    }

    let owner = CursorTrackingOwner::new(mode, surface);
    let options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    // SAFETY: The owner implements each callback requested by these options.
    // The view retains the tracking area, and the association below retains
    // its owner for exactly as long as the WKWebView lives.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::ZERO,
            options,
            Some(&owner),
            None,
        )
    };
    webview.addTrackingArea(&area);

    let cursor_options = NSTrackingAreaOptions::CursorUpdate
        | NSTrackingAreaOptions::ActiveInKeyWindow
        | NSTrackingAreaOptions::InVisibleRect;
    // Cursor updates cannot share the `ActiveAlways` tracking area above.
    // Key-capable inactive HUDs and the thumbnail use this second area for
    // standard cursor-update callbacks while hovered. The `ActiveAlways`
    // tracker still owns enter/move/exit while another app is active.
    let cursor_area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::ZERO,
            cursor_options,
            Some(&owner),
            None,
        )
    };
    webview.addTrackingArea(&cursor_area);

    let object = ptr::from_ref(webview).cast::<AnyObject>().cast_mut();
    let value = Retained::as_ptr(&owner).cast::<AnyObject>().cast_mut();
    // SAFETY: `object` and `value` are live Objective-C objects. This process-
    // local key is stable, and the retain policy keeps the owner alive.
    unsafe {
        objc_setAssociatedObject(
            object,
            cursor_tracker_association_key(),
            value,
            OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
        if let Some(window) = webview.window() {
            let window_object = ptr::from_ref(&*window).cast::<AnyObject>().cast_mut();
            objc_setAssociatedObject(
                window_object,
                cursor_tracker_window_association_key(),
                value,
                OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
        }
    }
}

fn associated_cursor_tracker(webview: &NSView) -> Option<&CursorTrackingOwner> {
    let object = ptr::from_ref(webview).cast::<AnyObject>();
    // SAFETY: Only `install_cursor_tracker` stores a value under this private
    // key, and it always stores a retained `CursorTrackingOwner`.
    let owner = unsafe { objc_getAssociatedObject(object, cursor_tracker_association_key()) };
    unsafe { owner.cast::<CursorTrackingOwner>().as_ref() }
}

fn cursor_tracker_association_key() -> *const c_void {
    ptr::addr_of!(CURSOR_TRACKER_ASSOCIATION_KEY).cast()
}

fn cursor_tracker_window_association_key() -> *const c_void {
    ptr::addr_of!(CURSOR_TRACKER_WINDOW_ASSOCIATION_KEY).cast()
}

fn cursor_surface_for_window(window: &NSWindow) -> Option<CursorSurface> {
    let object = ptr::from_ref(window).cast::<AnyObject>();
    // SAFETY: `install_cursor_tracker` stores only a retained
    // `CursorTrackingOwner` under this process-local key.
    let owner =
        unsafe { objc_getAssociatedObject(object, cursor_tracker_window_association_key()) };
    unsafe { owner.cast::<CursorTrackingOwner>().as_ref() }.map(|tracker| tracker.ivars().surface)
}

fn set_cursor_rects_enabled(window: &NSWindow, enabled: bool) {
    if enabled && !window.areCursorRectsEnabled() {
        window.enableCursorRects();
    } else if !enabled && window.areCursorRectsEnabled() {
        window.disableCursorRects();
    }
}

fn capture_overlay_owns_cursor() -> bool {
    CAPTURE_OVERLAY_OWNS_CURSOR.load(Ordering::Acquire)
}

fn cursor_surface_can_apply(surface: CursorSurface, capture_active: bool) -> bool {
    surface == CursorSurface::CaptureOverlay || !capture_active
}

fn cursor_surface_uses_key_window(surface: CursorSurface) -> bool {
    matches!(
        surface,
        CursorSurface::InactiveHud | CursorSurface::Thumbnail
    )
}

fn cursor_surface_can_take_key_window_with_thumbnail_allowed(
    surface: CursorSurface,
    thumbnail_allowed: bool,
) -> bool {
    cursor_surface_uses_key_window(surface)
        && (surface != CursorSurface::Thumbnail || thumbnail_allowed)
}

fn cursor_surface_can_take_key_window(surface: CursorSurface) -> bool {
    cursor_surface_can_take_key_window_with_thumbnail_allowed(
        surface,
        THUMBNAIL_KEY_WINDOW_ALLOWED.load(Ordering::Acquire),
    )
}

fn should_reset_cursor_on_exit(surface: CursorSurface, capture_active: bool) -> bool {
    surface != CursorSurface::CaptureOverlay && !capture_active
}

fn native_window(window: &WebviewWindow) -> Result<&NSWindow, &'static str> {
    let pointer = window
        .ns_window()
        .map_err(|_| "macOS window handle is unavailable")?;
    // SAFETY: Tauri returned the NSWindow belonging to the borrowed live
    // `WebviewWindow`; the reference cannot outlive that borrow.
    unsafe { pointer.cast::<NSWindow>().as_ref() }.ok_or("macOS window handle is null")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use objc2::sel;
    use objc2_app_kit::{
        NSBezierPath, NSEventModifierFlags, NSEventType, NSMainMenuWindowLevel, NSWindowStyleMask,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

    use super::{
        CAPTURE_OVERLAY_OWNS_CURSOR, CursorMode, CursorSurface, THUMBNAIL_CURSOR_MODE,
        capture_surface_collection_behavior, capture_surface_window_level,
        clamp_display_corner_radius, corner_radius_from_bezel_path, cursor_mode_is_interactive,
        cursor_surface_can_apply, cursor_surface_can_take_key_window_with_thumbnail_allowed,
        cursor_surface_uses_key_window, display_corner_radius_points, parse_display_id,
        reassert_thumbnail_cursor_after_click, shortcut_modifiers_pressed,
        should_conceal_documents_for_capture_activation, should_release_thumbnail_key_after_event,
        should_reset_cursor_on_exit, style_mask_is_titled_document,
        window_corner_radius_for_major_version,
    };

    #[test]
    fn conceals_documents_only_when_another_app_is_frontmost() {
        // Editor open + capture shortcut: Captures is already frontmost — keep
        // the editor on screen for the whole selection session.
        assert!(!should_conceal_documents_for_capture_activation(false));
        // Capture while Chrome/Discord is key: order out so activation cannot
        // flash editors above the user's work when the overlay dismisses.
        assert!(should_conceal_documents_for_capture_activation(true));
    }

    #[test]
    fn titled_document_mask_matches_editors_not_capture_surfaces() {
        assert!(style_mask_is_titled_document(
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable
        ));
        assert!(!style_mask_is_titled_document(
            NSWindowStyleMask::Borderless
        ));
        assert!(!style_mask_is_titled_document(
            NSWindowStyleMask::Titled | NSWindowStyleMask::NonactivatingPanel
        ));
        assert!(!style_mask_is_titled_document(
            NSWindowStyleMask::NonactivatingPanel
        ));
    }

    #[test]
    fn waits_for_shortcut_modifiers_but_not_lock_keys() {
        assert!(shortcut_modifiers_pressed(
            NSEventModifierFlags::Control | NSEventModifierFlags::Shift
        ));
        assert!(shortcut_modifiers_pressed(
            NSEventModifierFlags::Option | NSEventModifierFlags::Command
        ));
        assert!(!shortcut_modifiers_pressed(NSEventModifierFlags::CapsLock));
        assert!(!shortcut_modifiers_pressed(NSEventModifierFlags::empty()));
    }

    #[test]
    fn uses_the_window_radius_for_each_macos_design_generation() {
        assert_eq!(window_corner_radius_for_major_version(15), 10.0);
        assert_eq!(window_corner_radius_for_major_version(26), 25.0);
        assert_eq!(window_corner_radius_for_major_version(27), 25.0);
    }

    #[test]
    fn capture_surfaces_sit_above_the_menu_bar() {
        assert!(capture_surface_window_level() > NSMainMenuWindowLevel);
    }

    #[test]
    fn capture_surfaces_join_spaces_as_fullscreen_auxiliaries() {
        let behavior = capture_surface_collection_behavior();
        assert!(behavior.contains(objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(behavior.contains(objc2_app_kit::NSWindowCollectionBehavior::FullScreenAuxiliary));
    }

    #[test]
    fn parses_xcap_display_ids() {
        assert_eq!(parse_display_id("1"), Some(1));
        assert_eq!(parse_display_id("69733382"), Some(69_733_382));
        assert_eq!(parse_display_id("display-1"), None);
    }

    #[test]
    fn clamps_display_corner_radius_to_half_points() {
        assert_eq!(clamp_display_corner_radius(-1.0), 0.0);
        assert_eq!(clamp_display_corner_radius(f64::NAN), 0.0);
        assert_eq!(clamp_display_corner_radius(36.997_622_963_456_48), 37.0);
        assert_eq!(clamp_display_corner_radius(38.2), 38.0);
        assert_eq!(clamp_display_corner_radius(38.6), 38.5);
        assert_eq!(clamp_display_corner_radius(54.0), 54.0);
    }

    #[test]
    fn rounded_rect_bezel_path_reports_its_radius() {
        let frame = NSRect::new(NSPoint::new(10.0, 20.0), NSSize::new(200.0, 120.0));
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(frame, 12.0, 12.0);
        assert_eq!(
            clamp_display_corner_radius(corner_radius_from_bezel_path(&path, frame)),
            12.0
        );
    }

    #[test]
    fn rectangular_bezel_path_reports_no_radius() {
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(100.0, 80.0));
        let path = NSBezierPath::bezierPathWithRect(frame);
        assert_eq!(
            clamp_display_corner_radius(corner_radius_from_bezel_path(&path, frame)),
            0.0
        );
    }

    #[test]
    fn missing_legacy_display_corner_selectors_are_ignored() {
        let object = objc2_foundation::NSObject::new();
        assert!(!object.respondsToSelector(sel!(bezelPath)));
        assert!(!object.respondsToSelector(sel!(_displayCornerRadius)));
        assert!(!object.respondsToSelector(sel!(_cornerRadius)));
    }

    #[test]
    fn live_display_corner_lookup_does_not_abort() {
        let radius = display_corner_radius_points("1");
        assert!(radius.is_finite());
        assert!(radius >= 0.0);
    }

    #[test]
    fn active_capture_overlay_blocks_thumbnail_cursor_updates() {
        assert!(cursor_surface_can_apply(
            CursorSurface::CaptureOverlay,
            true
        ));
        assert!(!cursor_surface_can_apply(CursorSurface::Thumbnail, true));
        assert!(cursor_surface_can_apply(CursorSurface::Thumbnail, false));
        assert!(!cursor_surface_can_apply(CursorSurface::InactiveHud, true));
        assert!(cursor_surface_can_apply(CursorSurface::InactiveHud, false));
    }

    #[test]
    fn inactive_interactive_surfaces_take_key_window_status_on_hover() {
        assert!(cursor_surface_uses_key_window(CursorSurface::InactiveHud));
        assert!(cursor_surface_uses_key_window(CursorSurface::Thumbnail));
        assert!(!cursor_surface_uses_key_window(
            CursorSurface::CaptureOverlay
        ));
    }

    #[test]
    fn thumbnail_releases_key_status_after_primary_click_delivery() {
        assert!(should_release_thumbnail_key_after_event(
            Some(CursorSurface::Thumbnail),
            NSEventType::LeftMouseUp
        ));
        assert!(!should_release_thumbnail_key_after_event(
            Some(CursorSurface::Thumbnail),
            NSEventType::LeftMouseDown
        ));
        assert!(!should_release_thumbnail_key_after_event(
            Some(CursorSurface::InactiveHud),
            NSEventType::LeftMouseUp
        ));
        assert!(!cursor_surface_can_take_key_window_with_thumbnail_allowed(
            CursorSurface::Thumbnail,
            false
        ));
        assert!(cursor_surface_can_take_key_window_with_thumbnail_allowed(
            CursorSurface::Thumbnail,
            true
        ));
        assert!(cursor_surface_can_take_key_window_with_thumbnail_allowed(
            CursorSurface::InactiveHud,
            false
        ));
    }

    #[test]
    fn inactive_surfaces_reset_the_cursor_when_capture_is_not_active() {
        assert!(should_reset_cursor_on_exit(CursorSurface::Thumbnail, false));
        assert!(should_reset_cursor_on_exit(
            CursorSurface::InactiveHud,
            false
        ));
        assert!(!should_reset_cursor_on_exit(CursorSurface::Thumbnail, true));
        assert!(!should_reset_cursor_on_exit(
            CursorSurface::CaptureOverlay,
            true
        ));
    }

    #[test]
    fn interactive_cursor_modes_cover_preview_buttons_and_drag() {
        assert!(cursor_mode_is_interactive(CursorMode::PointingHand));
        assert!(cursor_mode_is_interactive(CursorMode::OpenHand));
        assert!(cursor_mode_is_interactive(CursorMode::Crosshair));
        assert!(!cursor_mode_is_interactive(CursorMode::Arrow));
        assert!(!cursor_mode_is_interactive(CursorMode::WebView));
    }

    #[test]
    fn click_reassert_only_runs_for_interactive_thumbnail_cursors() {
        let previous_mode = THUMBNAIL_CURSOR_MODE.swap(CursorMode::Arrow as u8, Ordering::AcqRel);
        let previous_overlay = CAPTURE_OVERLAY_OWNS_CURSOR.swap(false, Ordering::AcqRel);

        assert!(!reassert_thumbnail_cursor_after_click());

        THUMBNAIL_CURSOR_MODE.store(CursorMode::PointingHand as u8, Ordering::Release);
        assert!(reassert_thumbnail_cursor_after_click());

        THUMBNAIL_CURSOR_MODE.store(CursorMode::OpenHand as u8, Ordering::Release);
        assert!(reassert_thumbnail_cursor_after_click());

        CAPTURE_OVERLAY_OWNS_CURSOR.store(true, Ordering::Release);
        assert!(!reassert_thumbnail_cursor_after_click());

        CAPTURE_OVERLAY_OWNS_CURSOR.store(previous_overlay, Ordering::Release);
        THUMBNAIL_CURSOR_MODE.store(previous_mode, Ordering::Release);
    }
}
