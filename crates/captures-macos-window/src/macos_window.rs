use crate::conceal_policy::should_conceal_documents_for_capture_activation;
use crate::cursor_policy::{
    CaptureCursor, CaptureCursorEvent, CaptureCursorKind, CaptureCursorMonitorAction,
    ThumbnailHoverCursor, capture_cursor_monitor_action, overlay_prepare_keeps_native_cursor,
    suppress_document_cursor_rects_for_thumbnail, thumbnail_may_take_key_window,
    thumbnail_passthrough_disables_cursor_rects, thumbnail_poll_is_live,
    thumbnail_resets_cursor_on_exit, thumbnail_unpolled_hover,
};

use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class,
    ffi::{OBJC_ASSOCIATION_RETAIN_NONATOMIC, objc_getAssociatedObject, objc_setAssociatedObject},
    msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationDidResignActiveNotification,
    NSBackingStoreType, NSBezierPath, NSBezierPathElement, NSColor, NSCursor, NSEvent, NSEventMask,
    NSEventType, NSPanel, NSPasteboard, NSRunningApplication, NSScreen, NSSound,
    NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSViewLayerContentsPlacement, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType,
    NSWindowStyleMask, NSWorkspace, NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNumber, NSObject, NSObjectProtocol, NSOperationQueue,
    NSPoint, NSProcessInfo, NSRect, NSSize, NSString,
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

#[path = "symbolic_hotkeys.rs"]
mod symbolic_hotkeys;

pub use symbolic_hotkeys::disable_symbolic_hotkeys;

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
const APPKIT_HOP_TIMEOUT: Duration = Duration::from_secs(2);

/// True when the calling thread is AppKit's main thread.
pub fn is_main_thread() -> bool {
    MainThreadMarker::new().is_some()
}

/// Runs `work` on the AppKit main thread, hopping there when needed.
///
/// macOS 26 traps AppKit use off the main thread (`Must only be used from the
/// main thread`). Capture, clipboard, and overlay work often starts on a
/// tokio worker, so hop before touching NSWindow / NSCursor / NSPasteboard.
///
/// Unlike Tauri's `run_on_main_thread`, this waits for `work` to finish (up to
/// two seconds). If the hop times out, AppKit is not run on the caller — that
/// would reintroduce the trap.
pub fn run_on_main<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    if is_main_thread() {
        return Some(work());
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        let _ = sender.send(work());
    });
    match receiver.recv_timeout(APPKIT_HOP_TIMEOUT) {
        Ok(value) => Some(value),
        Err(_) => {
            eprintln!("timed out waiting for AppKit work on the main thread");
            None
        }
    }
}

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

impl CaptureCursorKind {
    fn to_cursor_mode(self) -> CursorMode {
        match self {
            Self::Crosshair => CursorMode::Crosshair,
            Self::WebView => CursorMode::WebView,
            Self::Arrow => CursorMode::Arrow,
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
    view: Cell<Option<ptr::NonNull<NSView>>>,
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
            view: Cell::new(None),
        });
        // SAFETY: `NSObject`'s `init` method has this signature.
        let owner = unsafe { msg_send![super(this), init] };
        if surface == CursorSurface::Thumbnail {
            publish_thumbnail_cursor_mode(mode);
        }
        owner
    }

    fn set_view(&self, view: &NSView) {
        self.ivars().view.set(Some(ptr::NonNull::from(view)));
        if self.ivars().surface == CursorSurface::Thumbnail
            && let Some(window) = view.window()
        {
            remember_thumbnail_window(&window);
        }
    }

    fn tracked_window(&self) -> Option<Retained<NSWindow>> {
        let view = self.ivars().view.get()?;
        // SAFETY: `set_view` stores the live WKWebView that owns this tracker.
        unsafe { view.as_ref() }.window()
    }

    fn event_or_tracked_window(&self, event: Option<&NSEvent>) -> Option<Retained<NSWindow>> {
        let main_thread = MainThreadMarker::new()?;
        if let Some(event) = event
            && let Some(window) = event.window(main_thread)
        {
            return Some(window);
        }
        self.tracked_window()
    }

    fn set_mode(&self, mode: CursorMode) {
        self.ivars().mode.set(mode);
        if self.ivars().surface == CursorSurface::Thumbnail {
            publish_thumbnail_cursor_mode(mode);
        }
        // Capture surfaces and mini previews appear under a stationary pointer.
        // Apply immediately so the mode does not wait for mouseEntered.
        if self.ivars().surface == CursorSurface::Thumbnail
            || (self.ivars().surface == CursorSurface::CaptureOverlay
                && capture_overlay_owns_cursor())
        {
            self.apply_cursor(None);
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
        // Passthrough regions keep the panel tall after collapse. Becoming key
        // there would install WebKit's default cursor over the app underneath.
        if self.ivars().surface == CursorSurface::Thumbnail
            && !cursor_mode_is_interactive(self.effective_thumbnail_cursor_mode())
        {
            return;
        }
        if let Some(window) = self.event_or_tracked_window(Some(event))
            && !window.isKeyWindow()
        {
            if self.ivars().surface == CursorSurface::Thumbnail {
                remember_frontmost_app_before_thumbnail_key();
            }
            window.makeKeyWindow();
            // Becoming key lets WebKit re-enable cursor rectangles. Keep them
            // off while AppKit owns an interactive grab/pointer cursor so the
            // arrow and CSS pointer cannot alternate every mouse event.
            if cursor_mode_is_interactive(self.effective_thumbnail_cursor_mode()) {
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
        if let Some(window) = self.event_or_tracked_window(Some(event))
            && window.isKeyWindow()
        {
            if self.ivars().surface == CursorSurface::Thumbnail {
                restore_competing_cursor_rects();
                resign_ns_window_key_without_raising_documents(&window);
            } else {
                window.resignKeyWindow();
            }
        }
    }

    fn effective_thumbnail_cursor_mode(&self) -> CursorMode {
        let mode = self.ivars().mode.get();
        if self.ivars().surface != CursorSurface::Thumbnail {
            return mode;
        }
        let hover = cursor_mode_to_thumbnail_hover(mode);
        let hover = if pointer_inside_thumbnail_window() {
            thumbnail_unpolled_hover(thumbnail_pointer_poll_is_live(), hover)
        } else {
            hover
        };
        thumbnail_hover_to_cursor_mode(hover)
    }

    fn apply_cursor(&self, event: Option<&NSEvent>) {
        if !cursor_surface_can_apply(self.ivars().surface, capture_overlay_owns_cursor()) {
            return;
        }
        let mode = if self.ivars().surface == CursorSurface::Thumbnail {
            self.effective_thumbnail_cursor_mode()
        } else {
            self.ivars().mode.get()
        };
        if self.ivars().surface == CursorSurface::Thumbnail && !cursor_mode_is_interactive(mode) {
            return;
        }
        if let Some(window) = self.event_or_tracked_window(event) {
            if cursor_mode_is_interactive(mode) {
                set_cursor_rects_enabled(&window, false);
            }
            suppress_competing_cursor_rects_if_needed(mode);
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

fn should_rearm_thumbnail_key_window(interactive: bool, mode_changed: bool) -> bool {
    interactive && mode_changed
}

/// Last AppKit cursor mode published for the always-on-top thumbnail stack.
///
/// Click handling lives in WebKit, which resets `NSCursor` to the arrow on
/// primary-button down/up. JS reassert arrives one IPC hop too late and flashes
/// the default arrow. A process-local event monitor re-applies this mode on the
/// same run loop as the mouse event (and once more on the next turn).
static THUMBNAIL_CURSOR_MODE: AtomicU8 = AtomicU8::new(CursorMode::Arrow as u8);
static THUMBNAIL_POINTER_POLL_AT_MS: AtomicU64 = AtomicU64::new(0);
static THUMBNAIL_POINTER_POLL_CLOCK: OnceLock<Instant> = OnceLock::new();
// Mouse-up releases thumbnail key status so the frontmost app cannot remain
// visually inactive under a stationary pointer. Leaving/re-entering the card or
// moving to a different cursor region rearms key-on-hover.
static THUMBNAIL_KEY_WINDOW_ALLOWED: AtomicBool = AtomicBool::new(true);
// The thumbnail WebView stays ordered onscreen on macOS when concealed. This
// avoids AppKit donating key status to an open editor when the final preview is
// removed, while still letting the desktop layer make the panel click-through.
static THUMBNAIL_PRESENTED: AtomicBool = AtomicBool::new(false);
static THUMBNAIL_CLICK_CURSOR_MONITOR: Mutex<Option<MainThreadMonitor>> = Mutex::new(None);
static GLOBAL_CURSOR_MONITOR: Mutex<Option<MainThreadMonitor>> = Mutex::new(None);
static CURSOR_CLAIM_PANEL: Mutex<Option<MainThreadPanel>> = Mutex::new(None);
static THUMBNAIL_WINDOW_PTR: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub fn note_thumbnail_pointer_poll() {
    let clock = THUMBNAIL_POINTER_POLL_CLOCK.get_or_init(Instant::now);
    THUMBNAIL_POINTER_POLL_AT_MS.store(
        clock
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
            .max(1),
        Ordering::Release,
    );
}

#[must_use]
pub fn thumbnail_pointer_poll_is_live() -> bool {
    let clock = THUMBNAIL_POINTER_POLL_CLOCK.get_or_init(Instant::now);
    let now_ms = clock.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let last_poll_ms = THUMBNAIL_POINTER_POLL_AT_MS.load(Ordering::Acquire);
    last_poll_ms != 0 && thumbnail_poll_is_live(now_ms.saturating_sub(last_poll_ms))
}

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

/// Same Send/Sync contract as [`MainThreadMonitor`] for the cursor-claim
/// NSPanel used while the capture overlay is not yet key.
#[allow(dead_code)]
struct MainThreadPanel(Retained<NSPanel>);

// SAFETY: Created and used only on AppKit's main thread.
unsafe impl Send for MainThreadPanel {}
// SAFETY: Same as `Send`.
unsafe impl Sync for MainThreadPanel {}

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

fn cursor_mode_to_thumbnail_hover(mode: CursorMode) -> ThumbnailHoverCursor {
    match mode {
        CursorMode::PointingHand => ThumbnailHoverCursor::Pointer,
        CursorMode::OpenHand => ThumbnailHoverCursor::Grab,
        _ => ThumbnailHoverCursor::Default,
    }
}

fn thumbnail_hover_to_cursor_mode(kind: ThumbnailHoverCursor) -> CursorMode {
    match kind {
        ThumbnailHoverCursor::Default => CursorMode::WebView,
        ThumbnailHoverCursor::Pointer => CursorMode::PointingHand,
        ThumbnailHoverCursor::Grab => CursorMode::OpenHand,
    }
}

fn thumbnail_kind_to_hover(kind: ThumbnailCursorKind) -> ThumbnailHoverCursor {
    match kind {
        ThumbnailCursorKind::Default => ThumbnailHoverCursor::Default,
        ThumbnailCursorKind::Pointer => ThumbnailHoverCursor::Pointer,
        ThumbnailCursorKind::Grab => ThumbnailHoverCursor::Grab,
    }
}

fn remember_thumbnail_window(window: &NSWindow) {
    THUMBNAIL_WINDOW_PTR.store(ptr::from_ref(window) as usize, Ordering::Release);
}

fn thumbnail_ns_window() -> Option<&'static NSWindow> {
    let pointer = THUMBNAIL_WINDOW_PTR.load(Ordering::Acquire) as *const NSWindow;
    // SAFETY: The thumbnail NSWindow is process-lived; `configure_thumbnail_inactive_hover`
    // stores it and the panel is never destroyed for the rest of the session.
    unsafe { pointer.as_ref() }
}

fn point_in_ns_rect(point: NSPoint, rect: NSRect) -> bool {
    point.x >= rect.origin.x
        && point.y >= rect.origin.y
        && point.x < rect.origin.x + rect.size.width
        && point.y < rect.origin.y + rect.size.height
}

fn pointer_inside_thumbnail_window() -> bool {
    let Some(window) = thumbnail_ns_window() else {
        return false;
    };
    point_in_ns_rect(NSEvent::mouseLocation(), window.frame())
}

fn app_is_active() -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(main_thread).isActive()
}

/// Re-apply the interactive thumbnail cursor after a click/key-window handoff.
///
/// Returns whether an interactive cursor was reasserted (for tests).
fn reassert_thumbnail_cursor_after_click() -> bool {
    apply_interactive_thumbnail_cursor(false)
}

fn apply_unpolled_thumbnail_hover_cursor() -> bool {
    apply_interactive_thumbnail_cursor(true)
}

fn apply_interactive_thumbnail_cursor(promote_unpolled: bool) -> bool {
    if capture_overlay_owns_cursor() {
        return false;
    }
    let stored = cursor_mode_to_thumbnail_hover(thumbnail_cursor_mode());
    let hover = if promote_unpolled {
        stored.unpolled_hover()
    } else {
        stored
    };
    let mode = thumbnail_hover_to_cursor_mode(hover);
    if !cursor_mode_is_interactive(mode) {
        return false;
    }
    apply_cursor_mode(mode);
    suppress_competing_cursor_rects_if_needed(mode);
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
        resign_ns_window_key_without_raising_documents(&window);
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
        let event_type = event_ref.r#type();
        let click_handoff =
            event_type == NSEventType::LeftMouseDown || event_type == NSEventType::LeftMouseUp;
        let over_thumbnail = pointer_inside_thumbnail_window();
        let thumbnail_window = thumbnail_key_window_for_mouse_up(event_ref);
        if thumbnail_window.is_some() {
            THUMBNAIL_KEY_WINDOW_ALLOWED.store(false, Ordering::Release);
        }
        if !click_handoff && !over_thumbnail {
            return event.as_ptr();
        }
        let reasserted = if over_thumbnail && (click_handoff || !app_is_active()) {
            if !thumbnail_pointer_poll_is_live() {
                apply_unpolled_thumbnail_hover_cursor()
            } else if click_handoff {
                reassert_thumbnail_cursor_after_click()
            } else {
                false
            }
        } else {
            reassert_thumbnail_cursor_after_click()
        };
        if click_handoff && (reasserted || thumbnail_window.is_some()) {
            // WebKit installs the arrow while handling the click, then an
            // opening editor's cursor rectangles can restore it again. Reassert
            // across the next main-queue turns, and release thumbnail key
            // status so a Copy/Save/Delete click cannot leave the app
            // underneath inactive.
            DispatchQueue::main().exec_async(move || {
                let _ = if thumbnail_pointer_poll_is_live() {
                    reassert_thumbnail_cursor_after_click()
                } else {
                    apply_unpolled_thumbnail_hover_cursor()
                };
                if let Some(window_address) = thumbnail_window {
                    release_thumbnail_key_window(window_address);
                }
                DispatchQueue::main().exec_async(|| {
                    let _ = if thumbnail_pointer_poll_is_live() {
                        reassert_thumbnail_cursor_after_click()
                    } else {
                        apply_unpolled_thumbnail_hover_cursor()
                    };
                    DispatchQueue::main().exec_async(|| {
                        let _ = if thumbnail_pointer_poll_is_live() {
                            reassert_thumbnail_cursor_after_click()
                        } else {
                            apply_unpolled_thumbnail_hover_cursor()
                        };
                    });
                });
            });
        }
        event.as_ptr()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown
                | NSEventMask::LeftMouseUp
                | NSEventMask::LeftMouseDragged
                | NSEventMask::MouseMoved,
            &block,
        )
    };
    *guard = monitor.map(MainThreadMonitor);
}

fn suppress_competing_cursor_rects_if_needed(mode: CursorMode) {
    if capture_overlay_owns_cursor() {
        return;
    }
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let Some(key) = NSApplication::sharedApplication(main_thread).keyWindow() else {
        return;
    };
    let is_thumbnail = cursor_surface_for_window(&key) == Some(CursorSurface::Thumbnail);
    if !suppress_document_cursor_rects_for_thumbnail(
        cursor_mode_is_interactive(mode),
        is_thumbnail,
        is_titled_document_window(&key),
    ) {
        return;
    }
    set_cursor_rects_enabled(&key, false);
    SUPPRESSED_CURSOR_RECT_WINDOW.with(|slot| {
        *slot.borrow_mut() = Some(key);
    });
}

fn restore_competing_cursor_rects() {
    SUPPRESSED_CURSOR_RECT_WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().take() {
            set_cursor_rects_enabled(&window, true);
        }
    });
}

