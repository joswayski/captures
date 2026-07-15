#![cfg(target_os = "macos")]

use std::{cell::Cell, ffi::c_void, ptr};

use objc2::{
    AllocAnyThread, DefinedClass, define_class,
    ffi::{OBJC_ASSOCIATION_RETAIN_NONATOMIC, objc_getAssociatedObject, objc_setAssociatedObject},
    msg_send,
    rc::Retained,
    runtime::AnyObject,
};
use objc2_app_kit::{
    NSCursor, NSEvent, NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
};
use objc2_foundation::{NSObject, NSRect, NSSize};
use tauri::WebviewWindow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorMode {
    Arrow,
    Crosshair,
    PointingHand,
    WebView,
}

struct CursorTrackingIvars {
    mode: Cell<CursorMode>,
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
        fn mouse_entered(&self, _event: &NSEvent) {
            self.apply_cursor();
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, _event: &NSEvent) {
            self.apply_cursor();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            NSCursor::arrowCursor().set();
        }
    }
);

impl CursorTrackingOwner {
    fn new(mode: CursorMode) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CursorTrackingIvars {
            mode: Cell::new(mode),
        });
        // SAFETY: `NSObject`'s `init` method has this signature.
        unsafe { msg_send![super(this), init] }
    }

    fn set_mode(&self, mode: CursorMode) {
        self.ivars().mode.set(mode);
    }

    fn apply_cursor(&self) {
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

/// Configures the capture preview as an inactive-app HUD.
///
/// Tauri enables mouse-move events on its `NSWindow`, but WebKit's own tracking
/// can still become inactive when another application is frontmost. An
/// `ActiveAlways` tracking area keeps CSS hover and pointer events alive.
pub fn configure_inactive_hover(window: &WebviewWindow) -> Result<(), &'static str> {
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
            install_cursor_tracker(webview, CursorMode::Arrow);
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

/// Makes a reused capture overlay transparent before bringing it onscreen.
pub fn prepare_capture_overlay(window: &WebviewWindow) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    native_window.setAlphaValue(0.0);
    set_cursor_rects_enabled(native_window, true);
    set_tracked_cursor(window, CursorMode::WebView)?;
    Ok(())
}

/// Applies the capture cursor after the overlay becomes the key window.
pub fn activate_capture_cursor(
    window: &WebviewWindow,
    use_crosshair: bool,
) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    if use_crosshair {
        // A newly created, previously hidden WKWebView can still own a stale
        // arrow cursor rectangle. Disable cursor rectangles for the duration
        // of region capture so the first mouse movement cannot replace the
        // native crosshair.
        set_cursor_rects_enabled(native_window, false);
        set_tracked_cursor(window, CursorMode::Crosshair)?;
        NSCursor::crosshairCursor().set();
    } else {
        // Window capture uses a custom CSS camera cursor, so WebKit remains the
        // cursor owner in this mode. Refresh its rectangles after the overlay
        // becomes key and after its fade-in completes.
        set_cursor_rects_enabled(native_window, true);
        set_tracked_cursor(window, CursorMode::WebView)?;
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
    let native_window = native_window(window)?;
    native_window.setAlphaValue(1.0);
    set_cursor_rects_enabled(native_window, true);
    set_tracked_cursor(window, CursorMode::Arrow)?;
    NSCursor::arrowCursor().set();
    Ok(())
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
    let native_window = native_window(window)?;
    set_cursor_rects_enabled(native_window, !pointing);
    let mode = if pointing {
        CursorMode::PointingHand
    } else {
        CursorMode::Arrow
    };
    set_tracked_cursor(window, mode)?;
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
    let native_window = native_window(window)?;
    set_cursor_rects_enabled(native_window, false);
    set_tracked_cursor(window, CursorMode::PointingHand)?;
    NSCursor::pointingHandCursor().set();
    Ok(())
}

fn set_tracked_cursor(window: &WebviewWindow, mode: CursorMode) -> Result<(), &'static str> {
    window
        .as_ref()
        .with_webview(move |platform_webview| {
            let pointer = platform_webview.inner();
            // SAFETY: Tauri supplies a live WKWebView, which inherits from
            // NSView, for the duration of this callback.
            let Some(webview) = (unsafe { pointer.cast::<NSView>().as_ref() }) else {
                return;
            };
            install_cursor_tracker(webview, mode);
        })
        .map_err(|_| "macOS webview handle is unavailable")
}

fn install_cursor_tracker(webview: &NSView, mode: CursorMode) {
    if let Some(owner) = associated_cursor_tracker(webview) {
        owner.set_mode(mode);
        return;
    }

    let owner = CursorTrackingOwner::new(mode);
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

fn native_window(window: &WebviewWindow) -> Result<&NSWindow, &'static str> {
    let pointer = window
        .ns_window()
        .map_err(|_| "macOS window handle is unavailable")?;
    // SAFETY: Tauri returned the NSWindow belonging to the borrowed live
    // `WebviewWindow`; the reference cannot outlive that borrow.
    unsafe { pointer.cast::<NSWindow>().as_ref() }.ok_or("macOS window handle is null")
}
