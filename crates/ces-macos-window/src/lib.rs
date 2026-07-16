#![cfg(target_os = "macos")]

use std::{
    cell::Cell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, define_class,
    ffi::{OBJC_ASSOCIATION_RETAIN_NONATOMIC, objc_getAssociatedObject, objc_setAssociatedObject},
    msg_send,
    rc::Retained,
    runtime::AnyObject,
};
use objc2_app_kit::{
    NSCursor, NSEvent, NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSRect, NSSize};
use tauri::WebviewWindow;
use tauri_nspanel::WebviewWindowExt;

mod thumbnail_panel {
    use tauri::Manager;
    use tauri_nspanel::tauri_panel;

    tauri_panel! {
        panel!(ThumbnailPanel {
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

use thumbnail_panel::ThumbnailPanel;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorMode {
    Arrow,
    Crosshair,
    PointingHand,
    WebView,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorSurface {
    CaptureOverlay,
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
            self.activate_window_if_needed(event);
            self.apply_cursor();
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.activate_window_if_needed(event);
            self.apply_cursor();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
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
            self.apply_cursor();
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
        unsafe { msg_send![super(this), init] }
    }

    fn set_mode(&self, mode: CursorMode) {
        self.ivars().mode.set(mode);
    }

    fn activate_window_if_needed(&self, event: &NSEvent) {
        if self.ivars().surface != CursorSurface::Thumbnail
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
        }
    }

    fn resign_window_if_needed(&self, event: &NSEvent) {
        if self.ivars().surface != CursorSurface::Thumbnail
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

    fn apply_cursor(&self) {
        if !cursor_surface_can_apply(self.ivars().surface, capture_overlay_owns_cursor()) {
            return;
        }
        match self.ivars().mode.get() {
            CursorMode::Arrow => NSCursor::arrowCursor().set(),
            CursorMode::Crosshair => NSCursor::crosshairCursor().set(),
            CursorMode::PointingHand => NSCursor::pointingHandCursor().set(),
            CursorMode::WebView => {}
        }
    }
}

// The address of this byte is used as the Objective-C association key.
static CURSOR_TRACKER_ASSOCIATION_KEY: u8 = 0;
// NSCursor is application-wide, so a hidden preview must not replace the
// cursor selected by the active capture overlay.
static CAPTURE_OVERLAY_OWNS_CURSOR: AtomicBool = AtomicBool::new(false);

/// Registers the panel manager used by the capture thumbnail window.
pub fn init_panel_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_nspanel::init()
}

/// Configures the capture preview as an inactive-app HUD.
///
/// Tauri enables mouse-move events on its `NSWindow`, but WebKit's own tracking
/// can still become inactive when another application is frontmost. An
/// `ActiveAlways` tracking area keeps CSS hover and pointer events alive.
pub fn configure_inactive_hover(window: &WebviewWindow) -> Result<(), &'static str> {
    let panel = window
        .to_panel::<ThumbnailPanel>()
        .map_err(|_| "failed to convert the capture preview to an NSPanel")?;
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
        .with_webview(|platform_webview| {
            let pointer = platform_webview.inner();
            // SAFETY: Tauri supplies the live WKWebView to this callback. A
            // WKWebView inherits from NSView and remains alive for the entire
            // callback.
            let Some(webview) = (unsafe { pointer.cast::<NSView>().as_ref() }) else {
                return;
            };
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
            install_cursor_tracker(webview, CursorMode::Arrow, CursorSurface::Thumbnail);
        })
        .map_err(|_| "macOS webview handle is unavailable")
}

/// Shows the preview without making CES the active application.
pub fn show_without_activating(window: &WebviewWindow) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    native_window.setLevel(NSStatusWindowLevel);
    native_window.orderFrontRegardless();
    Ok(())
}

/// Installs the capture overlay's native cursor tracker during app startup.
///
/// The overlay is created hidden and reused for every capture. Installing its
/// tracking areas here keeps the first capture on the same path as later ones,
/// instead of doing one-time AppKit setup while the overlay is being focused.
pub fn configure_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    native_window(window)?.setAcceptsMouseMovedEvents(true);
    set_tracked_cursor(window, CursorMode::Arrow, CursorSurface::CaptureOverlay)
}