fn ensure_global_cursor_monitor() {
    let Ok(mut guard) = GLOBAL_CURSOR_MONITOR.lock() else {
        return;
    };
    if guard.is_some() {
        return;
    }
    let block = RcBlock::new(|event: ptr::NonNull<NSEvent>| {
        // SAFETY: AppKit supplies a live NSEvent for the duration of the global
        // monitor callback.
        handle_global_cursor_event(unsafe { event.as_ref() });
    });
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::MouseMoved
            | NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseUp
            | NSEventMask::FlagsChanged,
        &block,
    );
    *guard = monitor.map(MainThreadMonitor);
}

fn handle_global_cursor_event(event: &NSEvent) {
    if capture_overlay_owns_cursor() {
        apply_capture_cursor_monitor_event(event);
        return;
    }
    if event.r#type() == NSEventType::FlagsChanged {
        return;
    }
    apply_thumbnail_hover_from_global_pointer();
}

fn apply_thumbnail_hover_from_global_pointer() {
    if !thumbnail_is_presented() || capture_overlay_owns_cursor() {
        return;
    }
    let Some(window) = thumbnail_ns_window() else {
        return;
    };
    if !point_in_ns_rect(NSEvent::mouseLocation(), window.frame()) {
        return;
    }
    if !thumbnail_pointer_poll_is_live() {
        // Frozen JS poll can leave the stack click-through. Tracking areas do
        // not fire on a window that ignores mouse events, so enable them before
        // hover.
        window.setIgnoresMouseEvents(false);
        THUMBNAIL_KEY_WINDOW_ALLOWED.store(true, Ordering::Release);
        if cursor_surface_can_take_key_window(CursorSurface::Thumbnail) && !window.isKeyWindow() {
            remember_frontmost_app_before_thumbnail_key();
            window.makeKeyWindow();
        }
        let _ = apply_unpolled_thumbnail_hover_cursor();
    }
}

fn ensure_thumbnail_resign_active_observer() {
    debug_assert!(is_main_thread());
    THUMBNAIL_RESIGN_ACTIVE_OBSERVER.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        let block = RcBlock::new(|_notification: ptr::NonNull<NSNotification>| {
            THUMBNAIL_KEY_WINDOW_ALLOWED.store(true, Ordering::Release);
            apply_thumbnail_hover_from_global_pointer();
        });
        let center = NSNotificationCenter::defaultCenter();
        let queue = NSOperationQueue::mainQueue();
        // SAFETY: The notification name matches NSApplication's resign-active
        // notification, and the main queue serializes AppKit cursor state.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidResignActiveNotification),
                None,
                Some(&queue),
                &block,
            )
        };
        *slot = Some(observer);
    });
}

fn capture_overlay_is_key() -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(main_thread)
        .keyWindow()
        .is_some_and(|window| {
            cursor_surface_for_window(&window) == Some(CursorSurface::CaptureOverlay)
        })
}

fn screen_frame_containing_mouse(mtm: MainThreadMarker) -> NSRect {
    let mouse = NSEvent::mouseLocation();
    NSScreen::screens(mtm)
        .into_iter()
        .find(|screen| point_in_ns_rect(mouse, screen.frame()))
        .map(|screen| screen.frame())
        .unwrap_or_else(|| NSRect::new(mouse, NSSize::new(1.0, 1.0)))
}

