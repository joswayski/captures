//! Shipping capture-failure reporting (`report_capture_error` and
//! `capture_error_message` in `apps/desktop/src-tauri/src/lib.rs`).
//!
//! A failed tray, shortcut or capture-menu screenshot shows a modal error
//! dialog titled "Captures" with a single OK button. It never lands in the
//! History error card, which only shows History load and delete failures.
//! This module also holds where the screenshot shortcuts and tray items go
//! while a recording may be running ([`screenshot_route`], [`display_route`],
//! [`new_capture_route`]).

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

/// Shipping `AppError::CaptureInProgress` text, which New Capture reports
/// while a recording session owns the capture flow.
pub const CAPTURE_IN_PROGRESS: &str = "capture already in progress";

/// Where a Screenshot Region, Window or Display shortcut or tray item goes
/// for the recording's state, if any.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenshotRoute {
    /// No recording session: region and window open their selector, and
    /// display opens the capture menu on Full screen.
    Start,
    /// A running or paused take: the same screenshot, beside the recording
    /// that keeps running (`start_capture_inner(mode)`).
    BesideRecording,
    /// Refused silently, like shipping's `CaptureInProgress`.
    Ignore,
}

/// Shipping `lib.rs` capture shortcuts and `start_capture_from_tray` call
/// `start_capture_inner(mode)` for every target, so region and window work
/// during a recording exactly like the recording controls' Screenshot button.
/// `screenshot_capture_is_blocked` (`recording.rs`) refuses them while the
/// take is selecting, counting down, finalizing or in its editor. A running
/// or paused take accepts one, and the recording carries on untouched.
pub const fn screenshot_route(recording: Option<RecordingState>) -> ScreenshotRoute {
    match recording {
        None | Some(RecordingState::Ready | RecordingState::Failed | RecordingState::Discarded) => {
            ScreenshotRoute::Start
        }
        Some(RecordingState::Recording | RecordingState::Paused) => {
            ScreenshotRoute::BesideRecording
        }
        Some(
            RecordingState::Selecting
            | RecordingState::Countdown
            | RecordingState::Finalizing
            | RecordingState::Editor,
        ) => ScreenshotRoute::Ignore,
    }
}

/// Where New Capture (its shortcut and the tray item) goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewCaptureRoute {
    /// The capture menu in Screenshot mode.
    CaptureMenu,
    /// Show the running or paused take's hidden controls, and nothing else
    /// (`restore_hidden_recording_controls`).
    RestoreControls,
    /// The capture error dialog with [`CAPTURE_IN_PROGRESS`]: shipping
    /// `prepare_capture_selector_inner` refuses the menu while a recording
    /// session is active, and `open_capture_controls` reports that failure.
    InProgress,
}

/// Shipping `open_capture_controls`: hidden controls of a running or paused
/// take come back; any other live session (`recording_session_is_active`)
/// reports "capture already in progress"; otherwise the menu opens.
pub const fn new_capture_route(
    recording: Option<RecordingState>,
    controls_hidden: bool,
) -> NewCaptureRoute {
    match recording {
        Some(RecordingState::Recording | RecordingState::Paused) if controls_hidden => {
            NewCaptureRoute::RestoreControls
        }
        None | Some(RecordingState::Ready | RecordingState::Failed | RecordingState::Discarded) => {
            NewCaptureRoute::CaptureMenu
        }
        Some(_) => NewCaptureRoute::InProgress,
    }
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
/// menu. Otherwise they follow [`screenshot_route`] and capture the display
/// under the pointer beside a running or paused take.
pub const fn display_route(recording: Option<RecordingState>) -> DisplayRoute {
    match screenshot_route(recording) {
        ScreenshotRoute::Start => DisplayRoute::CaptureMenu,
        ScreenshotRoute::BesideRecording => DisplayRoute::CaptureDisplay,
        ScreenshotRoute::Ignore => DisplayRoute::Ignore,
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

    #[test]
    fn region_and_window_screenshots_go_beside_a_running_take() {
        use RecordingState::*;
        assert_eq!(screenshot_route(None), ScreenshotRoute::Start);
        for finished in [Ready, Failed, Discarded] {
            assert_eq!(screenshot_route(Some(finished)), ScreenshotRoute::Start);
        }
        for running in [Recording, Paused] {
            assert_eq!(
                screenshot_route(Some(running)),
                ScreenshotRoute::BesideRecording,
                "{running:?} accepts a region, window or display screenshot"
            );
        }
        for blocked in [Selecting, Countdown, Finalizing, Editor] {
            assert_eq!(
                screenshot_route(Some(blocked)),
                ScreenshotRoute::Ignore,
                "{blocked:?} refuses screenshots like `screenshot_capture_is_blocked`"
            );
        }
        // Display keeps its own idle route: the capture menu on Full screen.
        for state in [
            None,
            Some(Ready),
            Some(Recording),
            Some(Paused),
            Some(Countdown),
            Some(Editor),
        ] {
            let expected = match screenshot_route(state) {
                ScreenshotRoute::Start => DisplayRoute::CaptureMenu,
                ScreenshotRoute::BesideRecording => DisplayRoute::CaptureDisplay,
                ScreenshotRoute::Ignore => DisplayRoute::Ignore,
            };
            assert_eq!(display_route(state), expected, "{state:?}");
        }
    }

    #[test]
    fn new_capture_restores_hidden_controls_or_reports_the_busy_take() {
        use RecordingState::*;
        for hidden in [false, true] {
            assert_eq!(
                new_capture_route(None, hidden),
                NewCaptureRoute::CaptureMenu
            );
            for finished in [Ready, Failed, Discarded] {
                assert_eq!(
                    new_capture_route(Some(finished), hidden),
                    NewCaptureRoute::CaptureMenu
                );
            }
            for busy in [Selecting, Countdown, Finalizing, Editor] {
                assert_eq!(
                    new_capture_route(Some(busy), hidden),
                    NewCaptureRoute::InProgress,
                    "{busy:?}"
                );
            }
        }
        for running in [Recording, Paused] {
            assert_eq!(
                new_capture_route(Some(running), true),
                NewCaptureRoute::RestoreControls
            );
            assert_eq!(
                new_capture_route(Some(running), false),
                NewCaptureRoute::InProgress,
                "visible controls: shipping reports `CaptureInProgress`"
            );
        }
        assert_eq!(
            message(CAPTURE_IN_PROGRESS),
            "Captures could not start the capture: capture already in progress"
        );
    }
}