/// Makes a reused capture overlay transparent before bringing it onscreen.
pub fn prepare_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    native_window.setAlphaValue(0.0);
    set_cursor_rects_enabled(native_window, true);
    set_tracked_cursor(window, CursorMode::WebView, CursorSurface::CaptureOverlay)?;
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
    native_window(window)?.setAlphaValue(1.0);
    Ok(())
}

/// Restores native overlay state after a capture ends.
pub fn reset_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    let result = (|| {
        let native_window = native_window(window)?;
        native_window.setAlphaValue(1.0);
        set_cursor_rects_enabled(native_window, true);
        set_tracked_cursor(window, CursorMode::Arrow, CursorSurface::CaptureOverlay)
    })();
    NSCursor::arrowCursor().set();
    CAPTURE_OVERLAY_OWNS_CURSOR.store(false, Ordering::Release);
    result
}

/// Resizes a visible preview stack in one AppKit frame update while preserving
/// its bottom edge.
pub fn resize_from_bottom(
    window: &WebviewWindow,
    width: f64,
    height: f64,
) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    let current = native_window.frame();
    let frame = NSRect::new(current.origin, NSSize::new(width, height));
    native_window.setFrame_display(frame, true);
    Ok(())
}

/// Updates the cursor even while another application remains frontmost.
pub fn set_pointing_cursor(window: &WebviewWindow, pointing: bool) -> Result<(), &'static str> {
    if capture_overlay_owns_cursor() {
        return reset_pointing_cursor_state(window);
    }
    let native_window = native_window(window)?;
    set_cursor_rects_enabled(native_window, !pointing);
    let mode = if pointing {
        CursorMode::PointingHand
    } else {
        CursorMode::Arrow
    };
    set_tracked_cursor(window, mode, CursorSurface::Thumbnail)?;
    if pointing {
        NSCursor::pointingHandCursor().set();
    } else {
        NSCursor::arrowCursor().set();
    }
    Ok(())
}

/// Reapplies the pointing cursor without rebuilding WebKit cursor state.
///
/// macOS restores the frontmost application's arrow when CES becomes
/// inactive, even though the preview can still be hovering the same button.
/// Cursor rectangles remain disabled while a button is active, so setting the
/// native cursor again is enough to restore the hand without cursor flicker.
pub fn reassert_pointing_cursor(window: &WebviewWindow) -> Result<(), &'static str> {
    if capture_overlay_owns_cursor() {
        return reset_pointing_cursor_state(window);
    }
    let native_window = native_window(window)?;
    set_cursor_rects_enabled(native_window, false);
    set_tracked_cursor(window, CursorMode::PointingHand, CursorSurface::Thumbnail)?;
    NSCursor::pointingHandCursor().set();
    Ok(())
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
    // Once the non-activating preview becomes key on hover, this second area
    // gives AppKit a standard cursor-update callback without activating CES.
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

fn should_reset_cursor_on_exit(surface: CursorSurface, capture_active: bool) -> bool {
    surface == CursorSurface::Thumbnail && !capture_active
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
    use super::{CursorSurface, cursor_surface_can_apply, should_reset_cursor_on_exit};

    #[test]
    fn active_capture_overlay_blocks_thumbnail_cursor_updates() {
        assert!(cursor_surface_can_apply(
            CursorSurface::CaptureOverlay,
            true
        ));
        assert!(!cursor_surface_can_apply(CursorSurface::Thumbnail, true));
        assert!(cursor_surface_can_apply(CursorSurface::Thumbnail, false));
    }

    #[test]
    fn only_an_available_thumbnail_resets_the_cursor_on_exit() {
        assert!(should_reset_cursor_on_exit(CursorSurface::Thumbnail, false));
        assert!(!should_reset_cursor_on_exit(CursorSurface::Thumbnail, true));
        assert!(!should_reset_cursor_on_exit(
            CursorSurface::CaptureOverlay,
            true
        ));
    }
}