fn show_cursor_claim_panel() {
    if capture_overlay_is_key() {
        hide_cursor_claim_panel();
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let target_frame = screen_frame_containing_mouse(mtm);
    let Ok(mut guard) = CURSOR_CLAIM_PANEL.lock() else {
        return;
    };
    let panel = if let Some(MainThreadPanel(panel)) = guard.as_ref() {
        panel.clone()
    } else {
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            target_frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setHidesOnDeactivate(false);
        panel.setWorksWhenModal(true);
        panel.setLevel(capture_surface_window_level());
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        // Eat mouse events so the frontmost app cannot keep writing its cursor
        // while modifiers come up. Sharing-none keeps this surface out of the
        // freeze-frame.
        panel.setIgnoresMouseEvents(false);
        panel.setSharingType(NSWindowSharingType::None);
        panel.setAcceptsMouseMovedEvents(true);
        unsafe {
            panel.setReleasedWhenClosed(false);
        }
        panel.setCollectionBehavior(capture_surface_collection_behavior());
        panel.setAlphaValue(WINDOW_REVEAL_PRIME_ALPHA);
        *guard = Some(MainThreadPanel(panel.clone()));
        panel
    };
    drop(guard);
    let already_ready = panel.isVisible()
        && panel.isKeyWindow()
        && point_in_ns_rect(NSEvent::mouseLocation(), panel.frame());
    if already_ready {
        return;
    }
    panel.setFrame_display(target_frame, false);
    panel.orderFrontRegardless();
    panel.makeKeyWindow();
}

fn hide_cursor_claim_panel() {
    let Ok(guard) = CURSOR_CLAIM_PANEL.lock() else {
        return;
    };
    if let Some(MainThreadPanel(panel)) = guard.as_ref() {
        if !panel.isVisible() {
            return;
        }
        if panel.isKeyWindow() {
            panel.resignKeyWindow();
        }
        panel.orderOut(None);
    }
}

/// Claims the capture cursor before the overlay paints or shortcut modifiers
/// come up. A nonactivating panel covering the display under the pointer can
/// be key while another app stays frontmost, so `NSCursor.set` is not ignored
/// and does not wait for a move. Window sharing is disabled so the panel is
/// omitted from the freeze-frame.
///
/// CSS-owned surfaces (the capture menu, window capture) skip the native seed.
/// Seeding a crosshair or arrow there races panel grab/pointer on every
/// reassert, including mouse-move monitors.
pub fn claim_capture_cursor(cursor: CaptureCursor) {
    if !is_main_thread() {
        let _ = run_on_main(move || claim_capture_cursor(cursor));
        return;
    }
    CAPTURE_OVERLAY_OWNS_CURSOR.store(true, Ordering::Release);
    store_capture_cursor(cursor);
    ensure_capture_cursor_monitor();
    ensure_global_cursor_monitor();
    NSCursor::setHiddenUntilMouseMoves(false);
    if cursor.native_owned {
        show_cursor_claim_panel();
        apply_cursor_mode(cursor.kind.to_cursor_mode());
        DispatchQueue::main().exec_async(|| {
            reassert_claimed_capture_cursor();
            DispatchQueue::main().exec_async(reassert_claimed_capture_cursor);
        });
    }
}

fn reassert_claimed_capture_cursor() {
    if !capture_overlay_owns_cursor() {
        return;
    }
    let cursor = stored_capture_cursor();
    if cursor.reasserts_native_cursor_on_modifiers() {
        show_cursor_claim_panel();
        apply_cursor_mode(cursor.kind.to_cursor_mode());
    }
}

// The address of this byte is used as the Objective-C association key.
static CURSOR_TRACKER_ASSOCIATION_KEY: u8 = 0;
// Associates the same tracker with its NSWindow so the click monitor can tell
// a nonactivating thumbnail panel from recording HUD and document windows.
static CURSOR_TRACKER_WINDOW_ASSOCIATION_KEY: u8 = 0;
// NSCursor is application-wide, so a hidden preview must not replace the
// cursor selected by the active capture overlay.
static CAPTURE_OVERLAY_OWNS_CURSOR: AtomicBool = AtomicBool::new(false);
static CAPTURE_CURSOR_KIND: AtomicU8 = AtomicU8::new(0);
static CAPTURE_CURSOR_NATIVE_OWNED: AtomicBool = AtomicBool::new(false);
static CAPTURE_CURSOR_MONITOR: Mutex<Option<MainThreadMonitor>> = Mutex::new(None);
// When a transient capture surface activates Captures (region/window overlay,
// recording selector, countdown), sibling document windows such as the
// screenshot editor are ordered front with the app. Remember the user's
// previous frontmost app so we can hand focus back after the surface dismisses.
// Order those documents out only after the capture surface is opaque.
static FRONTMOST_APP_BEFORE_CAPTURE: Mutex<Option<Retained<NSRunningApplication>>> =
    Mutex::new(None);
// The interactive update notice activates Captures so its buttons and keyboard
// focus work. Keep its activation handoff separate from capture overlays: when
// the notice hides, AppKit otherwise donates key status to an open editor and
// raises that editor over the app the user was working in.
static FRONTMOST_APP_BEFORE_UPDATE_NOTICE: Mutex<Option<Retained<NSRunningApplication>>> =
    Mutex::new(None);
// `Some(None)` means the update notice most recently yielded to another
// Captures window; the nested option distinguishes that from no focus handoff.
static PENDING_UPDATE_NOTICE_REFOCUS_TARGET: Mutex<Option<Option<Retained<NSRunningApplication>>>> =
    Mutex::new(None);
// A thumbnail click can make Captures active before its handler hides the
// panel. Preserve the external app while hover first gives the panel key status
// so AppKit cannot donate focus to an open editor during collapse.
static FRONTMOST_APP_BEFORE_THUMBNAIL_KEY: Mutex<Option<Retained<NSRunningApplication>>> =
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
    // When documents were ordered out because another app was frontmost, keep
    // that app so reveal can hand activation back after `orderFront` without
    // lifting preferences/history/feedback above the user's work.
    static CONCEALED_DOCUMENT_REVEAL_YIELD_TO: RefCell<Option<Retained<NSRunningApplication>>> =
        const { RefCell::new(None) };
    // NSWorkspace retains the block-backed observer, while this token keeps the
    // registration discoverable and prevents duplicate observers.
    static UPDATE_NOTICE_WORKSPACE_OBSERVER:
        RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
            const { RefCell::new(None) };
    static THUMBNAIL_RESIGN_ACTIVE_OBSERVER:
        RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
            const { RefCell::new(None) };
    static SUPPRESSED_CURSOR_RECT_WINDOW: RefCell<Option<Retained<NSWindow>>> =
        const { RefCell::new(None) };
}

/// Returns whether a standard shortcut modifier is still physically held.
///
/// A registered macOS hotkey reports its primary key release before users
/// necessarily release its modifiers. Starting region capture during that gap
/// lets AppKit replace the crosshair with an arrow when the modifiers come up.
pub fn capture_shortcut_modifiers_pressed() -> bool {
    if !is_main_thread() {
        return run_on_main(capture_shortcut_modifiers_pressed).unwrap_or(false);
    }
    shortcut_modifiers_pressed(NSEvent::modifierFlags_class())
}

/// Returns the system pasteboard revision without reading its contents.
///
/// AppKit increments this value whenever any application replaces the
/// pasteboard, allowing Captures to notice that its last copied capture is no
/// longer current without inspecting the user's clipboard data.
pub fn clipboard_change_count() -> isize {
    if !is_main_thread() {
        return run_on_main(clipboard_change_count).unwrap_or(0);
    }
    NSPasteboard::generalPasteboard().changeCount()
}

/// Plays a short, low-volume system sound after a capture is confirmed.
pub fn play_capture_sound() -> Result<(), &'static str> {
    if !is_main_thread() {
        return run_on_main(play_capture_sound)
            .ok_or("macOS capture sound did not run on the main thread")?;
    }
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

/// Configures an inactive HUD whose entire surface is one pointing-hand target.
///
/// Unlike WebKit cursor rectangles, the native tracker applies on first entry
/// even while a browser or another application remains frontmost.
pub fn configure_pointing_inactive_hover(window: &WebviewWindow) -> Result<(), &'static str> {
    configure_inactive_hover_with_cursor::<InteractiveHudPanel>(
        window,
        CursorMode::PointingHand,
        CursorSurface::InactiveHud,
    )
}

/// Configures the mini-preview stack as a nonactivating mouse-interactive panel.
/// It becomes key only while the pointer is over live preview chrome so AppKit
/// can display its cursor, then releases key status after click delivery and on
/// exit so the application underneath stays active.
pub fn configure_thumbnail_inactive_hover(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || configure_thumbnail_inactive_hover(&window))
            .ok_or("inactive HUD setup did not run on the main thread")?;
    }
    THUMBNAIL_PRESENTED.store(false, Ordering::Release);
    let result = configure_inactive_hover_with_cursor::<ThumbnailPanel>(
        window,
        CursorMode::WebView,
        CursorSurface::Thumbnail,
    );
    if result.is_ok() {
        ensure_global_cursor_monitor();
        ensure_thumbnail_resign_active_observer();
        ensure_thumbnail_click_cursor_monitor();
        if let Ok(native) = native_window(window) {
            remember_thumbnail_window(native);
        }
    }
    result
}

fn configure_inactive_hover_with_cursor<P>(
    window: &WebviewWindow,
    initial_cursor: CursorMode,
    surface: CursorSurface,
) -> Result<(), &'static str>
where
    P: tauri_nspanel::FromWindow<tauri::Wry> + 'static,
{
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || {
            configure_inactive_hover_with_cursor::<P>(&window, initial_cursor, surface)
        })
        .ok_or("inactive HUD setup did not run on the main thread")?;
    }
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
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || show_without_activating(&window))
            .ok_or("window reveal did not run on the main thread")?;
    }
    let _main_thread =
        MainThreadMarker::new().ok_or("window reveal must run on the main thread")?;
    let native_window = native_window(window)?;
    native_window.setLevel(NSStatusWindowLevel);
    native_window.orderFrontRegardless();
    Ok(())
}

/// Reveals the mini-preview panel without activating Captures.
pub fn show_thumbnail_without_activating(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || show_thumbnail_without_activating(&window))
            .ok_or("thumbnail reveal did not run on the main thread")?;
    }
    native_window(window)?.setAlphaValue(1.0);
    show_without_activating(window)?;
    THUMBNAIL_PRESENTED.store(true, Ordering::Release);
    THUMBNAIL_KEY_WINDOW_ALLOWED.store(true, Ordering::Release);
    Ok(())
}

