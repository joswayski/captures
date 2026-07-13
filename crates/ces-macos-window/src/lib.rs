#![cfg(target_os = "macos")]

use objc2::AllocAnyThread;
use objc2_app_kit::{
    NSCursor, NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
};
use objc2_foundation::NSRect;
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

/// Updates the cursor even while another application remains frontmost.
pub fn set_pointing_cursor(pointing: bool) {
    let cursor = if pointing {
        NSCursor::pointingHandCursor()
    } else {
        NSCursor::arrowCursor()
    };
    cursor.set();
}

fn native_window(window: &WebviewWindow) -> Result<&NSWindow, &'static str> {
    let pointer = window
        .ns_window()
        .map_err(|_| "macOS window handle is unavailable")?;
    // SAFETY: Tauri returned the NSWindow belonging to the borrowed live
    // `WebviewWindow`; the reference cannot outlive that borrow.
    unsafe { pointer.cast::<NSWindow>().as_ref() }.ok_or("macOS window handle is null")
}
