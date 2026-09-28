//! Shipping capture-failure reporting (`report_capture_error` and
//! `capture_error_message` in `apps/desktop/src-tauri/src/lib.rs`).
//!
//! A failed tray, shortcut or capture-menu screenshot shows a modal error
//! dialog titled "Captures" with a single OK button. It never lands in the
//! History error card, which only shows History load and delete failures.

/// Shipping dialog title for a failed capture.
pub const TITLE: &str = "Captures";
/// Shipping `report_recording_error` title. Its message is the error itself.
pub const RECORDING_TITLE: &str = "Captures Recording";
/// The dialog's only button.
pub const OK: &str = "OK";

/// Shipping `capture_error_message` fallback for a host error that crosses
/// the ABI as text. Denied permissions open the permission recovery flow
/// instead (`permission_recovery::is_permission_denied`).
pub fn message(error: &str) -> String {
    format!("Captures could not start the capture: {error}")
}

/// Shipping display shortcut and tray "Screenshot Display": they open the
/// capture menu on Full screen, with its display picker, and capture the
/// display under the pointer directly only while a recording session is
/// active (`recording_session_is_active`).
pub const fn display_opens_capture_menu(recording_active: bool) -> bool {
    !recording_active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_failures_use_the_shipping_dialog_copy() {
        assert_eq!(TITLE, "Captures");
        assert_eq!(RECORDING_TITLE, "Captures Recording");
        assert_eq!(OK, "OK");
        assert_eq!(
            message("The selected display is no longer available."),
            "Captures could not start the capture: The selected display is no longer available."
        );
    }

    #[test]
    fn screenshot_display_opens_the_menu_unless_recording() {
        assert!(display_opens_capture_menu(false));
        assert!(!display_opens_capture_menu(true));
    }
}