/// Makes the mini-preview panel transparent without ordering it out.
///
/// Ordering out a key-capable nonactivating panel can make AppKit donate key
/// status to an open editor and activate Captures over the user's current app.
/// Keeping the click-through panel onscreen at zero alpha avoids that focus
/// handoff; the WebView is paused separately while it has no live cards.
pub fn conceal_thumbnail_without_hiding(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || conceal_thumbnail_without_hiding(&window))
            .ok_or("thumbnail conceal did not run on the main thread")?;
    }
    THUMBNAIL_PRESENTED.store(false, Ordering::Release);
    THUMBNAIL_KEY_WINDOW_ALLOWED.store(false, Ordering::Release);
    restore_competing_cursor_rects();
    let native_window = native_window(window)?;
    native_window.setAlphaValue(0.0);
    set_cursor_rects_enabled(
        native_window,
        !thumbnail_passthrough_disables_cursor_rects(),
    );
    set_tracked_cursor(window, CursorMode::WebView, CursorSurface::Thumbnail)?;
    resign_ns_window_key_without_raising_documents(native_window);
    Ok(())
}

/// Whether the mini-preview panel is currently visible to the user.
pub fn thumbnail_is_presented() -> bool {
    THUMBNAIL_PRESENTED.load(Ordering::Acquire)
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

fn keep_content_rectangular(native_window: &NSWindow) {
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
    // The physical panel can have rounded bezel corners, but the capture image
    // and pointer coordinate space are rectangular. Clipping this layer to the
    // bezel made those extreme pixels impossible to start a region from.
    let _: () = unsafe { msg_send![&*layer, setCornerRadius: 0.0] };
    let _: () = unsafe { msg_send![&*layer, setMasksToBounds: false] };
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
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || configure_capture_overlay(&window))
            .ok_or("capture overlay setup did not run on the main thread")?;
    }
    let native = native_window(window)?;
    elevate_fullscreen_capture_window(native);
    native.setSharingType(NSWindowSharingType::None);
    clear_transparent_window_backing(native);
    clear_transparent_webview_backing(window);
    native.setAlphaValue(0.0);
    set_tracked_cursor(window, CursorMode::Arrow, CursorSurface::CaptureOverlay)
}

/// Keeps the interactive capture selector above system chrome so a selected
/// display can be outlined from physical edge to physical edge, including the
/// menu-bar strip at the top of a macOS display.
pub fn configure_capture_selector(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || configure_capture_selector(&window))
            .ok_or("capture selector setup did not run on the main thread")?;
    }
    let _main_thread =
        MainThreadMarker::new().ok_or("capture selector setup must run on the main thread")?;
    let native = native_window(window)?;
    elevate_fullscreen_capture_window(native);
    native.setSharingType(NSWindowSharingType::None);
    Ok(())
}

/// Quartz / xcap screen rectangle for a visible AppKit window.
///
/// `kCGWindowBounds` includes the drop shadow. `NSWindow.frame` is the opaque
/// chrome, which is what the window selector highlight should cover.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisibleWindowFrame {
    pub window_number: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Convert an AppKit window frame (origin at the bottom-left of the menu-bar
/// screen) into Quartz global coordinates (origin at the top-left of that
/// screen), matching `kCGWindowBounds` / xcap.
pub fn appkit_frame_to_quartz(
    ns_x: f64,
    ns_y: f64,
    ns_width: f64,
    ns_height: f64,
    primary_height: f64,
) -> (i32, i32, u32, u32) {
    let x = ns_x.round() as i32;
    let y = (primary_height - ns_y - ns_height).round() as i32;
    let width = ns_width.round().max(1.0) as u32;
    let height = ns_height.round().max(1.0) as u32;
    (x, y, width, height)
}

/// Opaque frames for this process's visible windows, keyed by `CGWindowNumber`.
///
/// Used to replace shadow-inflated xcap bounds so the window selector overlay
/// matches the on-screen chrome of Preferences, editors, and other Captures
/// documents — and any other app whose windows we own.
pub fn visible_window_frames() -> Vec<VisibleWindowFrame> {
    if !is_main_thread() {
        return run_on_main(visible_window_frames).unwrap_or_default();
    }
    visible_window_frames_on_main()
}

fn visible_window_frames_on_main() -> Vec<VisibleWindowFrame> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let Some(primary) = NSScreen::screens(mtm).into_iter().next() else {
        return Vec::new();
    };
    let primary_height = primary.frame().size.height;
    let app = NSApplication::sharedApplication(mtm);
    app.windows()
        .iter()
        .filter_map(|window| {
            if !window.isVisible() {
                return None;
            }
            let number = window.windowNumber();
            if number <= 0 {
                return None;
            }
            let frame = window.frame();
            let (x, y, width, height) = appkit_frame_to_quartz(
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
                primary_height,
            );
            Some(VisibleWindowFrame {
                window_number: number as u32,
                x,
                y,
                width,
                height,
            })
        })
        .collect()
}

/// Include or omit this window from screenshots and recordings.
///
/// Tauri's `set_content_protected` maps to `NSWindowSharingType`, but
/// converting a window to an `NSPanel` can drop that setting. Set it on the
/// live AppKit object after panel configuration.
pub fn set_excluded_from_capture(
    window: &WebviewWindow,
    excluded: bool,
) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || set_excluded_from_capture(&window, excluded))
            .ok_or("capture sharing did not run on the main thread")?;
    }
    native_window(window)?.setSharingType(if excluded {
        NSWindowSharingType::None
    } else {
        NSWindowSharingType::ReadOnly
    });
    Ok(())
}

/// Re-asserts the menu-bar-covering window level after Tauri show/focus.
pub fn elevate_capture_surface(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || elevate_capture_surface(&window))
            .ok_or("capture surface elevation did not run on the main thread")?;
    }
    elevate_fullscreen_capture_window(native_window(window)?);
    Ok(())
}

/// Pins a fullscreen capture surface to the physical display, including the
/// menu bar, while keeping its rectangular capture and pointer coordinate space.
pub fn cover_display(window: &WebviewWindow, display_id: &str) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        let display_id = display_id.to_owned();
        return run_on_main(move || cover_display(&window, &display_id))
            .ok_or("fullscreen capture coverage did not run on the main thread")?;
    }
    let mtm =
        MainThreadMarker::new().ok_or("fullscreen capture coverage must run on the main thread")?;
    let native = native_window(window)?;
    elevate_fullscreen_capture_window(native);
    if let Some(screen) = screen_for_display_id(mtm, display_id) {
        native.setFrame_display(screen.frame(), true);
        keep_content_rectangular(native);
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
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || prepare_capture_overlay(&window))
            .ok_or("capture overlay prepare did not run on the main thread")?;
    }
    let native_window = native_window(window)?;
    elevate_fullscreen_capture_window(native_window);
    clear_transparent_window_backing(native_window);
    clear_transparent_webview_backing(window);
    prime_window_reveal(window)?;
    if overlay_prepare_keeps_native_cursor(stored_capture_cursor().native_owned)
        && capture_overlay_owns_cursor()
    {
        let cursor = stored_capture_cursor();
        set_cursor_rects_enabled(native_window, false);
        native_window.discardCursorRects();
        set_tracked_cursor(
            window,
            cursor.tracked_kind().to_cursor_mode(),
            CursorSurface::CaptureOverlay,
        )?;
        apply_cursor_mode(cursor.kind.to_cursor_mode());
    } else {
        set_cursor_rects_enabled(native_window, true);
        set_tracked_cursor(window, CursorMode::WebView, CursorSurface::CaptureOverlay)?;
    }
    Ok(())
}

/// Orders the primed overlay onscreen without making it key.
///
/// Tauri's `show()` uses `makeKeyAndOrderFront:`, which focuses the overlay
/// before the snapshot has painted and can flash a black WKWebView surface.
pub fn present_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || present_capture_overlay(&window))
            .ok_or("capture overlay present did not run on the main thread")?;
    }
    let native = native_window(window)?;
    // Session delivery and snapshot onLoad can both ask to wake the same
    // overlay. Re-preparing an already-present WKWebView clears and reattaches
    // its backing layer immediately before reveal, which can produce a one-frame
    // scale snap. Only the hidden -> visible edge needs native preparation.
    if !capture_overlay_needs_presentation(native.isVisible()) {
        return Ok(());
    }
    prepare_capture_overlay(window)?;
    native.orderFront(None);
    if capture_overlay_owns_cursor() {
        reassert_claimed_capture_cursor();
    }
    Ok(())
}

fn capture_overlay_needs_presentation(is_visible: bool) -> bool {
    !is_visible
}

/// Applies the capture cursor after the overlay becomes the key window.
///
/// AppKit does not send mouseEntered/cursorUpdate when a fullscreen surface
/// appears under a stationary pointer, and becoming key or releasing shortcut
/// modifiers can restore the arrow afterwards. Set the cursor immediately, then
/// re-assert on the next two main-queue turns and on flags-changed.
pub fn activate_capture_cursor(
    window: &WebviewWindow,
    cursor: CaptureCursor,
) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || activate_capture_cursor(&window, cursor))
            .ok_or("capture cursor did not run on the main thread")?;
    }
    claim_capture_cursor(cursor);
    apply_capture_cursor(window, cursor)?;
    if native_window(window)?.isKeyWindow() {
        hide_cursor_claim_panel();
        apply_capture_cursor(window, cursor)?;
    }
    let window = window.clone();
    DispatchQueue::main().exec_async(move || {
        reassert_stored_capture_cursor(&window);
        let window = window.clone();
        DispatchQueue::main().exec_async(move || {
            reassert_stored_capture_cursor(&window);
        });
    });
    Ok(())
}

fn store_capture_cursor(cursor: CaptureCursor) {
    CAPTURE_CURSOR_KIND.store(cursor.kind as u8, Ordering::Release);
    CAPTURE_CURSOR_NATIVE_OWNED.store(cursor.native_owned, Ordering::Release);
}

fn stored_capture_cursor() -> CaptureCursor {
    CaptureCursor {
        kind: match CAPTURE_CURSOR_KIND.load(Ordering::Acquire) {
            0 => CaptureCursorKind::Arrow,
            2 => CaptureCursorKind::WebView,
            _ => CaptureCursorKind::Crosshair,
        },
        native_owned: CAPTURE_CURSOR_NATIVE_OWNED.load(Ordering::Acquire),
    }
}

