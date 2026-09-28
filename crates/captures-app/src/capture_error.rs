//! Shipping capture-failure reporting (`report_capture_error` and
//! `capture_error_message` in `apps/desktop/src-tauri/src/lib.rs`).
//!
//! A failed tray, shortcut or capture-menu screenshot shows a modal error
//! dialog titled "Captures" with a single OK button. It never lands in the
//! History error card, which only shows History load and delete failures.
//! This module also holds where the screenshot shortcuts and tray items go
//! while a recording may be running ([`screenshot_route`], [`display_route`],
//! [`new_capture_route`]), and while a capture is already open or in flight
//! ([`busy_route`]).

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

/// A capture target named by a shortcut or tray item.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Region,
    Window,
    Display,
}

/// A capture shortcut or tray item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// New Capture: `open_capture_controls(Screenshot)`.
    NewCapture,
    /// A Screenshot Region, Window or Display shortcut or tray item.
    Screenshot(Target),
    /// A Record Region, Window or Display shortcut or tray item.
    Record(Target),
}

/// What the host has open or in flight when an [`Action`] arrives. During a
/// recording this is the screenshot beside the take, if any.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Activity {
    /// No capture UI is open and no screenshot is in flight.
    Idle,
    /// A region or window selector is on screen (shipping's `overlay`).
    Selector,
    /// The capture menu is on screen (shipping's `recording-selector`), on
    /// this target in Screenshot mode, or on `None` in Record mode.
    Menu { screenshot_target: Option<Target> },
    /// A screenshot is preparing, counting down or capturing.
    Busy,
}

/// Where an [`Action`] goes while a capture is open or in flight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusyRoute {
    /// Nothing is open: [`screenshot_route`], [`display_route`] and
    /// [`new_capture_route`] decide.
    Idle,
    /// Show the take's recording controls, and nothing else.
    RestoreControls,
    /// The capture error dialog with [`CAPTURE_IN_PROGRESS`].
    InProgress,
    /// Refused silently, like shipping's `CaptureInProgress`.
    Ignore,
    /// Freeze the screen with the open capture UI in it and open this region
    /// or window selector on that snapshot in its place. A selection made
    /// there never counts down (`screenshot_countdown_seconds_for_capture_ui`).
    RecaptureSelector(Target),
    /// Beside a running or paused take: capture the display under the pointer
    /// at once with the open selector in it, then close the selector.
    RecaptureDisplay,
    /// Freeze the screen with the open capture UI in it and open the capture
    /// menu on that snapshot in its place, in Record mode when `record`.
    RecaptureMenu { record: bool, target: Target },
    /// Switch the open capture menu to this mode and target in place.
    SwitchMenu { record: bool, target: Target },
}

