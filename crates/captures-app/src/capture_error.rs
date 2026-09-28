//! Shipping capture-failure reporting (`report_capture_error` and
//! `capture_error_message` in `apps/desktop/src-tauri/src/lib.rs`).
//!
//! A failed tray, shortcut or capture-menu screenshot shows a modal error
//! dialog titled "Captures" with a single OK button. It never lands in the
//! History error card, which only shows History load and delete failures.
//! This module also holds where Screenshot Display goes ([`display_route`]).

use captures_recording::RecordingState;

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

/// Where the display shortcut and tray "Screenshot Display" go.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayRoute {
    /// The capture menu on Full screen, with its display picker
    /// (`open_capture_controls_with_target(Screenshot, Display)`).
    CaptureMenu,
    /// Capture the display under the pointer directly, beside the recording
    /// that keeps running (`start_capture_inner(Display)`).
    CaptureDisplay,
    /// Refused silently, like shipping's `CaptureInProgress`.
    Ignore,
}

/// Shipping `lib.rs` display shortcut and `start_capture_from_tray`: with no
/// recording session (`recording_session_is_active`) they open the capture
/// menu. During a session they capture the display directly, except that
/// `screenshot_capture_is_blocked` refuses screenshots while the take is
/// selecting, counting down, finalizing or in its editor. A running or paused
/// take accepts one, and the recording carries on untouched.
pub const fn display_route(recording: Option<RecordingState>) -> DisplayRoute {
    match recording {
        None | Some(RecordingState::Ready | RecordingState::Failed | RecordingState::Discarded) => {
            DisplayRoute::CaptureMenu
        }
        Some(RecordingState::Recording | RecordingState::Paused) => DisplayRoute::CaptureDisplay,
        Some(
            RecordingState::Selecting
            | RecordingState::Countdown
            | RecordingState::Finalizing
            | RecordingState::Editor,
        ) => DisplayRoute::Ignore,
    }
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
    fn screenshot_display_opens_the_menu_unless_a_take_is_running() {
        use RecordingState::*;
        assert_eq!(display_route(None), DisplayRoute::CaptureMenu);
        for finished in [Ready, Failed, Discarded] {
            assert_eq!(display_route(Some(finished)), DisplayRoute::CaptureMenu);
        }
        for running in [Recording, Paused] {
            assert_eq!(
                display_route(Some(running)),
                DisplayRoute::CaptureDisplay,
                "{running:?} takes a direct display screenshot"
            );
        }
        for blocked in [Selecting, Countdown, Finalizing, Editor] {
            assert_eq!(
                display_route(Some(blocked)),
                DisplayRoute::Ignore,
                "{blocked:?} refuses screenshots like `screenshot_capture_is_blocked`"
            );
        }
    }
}