fn reassert_stored_capture_cursor(window: &WebviewWindow) {
    if !capture_overlay_owns_cursor() {
        return;
    }
    let _ = apply_capture_cursor(window, stored_capture_cursor());
    if native_window(window).is_ok_and(|native| native.isKeyWindow()) {
        hide_cursor_claim_panel();
        let _ = apply_capture_cursor(window, stored_capture_cursor());
    } else {
        reassert_claimed_capture_cursor();
    }
}

fn apply_capture_cursor(window: &WebviewWindow, cursor: CaptureCursor) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    let mode = cursor.kind.to_cursor_mode();
    let tracked_mode = cursor.tracked_kind().to_cursor_mode();
    NSCursor::setHiddenUntilMouseMoves(false);
    // Native-owned overlays apply NSCursor even if the WKWebView tracker is
    // not ready yet, so a stationary pointer keeps the crosshair.
    if cursor.disables_cursor_rects() {
        set_cursor_rects_enabled(native_window, false);
        native_window.discardCursorRects();
        apply_cursor_mode(mode);
        let _ = set_tracked_cursor(window, tracked_mode, CursorSurface::CaptureOverlay);
        synthesize_cursor_update(native_window);
        // Becoming key / cursorUpdate can re-enable WebKit rectangles. Re-assert
        // the native cursor before returning so a stationary pointer keeps it.
        set_cursor_rects_enabled(native_window, false);
        apply_cursor_mode(mode);
    } else {
        // Window capture and the capture menu keep CSS cursors (camera cursor,
        // panel grab/pointer). Do not seed NSCursor here: a native crosshair or
        // arrow races those rectangles until the next WebKit evaluation.
        set_cursor_rects_enabled(native_window, true);
        // WebKit owns the root target cursor plus panel grab/pointer cursors.
        // A native per-move tracker would race those cursor rectangles.
        let _ = set_tracked_cursor(window, tracked_mode, CursorSurface::CaptureOverlay);
        refresh_webkit_cursor_rects(native_window);
    }
    Ok(())
}

fn refresh_webkit_cursor_rects(native_window: &NSWindow) {
    set_cursor_rects_enabled(native_window, true);
    native_window.resetCursorRects();
    if let Some(view) = native_window.contentView() {
        native_window.invalidateCursorRectsForView(&view);
    }
    synthesize_cursor_update(native_window);
}

fn synthesize_cursor_update(native_window: &NSWindow) {
    let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::MouseMoved,
        native_window.mouseLocationOutsideOfEventStream(),
        objc2_app_kit::NSEventModifierFlags::empty(),
        NSProcessInfo::processInfo().systemUptime(),
        native_window.windowNumber(),
        None,
        0,
        0,
        0.0_f32,
    ) else {
        return;
    };
    native_window.cursorUpdate(&event);
    if let Some(view) = native_window.contentView() {
        view.cursorUpdate(&event);
    }
}

fn ensure_capture_cursor_monitor() {
    let Ok(mut guard) = CAPTURE_CURSOR_MONITOR.lock() else {
        return;
    };
    if guard.is_some() {
        return;
    }
    // SAFETY: The block only reads process-local atomics and touches NSCursor /
    // NSWindow on the main AppKit thread (local monitors run there). Returning
    // the event pointer unchanged leaves delivery intact.
    let block = RcBlock::new(|event: ptr::NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit supplies a live NSEvent for the duration of the local
        // monitor callback.
        apply_capture_cursor_monitor_event(unsafe { event.as_ref() });
        event.as_ptr()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::FlagsChanged | NSEventMask::MouseMoved,
            &block,
        )
    };
    *guard = monitor.map(MainThreadMonitor);
}

fn apply_capture_cursor_monitor_event(event: &NSEvent) {
    if !capture_overlay_owns_cursor() {
        return;
    }
    let event_kind = match event.r#type() {
        NSEventType::FlagsChanged => CaptureCursorEvent::FlagsChanged,
        NSEventType::MouseMoved => CaptureCursorEvent::MouseMoved,
        _ => return,
    };
    match capture_cursor_monitor_action(event_kind, stored_capture_cursor()) {
        CaptureCursorMonitorAction::Ignore => {}
        CaptureCursorMonitorAction::ReassertNative => {
            show_cursor_claim_panel();
            apply_cursor_mode(stored_capture_cursor().kind.to_cursor_mode());
        }
        CaptureCursorMonitorAction::RefreshWebKitRects => {
            // Selector and window capture keep CSS cursors. Rebuild rectangles
            // only when modifiers change so releasing ⌘⇧ does not leave a stuck
            // arrow. Doing this on MouseMoved races panel grab/pointer with the
            // default arrow on every pixel.
            let Some(main_thread) = MainThreadMarker::new() else {
                return;
            };
            if let Some(window) = NSApplication::sharedApplication(main_thread).keyWindow() {
                refresh_webkit_cursor_rects(&window);
            }
        }
    }
}

/// Drops capture cursor ownership without requiring the overlay window.
///
/// The capture menu is destroyed rather than reused, so hide/reset may not run
/// on a live `WebviewWindow`.
pub fn release_capture_cursor() {
    if !is_main_thread() {
        let _ = run_on_main(release_capture_cursor);
        return;
    }
    CAPTURE_OVERLAY_OWNS_CURSOR.store(false, Ordering::Release);
    hide_cursor_claim_panel();
    NSCursor::setHiddenUntilMouseMoves(false);
    NSCursor::arrowCursor().set();
}

/// Reveals the overlay after WebKit has painted its reset state.
///
/// Makes the frozen frame fully opaque first, then orders out titled documents
/// so an open editor cannot vanish for a frame while this surface is still
/// transparent.
pub fn reveal_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    reveal_window(window)?;
    conceal_documents_under_opaque_capture_surface();
    Ok(())
}

/// Makes a window transparent while keeping it onscreen so WebKit can paint.
pub fn prepare_window_reveal(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || prepare_window_reveal(&window))
            .ok_or("window reveal prepare did not run on the main thread")?;
    }
    native_window(window)?.setAlphaValue(0.0);
    Ok(())
}

/// Keeps a hidden WebView awake without visibly exposing its cached surface.
///
/// AppKit can suspend a fully transparent WKWebView. A tiny non-zero alpha is
/// enough to let it paint the next frame while remaining imperceptible until
/// `reveal_window` makes the finished surface visible.
pub fn prime_window_reveal(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || prime_window_reveal(&window))
            .ok_or("window reveal prime did not run on the main thread")?;
    }
    native_window(window)?.setAlphaValue(WINDOW_REVEAL_PRIME_ALPHA);
    Ok(())
}

/// Reveals a window after its WebKit surface has painted.
pub fn reveal_window(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || reveal_window(&window))
            .ok_or("window reveal did not run on the main thread")?;
    }
    native_window(window)?.setAlphaValue(1.0);
    Ok(())
}

/// Activates an accessory app window and makes it key so keyboard cancellation
/// works even when the selector was launched while another app was frontmost.
/// Re-asserts on the next main-queue turn because AppKit activation is
/// asynchronous and can otherwise leave the newly revealed capture surface
/// visible but unable to receive Escape.
pub fn focus_window(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || focus_window(&window))
            .ok_or("window focus did not run on the main thread")?;
    }
    remember_frontmost_app_before_activation();
    // Countdown / recording selector callers show the covering surface first.
    conceal_documents_under_opaque_capture_surface();
    make_key_and_activate(window)?;
    let window = window.clone();
    DispatchQueue::main().exec_async(move || {
        // Escape can hide the surface before this queued retry runs. Never
        // order a cancelled overlay or selector back onscreen.
        if window.is_visible().unwrap_or(false) {
            let _ = make_key_and_activate(&window);
        }
    });
    Ok(())
}

/// Activates Captures and makes a document window key without recording a
/// capture frontmost-app anchor.
///
/// Editors and other intentional document surfaces call this after an Edit
/// click in the nonactivating thumbnail panel. Re-asserts on the next main-queue turn
/// so WebKit's asynchronous window creation cannot leave the document surface
/// visible but inactive.
///
/// Activation must not raise sibling document windows. `NSApplication.activate()`
/// and Tauri `set_focus` (which calls `activateIgnoringOtherApps:`) bring every
/// Captures window forward, so opening a second editor would also lift the first
/// over the user's other apps.
pub fn activate_document_window(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || activate_document_window(&window))
            .ok_or("window activation did not run on the main thread")?;
    }
    make_key_and_activate(window)?;
    let window = window.clone();
    DispatchQueue::main().exec_async(move || {
        let _ = make_key_and_activate(&window);
    });
    Ok(())
}

/// Remembers the external app that an interactive update notice is about to
/// cover. Call only when the notice is taking focus; status refreshes while it
/// is already key must preserve the original handoff target.
pub fn remember_frontmost_app_before_update_notice_activation() {
    if !is_main_thread() {
        let _ = run_on_main(remember_frontmost_app_before_update_notice_activation);
        return;
    }
    ensure_update_notice_workspace_observer();
    let previous = current_frontmost_if_not_captures();
    let mut pending = PENDING_UPDATE_NOTICE_REFOCUS_TARGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *pending = None;
    drop(pending);
    let mut slot = FRONTMOST_APP_BEFORE_UPDATE_NOTICE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = previous;
}

fn ensure_update_notice_workspace_observer() {
    debug_assert!(is_main_thread());
    UPDATE_NOTICE_WORKSPACE_OBSERVER.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        let block = RcBlock::new(|_notification: ptr::NonNull<NSNotification>| {
            refresh_update_notice_target_after_workspace_activation();
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let queue = NSOperationQueue::mainQueue();
        // SAFETY: The notification name and object types match NSWorkspace's
        // activation notification, and the main operation queue serializes the
        // callback with the AppKit-only handoff state below.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                Some(&queue),
                &block,
            )
        };
        *slot = Some(observer);
    });
}

