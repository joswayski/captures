#![cfg(target_os = "macos")]

use objc2::AllocAnyThread;
use objc2_app_kit::{
    NSCursor, NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
};
use objc2_foundation::{NSRect, NSSize};
use tauri::WebviewWindow;

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
    Ok(())
}

/// Applies the capture cursor after the overlay becomes the key window.
pub fn activate_capture_cursor(
    window: &WebviewWindow,
    use_crosshair: bool,
) -> Result<(), &'static str> {
    let native_window = native_window(window)?;
    // Keep WebKit cursor rectangles enabled so its CSS crosshair or camera
    // cursor remains authoritative after the next mouse event. Reset them
    // after the window becomes key so the camera cursor is refreshed without
    // replacing it with an arrow at the end of the overlay fade.
    set_cursor_rects_enabled(native_window, true);
    native_window.resetCursorRects();
    if use_crosshair {
        // CSS cursor rectangles are only applied after AppKit processes a
        // cursor update. Set the crosshair once as well so region mode changes
        // immediately even when the mouse has not moved yet.
        NSCursor::crosshairCursor().set();
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
    let cursor = if pointing {
        NSCursor::pointingHandCursor()
    } else {
        NSCursor::arrowCursor()
    };
    cursor.set();
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
    NSCursor::pointingHandCursor().set();
    Ok(())
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