/// Shipping capture actions while a capture is open or in flight.
///
/// New Capture (`open_capture_controls`) first shows recording controls that
/// are off screen, which includes controls a screenshot beside the take has
/// concealed (`restore_hidden_recording_controls` checks the HUD window).
/// `prepare_capture_selector_inner` then reports "capture already in
/// progress" for any recording session, a screenshot countdown, or a
/// screenshot session without its overlay. With the overlay or the menu up it
/// recaptures: `overlay_visible` and `should_recapture_open_capture_menu`
/// freeze the capture UI into the new menu's snapshot. Otherwise it switches
/// the open menu in place (`recording-selection-ready`). Screenshot Display
/// without a recording session takes the same path on Full screen, and so do
/// the Record items in Record mode, which ignore `CaptureInProgress`.
///
/// Region and window (`start_capture_inner`) are refused silently while a
/// countdown runs or `screenshot_capture_is_blocked`. With the overlay up, or
/// the menu up on the same screenshot target, `should_recapture_visible_capture_ui`
/// freezes that UI into the new selector's snapshot; another menu target
/// switches in place (`switch_open_capture_selector_to_screenshot`). Display
/// beside a running take captures at once with the overlay in it and hides
/// the overlay afterwards.
///
/// `controls_on_screen` is whether the take's recording controls are
/// showing now, neither hidden by the user nor concealed for a screenshot.
pub const fn busy_route(
    action: Action,
    activity: Activity,
    recording: Option<RecordingState>,
    controls_on_screen: bool,
) -> BusyRoute {
    if matches!(activity, Activity::Idle) {
        return BusyRoute::Idle;
    }
    let running = matches!(
        recording,
        Some(RecordingState::Recording | RecordingState::Paused)
    );
    let session = match recording {
        Some(state) => !state.is_terminal(),
        None => false,
    };
    if session {
        return match action {
            Action::NewCapture if running && !controls_on_screen => BusyRoute::RestoreControls,
            Action::NewCapture => BusyRoute::InProgress,
            Action::Record(_) => BusyRoute::Ignore,
            Action::Screenshot(target) => match activity {
                Activity::Selector if running => match target {
                    Target::Display => BusyRoute::RecaptureDisplay,
                    Target::Region | Target::Window => BusyRoute::RecaptureSelector(target),
                },
                _ => BusyRoute::Ignore,
            },
        };
    }
    match activity {
        Activity::Idle => BusyRoute::Idle,
        Activity::Busy => match action {
            Action::NewCapture | Action::Screenshot(Target::Display) => BusyRoute::InProgress,
            Action::Screenshot(_) | Action::Record(_) => BusyRoute::Ignore,
        },
        Activity::Selector => match action {
            Action::NewCapture => BusyRoute::RecaptureMenu {
                record: false,
                target: Target::Region,
            },
            Action::Screenshot(Target::Display) => BusyRoute::RecaptureMenu {
                record: false,
                target: Target::Display,
            },
            Action::Screenshot(target) => BusyRoute::RecaptureSelector(target),
            Action::Record(target) => BusyRoute::RecaptureMenu {
                record: true,
                target,
            },
        },
        Activity::Menu { screenshot_target } => {
            let (record, target) = match action {
                Action::NewCapture => (false, Target::Region),
                Action::Screenshot(target) => (false, target),
                Action::Record(target) => (true, target),
            };
            let same = !record
                && matches!(
                    (screenshot_target, target),
                    (Some(Target::Region), Target::Region)
                        | (Some(Target::Window), Target::Window)
                        | (Some(Target::Display), Target::Display)
                );
            if !same {
                BusyRoute::SwitchMenu { record, target }
            } else if matches!(action, Action::Screenshot(Target::Region | Target::Window)) {
                BusyRoute::RecaptureSelector(target)
            } else {
                BusyRoute::RecaptureMenu {
                    record: false,
                    target,
                }
            }
        }
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

    #[test]
    fn busy_new_capture_restores_off_screen_controls_or_reports_the_capture() {
        use RecordingState::*;
        for activity in [Activity::Selector, Activity::Busy] {
            for running in [Recording, Paused] {
                assert_eq!(
                    busy_route(Action::NewCapture, activity, Some(running), false),
                    BusyRoute::RestoreControls,
                    "concealed controls come back ({activity:?}, {running:?})"
                );
                assert_eq!(
                    busy_route(Action::NewCapture, activity, Some(running), true),
                    BusyRoute::InProgress
                );
            }
            for state in [Selecting, Countdown, Finalizing, Editor] {
                for on_screen in [false, true] {
                    assert_eq!(
                        busy_route(Action::NewCapture, activity, Some(state), on_screen),
                        BusyRoute::InProgress,
                        "{state:?}"
                    );
                }
            }
        }
        // Outside a recording a countdown or a screenshot without its
        // selector reports the busy capture, and so does Screenshot Display,
        // which opens the menu through the same path.
        for recording in [None, Some(Ready), Some(Failed), Some(Discarded)] {
            for action in [Action::NewCapture, Action::Screenshot(Target::Display)] {
                assert_eq!(
                    busy_route(action, Activity::Busy, recording, false),
                    BusyRoute::InProgress
                );
            }
            for action in [
                Action::Screenshot(Target::Region),
                Action::Screenshot(Target::Window),
                Action::Record(Target::Region),
                Action::Record(Target::Display),
            ] {
                assert_eq!(
                    busy_route(action, Activity::Busy, recording, false),
                    BusyRoute::Ignore,
                    "{action:?}"
                );
            }
        }
        for action in [Action::NewCapture, Action::Screenshot(Target::Region)] {
            assert_eq!(
                busy_route(action, Activity::Idle, Some(Recording), true),
                BusyRoute::Idle
            );
        }
    }

    #[test]
    fn a_shortcut_pressed_again_over_an_open_selector_recaptures_it() {
        use RecordingState::*;
        for recording in [None, Some(Ready)] {
            for target in [Target::Region, Target::Window] {
                assert_eq!(
                    busy_route(
                        Action::Screenshot(target),
                        Activity::Selector,
                        recording,
                        false
                    ),
                    BusyRoute::RecaptureSelector(target)
                );
            }
            assert_eq!(
                busy_route(Action::NewCapture, Activity::Selector, recording, false),
                BusyRoute::RecaptureMenu {
                    record: false,
                    target: Target::Region
                }
            );
            assert_eq!(
                busy_route(
                    Action::Screenshot(Target::Display),
                    Activity::Selector,
                    recording,
                    false
                ),
                BusyRoute::RecaptureMenu {
                    record: false,
                    target: Target::Display
                }
            );
            for target in [Target::Region, Target::Window, Target::Display] {
                assert_eq!(
                    busy_route(Action::Record(target), Activity::Selector, recording, false),
                    BusyRoute::RecaptureMenu {
                        record: true,
                        target
                    }
                );
            }
        }
        // Beside a running take the selector is the take's screenshot.
        for running in [Recording, Paused] {
            for on_screen in [false, true] {
                for target in [Target::Region, Target::Window] {
                    assert_eq!(
                        busy_route(
                            Action::Screenshot(target),
                            Activity::Selector,
                            Some(running),
                            on_screen
                        ),
                        BusyRoute::RecaptureSelector(target)
                    );
                }
                assert_eq!(
                    busy_route(
                        Action::Screenshot(Target::Display),
                        Activity::Selector,
                        Some(running),
                        on_screen
                    ),
                    BusyRoute::RecaptureDisplay
                );
                for activity in [Activity::Selector, Activity::Busy] {
                    assert_eq!(
                        busy_route(
                            Action::Record(Target::Region),
                            activity,
                            Some(running),
                            on_screen
                        ),
                        BusyRoute::Ignore
                    );
                    if activity == Activity::Busy {
                        assert_eq!(
                            busy_route(
                                Action::Screenshot(Target::Window),
                                activity,
                                Some(running),
                                on_screen
                            ),
                            BusyRoute::Ignore,
                            "a countdown refuses screenshots silently"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_open_capture_menu_switches_in_place_or_recaptures_its_own_target() {
        let menu = |target| Activity::Menu {
            screenshot_target: target,
        };
        // Another target or mode switches the menu in place.
        assert_eq!(
            busy_route(
                Action::Screenshot(Target::Window),
                menu(Some(Target::Region)),
                None,
                false
            ),
            BusyRoute::SwitchMenu {
                record: false,
                target: Target::Window
            }
        );
        assert_eq!(
            busy_route(Action::NewCapture, menu(Some(Target::Display)), None, false),
            BusyRoute::SwitchMenu {
                record: false,
                target: Target::Region
            }
        );
        assert_eq!(
            busy_route(Action::NewCapture, menu(None), None, false),
            BusyRoute::SwitchMenu {
                record: false,
                target: Target::Region
            },
            "New Capture over Record mode switches to Screenshot"
        );
        for screenshot_target in [None, Some(Target::Region), Some(Target::Display)] {
            assert_eq!(
                busy_route(
                    Action::Record(Target::Region),
                    menu(screenshot_target),
                    None,
                    false
                ),
                BusyRoute::SwitchMenu {
                    record: true,
                    target: Target::Region
                },
                "Record items never recapture the menu"
            );
        }
        assert_eq!(
            busy_route(Action::Screenshot(Target::Region), menu(None), None, false),
            BusyRoute::SwitchMenu {
                record: false,
                target: Target::Region
            }
        );
        // The same screenshot target again freezes the menu into a new capture.
        for target in [Target::Region, Target::Window] {
            assert_eq!(
                busy_route(Action::Screenshot(target), menu(Some(target)), None, false),
                BusyRoute::RecaptureSelector(target)
            );
        }
        assert_eq!(
            busy_route(
                Action::Screenshot(Target::Display),
                menu(Some(Target::Display)),
                None,
                false
            ),
            BusyRoute::RecaptureMenu {
                record: false,
                target: Target::Display
            }
        );
        assert_eq!(
            busy_route(Action::NewCapture, menu(Some(Target::Region)), None, false),
            BusyRoute::RecaptureMenu {
                record: false,
                target: Target::Region
            }
        );
    }
}