fn refresh_update_notice_target_after_workspace_activation() {
    debug_assert!(is_main_thread());
    let current = current_frontmost_if_not_captures();
    let mut pending = PENDING_UPDATE_NOTICE_REFOCUS_TARGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    refresh_notice_activation_source_while_unfocused(&mut pending, current);
}

/// Refreshes the handoff target when a visible update notice loses and later
/// regains focus. A status refresh is not involved in that user-driven path,
/// so the source recorded when the notice was first shown is otherwise stale.
pub fn update_notice_focus_changed(is_focused: bool) {
    if !is_main_thread() {
        let _ = run_on_main(move || update_notice_focus_changed(is_focused));
        return;
    }

    if !is_focused {
        // AppKit can publish the window event just before NSWorkspace switches
        // its frontmost application. Sample on the next main-queue turn.
        DispatchQueue::main().exec_async(|| {
            let current = current_frontmost_if_not_captures();
            let mut pending = PENDING_UPDATE_NOTICE_REFOCUS_TARGET
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = notice_activation_source_after_focus_change(false, &mut pending, current);
        });
        return;
    }

    let current = current_frontmost_if_not_captures();
    let next = {
        let mut pending = PENDING_UPDATE_NOTICE_REFOCUS_TARGET
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        notice_activation_source_after_focus_change(true, &mut pending, current)
    };
    if let Some(next) = next {
        let mut slot = FRONTMOST_APP_BEFORE_UPDATE_NOTICE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = next;
    }
}

fn notice_activation_source_after_focus_change<T>(
    is_focused: bool,
    pending: &mut Option<Option<T>>,
    current_external: Option<T>,
) -> Option<Option<T>> {
    if !is_focused {
        *pending = Some(current_external);
        return None;
    }
    let previous = pending.take()?;
    Some(current_external.or(previous))
}

fn refresh_notice_activation_source_while_unfocused<T>(
    pending: &mut Option<Option<T>>,
    current_external: Option<T>,
) {
    if pending.is_some()
        && let Some(current_external) = current_external
    {
        *pending = Some(Some(current_external));
    }
}

/// Hides the interactive update notice and hands activation back to the app it
/// covered. AppKit may make an open editor key as the notice disappears, so
/// resign that donated key window before yielding to the external app.
pub fn dismiss_update_notice(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || dismiss_update_notice(&window))
            .ok_or("update notice dismissal did not run on the main thread")?;
    }

    let native = native_window(window)?;
    let previous = {
        let mut slot = FRONTMOST_APP_BEFORE_UPDATE_NOTICE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.take()
    };
    if native.isKeyWindow() {
        native.resignKeyWindow();
    }
    if previous.is_some()
        && let Some(main_thread) = MainThreadMarker::new()
    {
        let app = NSApplication::sharedApplication(main_thread);
        if let Some(key) = app.keyWindow()
            && is_titled_document_window(&key)
        {
            key.resignKeyWindow();
        }
    }
    if window.hide().is_err() {
        let mut slot = FRONTMOST_APP_BEFORE_UPDATE_NOTICE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = previous;
        native.makeKeyWindow();
        return Err("failed to hide update notice");
    }
    yield_activation_to(previous);
    Ok(())
}

/// Flags that activate Captures without `ActivateAllWindows`.
///
/// AppKit then brings only the key and main windows forward. Callers must make
/// the target both key and main first so a previously focused editor stays put.
pub(crate) fn single_window_activation_options() -> NSApplicationActivationOptions {
    // Deprecated and ignored on macOS 14+, but still required on earlier
    // systems to steal key from another app after a nonactivating panel click.
    #[allow(deprecated)]
    {
        NSApplicationActivationOptions::ActivateIgnoringOtherApps
    }
}

fn make_key_and_activate(window: &WebviewWindow) -> Result<(), &'static str> {
    MainThreadMarker::new().ok_or("window focus must run on the main thread")?;
    let native = native_window(window)?;
    // Become main before activation so “key + main only” cannot also raise the
    // last focused editor. `orderFrontRegardless` lifts this one window above
    // other apps while Captures is still inactive.
    native.makeMainWindow();
    native.makeKeyWindow();
    native.orderFrontRegardless();
    let _ = NSRunningApplication::currentApplication()
        .activateWithOptions(single_window_activation_options());
    Ok(())
}

/// Records the frontmost app before a transient Captures surface steals
/// activation. No-op when Captures is already frontmost, or when a capture
/// session already recorded an anchor (selector → countdown should not clobber
/// the original frontmost app).
///
/// Does **not** order out titled documents. Call
/// [`conceal_documents_under_opaque_capture_surface`] after the overlay,
/// selector, or countdown is opaque so an open editor cannot blink off while
/// the surface is still at prime alpha. When Captures already holds focus —
/// the usual case with an editor open — documents stay visible under the
/// always-on-top overlay for the whole capture UI session.
///
/// Call [`reveal_concealed_document_windows`] only when the full capture UI
/// session ends — not on intermediate frontmost restores (for example overlay →
/// countdown).
pub fn remember_frontmost_app_before_activation() {
    if !is_main_thread() {
        let _ = run_on_main(remember_frontmost_app_before_activation);
        return;
    }
    {
        let slot = FRONTMOST_APP_BEFORE_CAPTURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
    }
    let previous = current_frontmost_if_not_captures();
    let mut slot = FRONTMOST_APP_BEFORE_CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A concurrent activation may have recorded the anchor first; keep it.
    if slot.is_some() {
        return;
    }
    *slot = previous;
}

/// Orders out titled documents once a capture surface is already opaque.
///
/// [`remember_frontmost_app_before_activation`] only records the previous app.
/// Ordering editors out while the overlay is still at prime alpha made them
/// vanish, then the freeze-frame (which still contains the editor) painted and
/// they appeared to pop back. Call this after `reveal_window` / opaque show,
/// and before `makeKeyAndOrderFront`, so activation cannot raise them above
/// Chrome while they stay covered on the capture display.
pub fn conceal_documents_under_opaque_capture_surface() {
    if !is_main_thread() {
        let _ = run_on_main(conceal_documents_under_opaque_capture_surface);
        return;
    }
    if !should_conceal_documents_now() {
        return;
    }
    conceal_document_windows_for_capture();
}

fn should_conceal_documents_now() -> bool {
    should_conceal_documents_for_capture_activation(
        current_frontmost_if_not_captures().is_some(),
        captures_holds_user_focus(),
    )
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
    if !is_main_thread() {
        let _ = run_on_main(restore_frontmost_app_after_capture);
        return;
    }
    let previous = {
        let mut slot = FRONTMOST_APP_BEFORE_CAPTURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.take()
    };
    yield_activation_to(previous);
}

fn current_frontmost_if_not_captures() -> Option<Retained<NSRunningApplication>> {
    if captures_holds_user_focus() {
        return None;
    }
    frontmost_app_other_than_captures()
}

fn frontmost_app_other_than_captures() -> Option<Retained<NSRunningApplication>> {
    let frontmost = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let current = NSRunningApplication::currentApplication();
    if running_apps_are_same(&frontmost, &current) || frontmost.isTerminated() {
        None
    } else {
        Some(frontmost)
    }
}

fn running_apps_are_same(left: &NSRunningApplication, right: &NSRunningApplication) -> bool {
    left.processIdentifier() == right.processIdentifier()
}

/// True when the user is already working in Captures (an editor or other
/// titled document is key, or the app is active).
///
/// NSWorkspace can still name another app as frontmost for an LSUIElement
/// agent, which previously made capture startup order out the editor and
/// immediately show it again.
fn captures_holds_user_focus() -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(main_thread);
    if app.isActive() {
        return true;
    }
    app.keyWindow()
        .is_some_and(|window| is_titled_document_window(&window))
}

fn yield_activation_to(previous: Option<Retained<NSRunningApplication>>) {
    if !is_main_thread() {
        let _ = run_on_main(move || yield_activation_to(previous));
        return;
    }
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

fn activation_handoff_target<T>(current: Option<T>, remembered: Option<T>) -> Option<T> {
    current.or(remembered)
}

fn remember_frontmost_app_before_thumbnail_key() {
    debug_assert!(
        is_main_thread(),
        "thumbnail frontmost-app capture must run on AppKit's main thread"
    );
    let previous = frontmost_app_other_than_captures();
    let mut slot = FRONTMOST_APP_BEFORE_THUMBNAIL_KEY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = previous;
}

fn remembered_frontmost_app_before_thumbnail_key() -> Option<Retained<NSRunningApplication>> {
    FRONTMOST_APP_BEFORE_THUMBNAIL_KEY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .cloned()
}

fn clear_frontmost_app_before_thumbnail_key() {
    let mut slot = FRONTMOST_APP_BEFORE_THUMBNAIL_KEY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = None;
}

fn thumbnail_activation_handoff_target() -> Option<Retained<NSRunningApplication>> {
    activation_handoff_target(
        current_frontmost_if_not_captures(),
        remembered_frontmost_app_before_thumbnail_key(),
    )
}

/// Runs `work` without leaving Captures — and an open editor — in front of the
/// user's current app.
///
/// Hiding or showing the nonactivating thumbnail panel, or resigning its key
/// status, can activate Captures and donate key status to a titled document.
/// Yield activation back afterward. Do not order out editors for this short
/// panel hop: hide-then-show is visible as a flash when an editor is already
/// on screen.
pub fn run_without_stealing_activation<F: FnOnce()>(work: F) {
    debug_assert!(
        is_main_thread(),
        "run_without_stealing_activation must run on AppKit's main thread"
    );
    let previous = thumbnail_activation_handoff_target();
    work();
    clear_frontmost_app_before_thumbnail_key();
    yield_activation_to(previous);
}

/// Resigns key on a nonactivating panel without making an open editor key.
///
/// AppKit donates key status to the next window in the app when a key panel
/// resigns. If another app is frontmost, that would activate Captures and
/// order the screenshot or recording editor above the user's work.
pub fn resign_panel_key_without_raising_documents(
    window: &WebviewWindow,
) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || resign_panel_key_without_raising_documents(&window))
            .ok_or("panel key resign did not run on the main thread")?;
    }
    resign_ns_window_key_without_raising_documents(native_window(window)?);
    Ok(())
}

fn resign_ns_window_key_without_raising_documents(window: &NSWindow) {
    if !window.isKeyWindow() {
        clear_frontmost_app_before_thumbnail_key();
        return;
    }
    let previous = thumbnail_activation_handoff_target();
    window.resignKeyWindow();
    if previous.is_some()
        && let Some(main_thread) = MainThreadMarker::new()
    {
        let app = NSApplication::sharedApplication(main_thread);
        if let Some(key) = app.keyWindow()
            && is_titled_document_window(&key)
        {
            key.resignKeyWindow();
            // Key donation can also raise the editor inside Captures' inactive
            // window stack. Put it behind the user's frontmost app before
            // returning activation so it cannot remain visually promoted.
            key.orderBack(None);
        }
    }
    clear_frontmost_app_before_thumbnail_key();
    yield_activation_to(previous);
}

/// Orders out titled document windows so capture activation cannot flash them.
///
/// Borderless capture surfaces and nonactivating HUD panels are left alone.
/// Idempotent while a concealment session is already active.
pub fn conceal_document_windows_for_capture() {
    if !is_main_thread() {
        let _ = run_on_main(conceal_document_windows_for_capture);
        return;
    }
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    CONCEALED_DOCUMENT_WINDOWS.with(|concealed| {
        let mut concealed = concealed.borrow_mut();
        if !concealed.is_empty() {
            return;
        }
        let app = NSApplication::sharedApplication(main_thread);
        let mut to_conceal = Vec::new();
        for window in app.windows().iter() {
            if !is_titled_document_window(&window) || !window.isVisible() {
                continue;
            }
            to_conceal.push(window);
        }
        if to_conceal.is_empty() {
            return;
        }
        CONCEALED_DOCUMENT_REVEAL_YIELD_TO.with(|yield_to| {
            let mut yield_to = yield_to.borrow_mut();
            if yield_to.is_none() {
                *yield_to = current_frontmost_if_not_captures();
            }
        });
        for window in to_conceal {
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
    if !is_main_thread() {
        let _ = run_on_main(reveal_concealed_document_windows);
        return;
    }
    if MainThreadMarker::new().is_none() {
        return;
    }
    let yield_to = CONCEALED_DOCUMENT_REVEAL_YIELD_TO.with(|slot| slot.borrow_mut().take());
    let windows =
        CONCEALED_DOCUMENT_WINDOWS.with(|concealed| std::mem::take(&mut *concealed.borrow_mut()));
    let keep_behind_foreign_app = yield_to.is_some();
    for window in windows {
        // Destroyed webviews drop their NSWindow; skip anything already gone or
        // already visible from another path.
        if window.isVisible() {
            continue;
        }
        // orderFront (not orderFrontRegardless) keeps an inactive Captures
        // behind the restored frontmost app instead of floating above it.
        window.orderFront(None);
        if keep_behind_foreign_app {
            // `orderFront` can still raise this document above the user's app
            // when capture teardown briefly reactivates Captures. Push it to the
            // back of Captures' stack before handing activation back.
            window.orderBack(None);
        }
    }
    yield_activation_to(yield_to);
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
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || reset_capture_overlay(&window))
            .ok_or("overlay reset did not run on the main thread")?;
    }
    let result = (|| {
        let native_window = native_window(window)?;
        native_window.setAlphaValue(0.0);
        set_cursor_rects_enabled(native_window, true);
        set_tracked_cursor(window, CursorMode::Arrow, CursorSurface::CaptureOverlay)
    })();
    release_capture_cursor();
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
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || resize_from_bottom(&window, width, height))
            .ok_or("preview resize did not run on the main thread")?;
    }
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
/// - `Default` over click-through holes (collapsed stack, padding, exiting
///   cards). That kind releases the cursor so the app underneath can show
///   pointer/I-beam hover.
pub fn set_thumbnail_cursor(
    window: &WebviewWindow,
    kind: ThumbnailCursorKind,
) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || set_thumbnail_cursor(&window, kind))
            .ok_or("thumbnail cursor did not run on the main thread")?;
    }
    if capture_overlay_owns_cursor() || !thumbnail_kind_to_hover(kind).claims_ns_cursor() {
        return reset_pointing_cursor_state(window);
    }
    let native_window = native_window(window)?;
    remember_thumbnail_window(native_window);
    let interactive = thumbnail_kind_to_hover(kind).is_interactive();
    let mode = match kind {
        ThumbnailCursorKind::Default => CursorMode::WebView,
        ThumbnailCursorKind::Pointer => CursorMode::PointingHand,
        ThumbnailCursorKind::Grab => CursorMode::OpenHand,
    };
    if should_rearm_thumbnail_key_window(interactive, mode != thumbnail_cursor_mode()) {
        THUMBNAIL_KEY_WINDOW_ALLOWED.store(true, Ordering::Release);
    }
    // A nonactivating panel can become key without activating Captures. AppKit
    // only displays this app's NSCursor while the panel is key, so take key for
    // the live card and release it again over click-through/empty space.
    if interactive
        && cursor_surface_can_take_key_window(CursorSurface::Thumbnail)
        && !native_window.isKeyWindow()
    {
        remember_frontmost_app_before_thumbnail_key();
        native_window.makeKeyWindow();
    } else if !interactive && native_window.isKeyWindow() {
        restore_competing_cursor_rects();
        resign_ns_window_key_without_raising_documents(native_window);
    }
    set_cursor_rects_enabled(native_window, !interactive);
    set_tracked_cursor(window, mode, CursorSurface::Thumbnail)?;
    apply_thumbnail_ns_cursor(kind);
    // Becoming key can re-enable rectangles asynchronously after this returns.
    // Re-disable and re-set so grab survives a stationary entry onto the image.
    if interactive {
        set_cursor_rects_enabled(native_window, false);
        apply_thumbnail_ns_cursor(kind);
        suppress_competing_cursor_rects_if_needed(mode);
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
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || reassert_thumbnail_cursor(&window, kind))
            .ok_or("thumbnail cursor reassert did not run on the main thread")?;
    }
    if matches!(kind, ThumbnailCursorKind::Default) {
        return reset_pointing_cursor_state(window);
    }
    if capture_overlay_owns_cursor() {
        return reset_pointing_cursor_state(window);
    }
    let native_window = native_window(window)?;
    if cursor_surface_can_take_key_window(CursorSurface::Thumbnail) && !native_window.isKeyWindow()
    {
        remember_frontmost_app_before_thumbnail_key();
        native_window.makeKeyWindow();
    }
    set_cursor_rects_enabled(native_window, false);
    let mode = match kind {
        ThumbnailCursorKind::Default => CursorMode::WebView,
        ThumbnailCursorKind::Pointer => CursorMode::PointingHand,
        ThumbnailCursorKind::Grab => CursorMode::OpenHand,
    };
    set_tracked_cursor(window, mode, CursorSurface::Thumbnail)?;
    apply_thumbnail_ns_cursor(kind);
    set_cursor_rects_enabled(native_window, false);
    apply_thumbnail_ns_cursor(kind);
    suppress_competing_cursor_rects_if_needed(mode);
    Ok(())
}

fn apply_thumbnail_ns_cursor(kind: ThumbnailCursorKind) {
    if !thumbnail_kind_to_hover(kind).claims_ns_cursor() {
        return;
    }
    let mode = match kind {
        ThumbnailCursorKind::Default => CursorMode::WebView,
        ThumbnailCursorKind::Pointer => CursorMode::PointingHand,
        ThumbnailCursorKind::Grab => CursorMode::OpenHand,
    };
    apply_cursor_mode(mode);
}

/// Clears the preview's stored pointing cursor without changing the cursor
/// currently owned by another window.
pub fn reset_pointing_cursor_state(window: &WebviewWindow) -> Result<(), &'static str> {
    if !is_main_thread() {
        let window = window.clone();
        return run_on_main(move || reset_pointing_cursor_state(&window))
            .ok_or("thumbnail cursor reset did not run on the main thread")?;
    }
    THUMBNAIL_KEY_WINDOW_ALLOWED.store(false, Ordering::Release);
    restore_competing_cursor_rects();
    let native_window = native_window(window)?;
    if native_window.isKeyWindow() {
        resign_ns_window_key_without_raising_documents(native_window);
    }
    // The panel stays tall after collapse. Enabling WebKit cursor rectangles
    // (or stamping the arrow) would steal hover cursors from the app that is
    // now receiving clicks through the empty region.
    set_cursor_rects_enabled(
        native_window,
        !thumbnail_passthrough_disables_cursor_rects(),
    );
    set_tracked_cursor(window, CursorMode::WebView, CursorSurface::Thumbnail)
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
        owner.set_view(webview);
        owner.set_mode(mode);
        return;
    }

    let owner = CursorTrackingOwner::new(mode, surface);
    owner.set_view(webview);
    let options = pointer_tracking_options(surface);
    let cursor_options = cursor_update_tracking_options(surface);
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

fn pointer_tracking_options(surface: CursorSurface) -> NSTrackingAreaOptions {
    let mut options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    if surface_assumes_pointer_inside(surface) {
        options |= NSTrackingAreaOptions::AssumeInside;
    }
    options
}

fn cursor_update_tracking_options(surface: CursorSurface) -> NSTrackingAreaOptions {
    let mut options = NSTrackingAreaOptions::CursorUpdate | NSTrackingAreaOptions::InVisibleRect;
    if matches!(
        surface,
        CursorSurface::CaptureOverlay | CursorSurface::Thumbnail
    ) {
        // Overlay and mini previews must update the cursor while another app
        // is frontmost. ActiveInKeyWindow waits for a click/focus.
        options |= NSTrackingAreaOptions::ActiveAlways;
    } else {
        options |= NSTrackingAreaOptions::ActiveInKeyWindow;
    }
    if surface_assumes_pointer_inside(surface) {
        options |= NSTrackingAreaOptions::AssumeInside;
    }
    options
}

fn surface_assumes_pointer_inside(surface: CursorSurface) -> bool {
    // A fullscreen capture surface is created under the existing pointer, so
    // AppKit will not send mouseEntered until the mouse moves unless we treat
    // the pointer as already inside the tracking area.
    surface == CursorSurface::CaptureOverlay
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
    app_is_active: bool,
) -> bool {
    cursor_surface_uses_key_window(surface)
        && (surface != CursorSurface::Thumbnail
            || thumbnail_may_take_key_window(thumbnail_allowed, app_is_active))
}

fn cursor_surface_can_take_key_window(surface: CursorSurface) -> bool {
    cursor_surface_can_take_key_window_with_thumbnail_allowed(
        surface,
        THUMBNAIL_KEY_WINDOW_ALLOWED.load(Ordering::Acquire),
        app_is_active(),
    )
}

fn should_reset_cursor_on_exit(surface: CursorSurface, capture_active: bool) -> bool {
    if capture_active {
        return false;
    }
    match surface {
        CursorSurface::CaptureOverlay => false,
        CursorSurface::Thumbnail => thumbnail_resets_cursor_on_exit(),
        CursorSurface::InactiveHud => true,
    }
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
        NSBezierPath, NSEventModifierFlags, NSEventType, NSMainMenuWindowLevel,
        NSTrackingAreaOptions, NSWindowStyleMask,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

    use super::{
        CAPTURE_OVERLAY_OWNS_CURSOR, CursorMode, CursorSurface, THUMBNAIL_CURSOR_MODE,
        activation_handoff_target, appkit_frame_to_quartz, apply_unpolled_thumbnail_hover_cursor,
        capture_overlay_needs_presentation, capture_surface_collection_behavior,
        capture_surface_window_level, clamp_display_corner_radius, corner_radius_from_bezel_path,
        cursor_mode_is_interactive, cursor_mode_to_thumbnail_hover, cursor_surface_can_apply,
        cursor_surface_can_take_key_window_with_thumbnail_allowed, cursor_surface_uses_key_window,
        cursor_update_tracking_options, display_corner_radius_points, is_main_thread,
        notice_activation_source_after_focus_change, parse_display_id, point_in_ns_rect,
        pointer_tracking_options, reassert_thumbnail_cursor_after_click,
        refresh_notice_activation_source_while_unfocused, shortcut_modifiers_pressed,
        should_rearm_thumbnail_key_window, should_release_thumbnail_key_after_event,
        should_reset_cursor_on_exit, single_window_activation_options,
        style_mask_is_titled_document, surface_assumes_pointer_inside,
        thumbnail_hover_to_cursor_mode, window_corner_radius_for_major_version,
    };

    #[test]
    fn single_window_activation_does_not_raise_sibling_documents() {
        let options = single_window_activation_options();
        assert!(
            !options.contains(objc2_app_kit::NSApplicationActivationOptions::ActivateAllWindows),
            "opening one editor must not lift every other Captures window over the user's apps",
        );
    }

    #[test]
    fn update_notice_refreshes_the_target_after_user_driven_refocus() {
        let mut pending = None;
        assert_eq!(
            notice_activation_source_after_focus_change(false, &mut pending, Some("browser")),
            None
        );
        assert_eq!(pending, Some(Some("browser")));
        assert_eq!(
            notice_activation_source_after_focus_change(true, &mut pending, None),
            Some(Some("browser"))
        );
        assert_eq!(pending, None);

        let mut switched_app_handoff = None;
        assert_eq!(
            notice_activation_source_after_focus_change(
                false,
                &mut switched_app_handoff,
                Some("app-a"),
            ),
            None
        );
        refresh_notice_activation_source_while_unfocused(&mut switched_app_handoff, Some("app-b"));
        refresh_notice_activation_source_while_unfocused(&mut switched_app_handoff, None);
        assert_eq!(switched_app_handoff, Some(Some("app-b")));
        assert_eq!(
            notice_activation_source_after_focus_change(true, &mut switched_app_handoff, None),
            Some(Some("app-b"))
        );

        let mut editor_handoff = None;
        assert_eq!(
            notice_activation_source_after_focus_change(false, &mut editor_handoff, None::<&str>),
            None
        );
        assert_eq!(
            notice_activation_source_after_focus_change(true, &mut editor_handoff, None),
            Some(None)
        );
    }

    #[test]
    fn thumbnail_activation_handoff_falls_back_to_the_pre_key_app() {
        assert_eq!(
            activation_handoff_target(Some("current"), Some("remembered")),
            Some("current")
        );
        assert_eq!(
            activation_handoff_target(None, Some("remembered")),
            Some("remembered")
        );
        assert_eq!(activation_handoff_target::<&str>(None, None), None);
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
    fn capture_overlay_is_only_prepared_on_the_hidden_to_visible_edge() {
        assert!(capture_overlay_needs_presentation(false));
        assert!(!capture_overlay_needs_presentation(true));
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
    fn background_threads_are_not_the_appkit_main_thread() {
        let is_main = std::thread::spawn(is_main_thread)
            .join()
            .expect("thread should join");
        assert!(!is_main);
    }

    #[test]
    fn hops_display_corner_lookup_off_the_main_thread() {
        let radius = std::thread::spawn(|| {
            let _ = is_main_thread();
            display_corner_radius_points("1")
        })
        .join()
        .expect("background AppKit hop should not panic");
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
            false,
            true
        ));
        assert!(cursor_surface_can_take_key_window_with_thumbnail_allowed(
            CursorSurface::Thumbnail,
            false,
            false
        ));
        assert!(cursor_surface_can_take_key_window_with_thumbnail_allowed(
            CursorSurface::Thumbnail,
            true,
            true
        ));
        assert!(cursor_surface_can_take_key_window_with_thumbnail_allowed(
            CursorSurface::InactiveHud,
            false,
            true
        ));
    }

    #[test]
    fn inactive_surfaces_reset_the_cursor_when_capture_is_not_active() {
        assert!(!should_reset_cursor_on_exit(
            CursorSurface::Thumbnail,
            false
        ));
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
    fn default_cursor_updates_do_not_rearm_a_concealed_thumbnail() {
        assert!(!should_rearm_thumbnail_key_window(false, true));
        assert!(!should_rearm_thumbnail_key_window(true, false));
        assert!(should_rearm_thumbnail_key_window(true, true));
    }

    #[test]
    fn click_reassert_only_runs_for_interactive_thumbnail_cursors() {
        let previous_mode = THUMBNAIL_CURSOR_MODE.swap(CursorMode::Arrow as u8, Ordering::AcqRel);
        let previous_overlay = CAPTURE_OVERLAY_OWNS_CURSOR.swap(false, Ordering::AcqRel);

        assert!(!reassert_thumbnail_cursor_after_click());
        assert!(apply_unpolled_thumbnail_hover_cursor());

        THUMBNAIL_CURSOR_MODE.store(CursorMode::PointingHand as u8, Ordering::Release);
        assert!(reassert_thumbnail_cursor_after_click());

        THUMBNAIL_CURSOR_MODE.store(CursorMode::OpenHand as u8, Ordering::Release);
        assert!(reassert_thumbnail_cursor_after_click());

        CAPTURE_OVERLAY_OWNS_CURSOR.store(true, Ordering::Release);
        assert!(!reassert_thumbnail_cursor_after_click());

        CAPTURE_OVERLAY_OWNS_CURSOR.store(previous_overlay, Ordering::Release);
        THUMBNAIL_CURSOR_MODE.store(previous_mode, Ordering::Release);
    }

    #[test]
    fn capture_overlay_tracking_assumes_the_pointer_is_already_inside() {
        assert!(surface_assumes_pointer_inside(
            CursorSurface::CaptureOverlay
        ));
        assert!(!surface_assumes_pointer_inside(CursorSurface::Thumbnail));
        assert!(!surface_assumes_pointer_inside(CursorSurface::InactiveHud));
        assert!(
            pointer_tracking_options(CursorSurface::CaptureOverlay)
                .contains(NSTrackingAreaOptions::AssumeInside)
        );
        assert!(
            cursor_update_tracking_options(CursorSurface::CaptureOverlay)
                .contains(NSTrackingAreaOptions::AssumeInside)
        );
        assert!(
            !pointer_tracking_options(CursorSurface::Thumbnail)
                .contains(NSTrackingAreaOptions::AssumeInside)
        );
        assert!(
            cursor_update_tracking_options(CursorSurface::Thumbnail)
                .contains(NSTrackingAreaOptions::ActiveAlways)
        );
        assert!(
            cursor_update_tracking_options(CursorSurface::CaptureOverlay)
                .contains(NSTrackingAreaOptions::ActiveAlways)
        );
        assert!(
            !cursor_update_tracking_options(CursorSurface::InactiveHud)
                .contains(NSTrackingAreaOptions::ActiveAlways)
        );
    }

    #[test]
    fn unpolled_thumbnail_hover_promotes_the_arrow_to_a_pointing_hand() {
        assert_eq!(
            thumbnail_hover_to_cursor_mode(
                cursor_mode_to_thumbnail_hover(CursorMode::Arrow).unpolled_hover()
            ),
            CursorMode::PointingHand
        );
        assert_eq!(
            thumbnail_hover_to_cursor_mode(
                cursor_mode_to_thumbnail_hover(CursorMode::OpenHand).unpolled_hover()
            ),
            CursorMode::OpenHand
        );
    }

    #[test]
    fn screen_point_in_window_frame_uses_half_open_edges() {
        let frame = NSRect::new(NSPoint::new(10.0, 20.0), NSSize::new(100.0, 40.0));
        assert!(point_in_ns_rect(NSPoint::new(10.0, 20.0), frame));
        assert!(point_in_ns_rect(NSPoint::new(109.9, 59.9), frame));
        assert!(!point_in_ns_rect(NSPoint::new(110.0, 20.0), frame));
        assert!(!point_in_ns_rect(NSPoint::new(10.0, 60.0), frame));
        assert!(!point_in_ns_rect(NSPoint::new(0.0, 20.0), frame));
    }

    #[test]
    fn appkit_frames_convert_to_quartz_top_left_origin() {
        assert_eq!(
            appkit_frame_to_quartz(100.0, 200.0, 640.0, 480.0, 900.0),
            (100, 220, 640, 480)
        );
        // A window sitting on the menu-bar screen's top edge.
        assert_eq!(
            appkit_frame_to_quartz(0.0, 860.0, 800.0, 40.0, 900.0),
            (0, 0, 800, 40)
        );
    }
}
