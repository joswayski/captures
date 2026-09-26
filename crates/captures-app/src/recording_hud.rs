//! Recording HUD policy shared by the AppKit and wgpu hosts, ported from the
//! shipping `RecordingHud` (`apps/desktop/ui/src/App.tsx`) and its styles
//! (`.recording-hud*`, `.recording-tooltip` in `apps/desktop/ui/src/styles`).
//!
//! It owns which controls are enabled in each lifecycle state, their accessible
//! names and styled tooltip copy, the status label and dot, the inline error
//! line (including when it clears), and where a tooltip sits under its button.
//! Hosts own drawing, input and the recording session itself.

use captures_recording::RecordingState;
use serde::{Deserialize, Serialize};

use crate::tray_notice::LogicalRect;

/// Primary copy for a failed take. The New Capture menu's retry button uses the
/// same shipping string.
pub const RETRY_RECORDING: &str = "Retry recording";
pub const RESTART_RECORDING: &str = "Restart recording";
/// Shipping `recordingErrorMessage` fallback for an empty error.
pub const DEFAULT_ERROR: &str = "The recording could not be completed.";
pub const MICROPHONE_UNAVAILABLE: &str =
    "Microphone unavailable because no microphone was selected";
pub const HIDE_UNAVAILABLE: &str = "Hide unavailable because no tray restore path is available";

/// Shipping `.recording-hud-actions > .recording-tooltip > [role="tooltip"]`:
/// `top: calc(100% + 6px)` below the button.
pub const TOOLTIP_GAP: f64 = 6.0;
/// `padding: 5px 8px` inside a 1px `--glass-border`.
pub const TOOLTIP_PADDING_X: f64 = 8.0;
pub const TOOLTIP_PADDING_Y: f64 = 5.0;
/// `max-width: 180px`.
pub const TOOLTIP_MAX_WIDTH: f64 = 180.0;
/// Hidden tooltips sit 3px higher and slide down while fading in over `--dur-1`.
pub const TOOLTIP_SLIDE: f64 = 3.0;
/// The shipping tooltip has no hover delay; it only fades over `--dur-1`.
pub const TOOLTIP_DELAY_MS: u64 = 0;
/// `.recording-hud-error`: `top: calc(100% + 5px)` below the card, 10px insets.
pub const ERROR_GAP: f64 = 5.0;
pub const ERROR_INSET: f64 = 10.0;

/// HUD controls in their shipping left-to-right order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    Stop,
    PauseResume,
    Restart,
    Screenshot,
    Microphone,
    Delete,
    Hide,
}

impl Control {
    pub const ALL: [Self; 7] = [
        Self::Stop,
        Self::PauseResume,
        Self::Restart,
        Self::Screenshot,
        Self::Microphone,
        Self::Delete,
        Self::Hide,
    ];

    /// Shipping right-aligns the last three tooltips (`:nth-last-child(-n + 3)`)
    /// so Mute, Delete and Hide stay inside the HUD window; the rest are centered.
    pub const fn tooltip_right_aligned(self) -> bool {
        matches!(self, Self::Microphone | Self::Delete | Self::Hide)
    }
}

/// Status dot fill, as a design-token name the host resolves.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Dot {
    /// `--theme-signal` with its 16% halo.
    Signal,
    /// `--theme-accent` with its 16% halo (countdown and paused).
    Accent,
    /// `--info` with its 16% halo (saving).
    Info,
    /// `--glass-text-subtle`, no halo (failed).
    Subtle,
}

impl Dot {
    pub const fn token(self) -> &'static str {
        match self {
            Self::Signal => "theme-signal",
            Self::Accent => "theme-accent",
            Self::Info => "info",
            Self::Subtle => "glass-text-subtle",
        }
    }

    pub const fn halo(self) -> bool {
        !matches!(self, Self::Subtle)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Input {
    pub state: RecordingState,
    /// An action is in flight, or a confirmation is open (shipping `busy`).
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub has_microphone: bool,
    #[serde(default)]
    pub microphone_muted: bool,
    /// False where Hide has no restoration path (a Linux session without SNI).
    #[serde(default = "yes")]
    pub hide_available: bool,
}

const fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ControlView {
    pub control: Control,
    /// Accessible name (shipping `aria-label`).
    pub label: &'static str,
    /// Styled tooltip copy (shipping `HudTooltip` label).
    pub tooltip: &'static str,
    /// Shipping icon name from [`crate::icons`], or `None` for Stop's square.
    pub icon: Option<&'static str>,
    pub enabled: bool,
    pub selected: bool,
    pub tooltip_right_aligned: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    /// Shipping `recordingStatusLabel`; CSS shows it uppercased.
    pub status_label: &'static str,
    pub dot: Dot,
    pub dot_token: &'static str,
    pub dot_halo: bool,
    /// Shipping `recording-pulse` runs unless paused or failed (hosts still honor
    /// reduced motion).
    pub pulsing: bool,
    /// The elapsed timer only advances while recording.
    pub timer_running: bool,
    /// The level meter appears only when the take has a microphone.
    pub show_meter: bool,
    /// Restart asks "Restart recording?" first; a failed take retries at once.
    pub restart_confirms: bool,
    pub controls: Vec<ControlView>,
}

impl View {
    pub fn control(&self, control: Control) -> &ControlView {
        self.controls
            .iter()
            .find(|view| view.control == control)
            .expect("every control is presented")
    }
}

/// Shipping `recordingStatusLabel`.
pub const fn status_label(state: RecordingState) -> &'static str {
    match state {
        RecordingState::Countdown => "Starting…",
        RecordingState::Paused => "Paused",
        RecordingState::Finalizing => "Saving…",
        RecordingState::Failed => "Failed",
        _ => "Recording",
    }
}

/// Everything a host needs to draw the HUD for one lifecycle state.
pub fn present(input: &Input) -> View {
    let state = input.state;
    let busy = input.busy;
    let failed = state == RecordingState::Failed;
    let paused = state == RecordingState::Paused;
    let can_control = matches!(state, RecordingState::Recording | RecordingState::Paused);
    let can_restart = can_control || failed;
    let dot = match state {
        RecordingState::Countdown | RecordingState::Paused => Dot::Accent,
        RecordingState::Finalizing => Dot::Info,
        RecordingState::Failed | RecordingState::Discarded => Dot::Subtle,
        _ => Dot::Signal,
    };
    let control = |control, label, tooltip, icon, enabled, selected| ControlView {
        control,
        label,
        tooltip,
        icon,
        enabled,
        selected,
        tooltip_right_aligned: Control::tooltip_right_aligned(control),
    };
    let pause = if paused {
        "Resume recording"
    } else {
        "Pause recording"
    };
    let restart = if failed {
        RETRY_RECORDING
    } else {
        RESTART_RECORDING
    };
    let microphone = if input.microphone_muted {
        "Unmute microphone"
    } else {
        "Mute microphone"
    };
    let controls = vec![
        control(
            Control::Stop,
            "Stop recording",
            "Stop and save",
            None,
            can_control && !busy,
            false,
        ),
        control(
            Control::PauseResume,
            pause,
            pause,
            Some(if paused { "resume" } else { "pause" }),
            can_control && !busy,
            false,
        ),
        control(
            Control::Restart,
            restart,
            restart,
            Some("restart"),
            can_restart && !busy,
            false,
        ),
        control(
            Control::Screenshot,
            "Take a region screenshot",
            "Take a region screenshot",
            Some("capture"),
            can_control && !busy,
            false,
        ),
        control(
            Control::Microphone,
            if input.has_microphone {
                microphone
            } else {
                MICROPHONE_UNAVAILABLE
            },
            microphone,
            Some(if input.microphone_muted {
                "microphone-muted"
            } else {
                "microphone"
            }),
            input.has_microphone && can_control && !busy,
            input.microphone_muted,
        ),
        control(
            Control::Delete,
            "Delete recording",
            "Delete recording",
            Some("trash"),
            !busy && state != RecordingState::Finalizing,
            false,
        ),
        // Shipping leaves Hide enabled on a failed take, but its command refuses a
        // terminal session. Native hosts disable it instead of showing that error.
        control(
            Control::Hide,
            if input.hide_available {
                "Hide recording controls"
            } else {
                HIDE_UNAVAILABLE
            },
            "Hide controls",
            Some("hide-controls"),
            !busy && input.hide_available && can_control,
            false,
        ),
    ];
    View {
        status_label: status_label(state),
        dot,
        dot_token: dot.token(),
        dot_halo: dot.halo(),
        pulsing: !paused && !failed && state != RecordingState::Discarded,
        timer_running: state == RecordingState::Recording,
        show_meter: input.has_microphone,
        restart_confirms: !failed,
        controls,
    }
}

/// Shipping `recordingErrorMessage`: drop transport prefixes and capitalize a
/// leading article so engine errors read as sentences.
pub fn error_message(value: &str) -> String {
    let mut message = value.trim();
    loop {
        let lower = message.to_ascii_lowercase();
        let Some(prefix) = ["error:", "background task failed:", "recording failed:"]
            .into_iter()
            .find(|prefix| lower.starts_with(prefix))
        else {
            break;
        };
        message = message[prefix.len()..].trim_start();
    }
    if message.is_empty() {
        return DEFAULT_ERROR.into();
    }
    let article = ["the ", "a ", "an "]
        .iter()
        .any(|article| message.starts_with(article));
    if article {
        let mut chars = message.chars();
        let first = chars.next().expect("non-empty").to_ascii_uppercase();
        format!("{first}{}", chars.as_str())
    } else {
        message.to_owned()
    }
}

/// Events that change the inline HUD error line.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ErrorEvent {
    /// A new take (shipping resets on a new session id).
    SessionChanged,
    /// Any HUD action starts; shipping clears the line first.
    ActionStarted,
    /// A HUD action or the engine start failed.
    ActionFailed { message: String },
    /// The latest engine warning (microphone or desktop audio). Shipping's
    /// `recording-warning` event only fires when the warning changes.
    Warning { warning: Option<String> },
}

/// The inline error line state. `line` shows the host error, falling back to
/// the session's own failure (shipping `error || snapshot.error`).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorLine {
    pub message: Option<String>,
    pub warning: Option<String>,
}

impl ErrorLine {
    /// Applies an event and reports whether the displayed text may have changed.
    pub fn apply(&mut self, event: ErrorEvent) -> bool {
        let before = self.message.clone();
        match event {
            ErrorEvent::SessionChanged => *self = Self::default(),
            ErrorEvent::ActionStarted => self.message = None,
            ErrorEvent::ActionFailed { message } => self.message = Some(error_message(&message)),
            ErrorEvent::Warning { warning } => {
                if warning != self.warning {
                    if let Some(warning) = &warning {
                        self.message = Some(error_message(warning));
                    }
                    self.warning = warning;
                }
            }
        }
        before != self.message
    }

    pub fn line(&self, session_error: Option<&str>) -> Option<String> {
        self.message
            .clone()
            .or_else(|| session_error.map(error_message))
    }
}

/// Tooltip frame for a button (both top-left, y down, in HUD window points).
/// `text_width`/`text_height` are the measured one-line label; the width is
/// capped at [`TOOLTIP_MAX_WIDTH`]. `progress` runs 0→1 over `--dur-1`; the
/// frame slides [`TOOLTIP_SLIDE`] down while it fades in. The result stays inside
/// `bounds` so the fixed HUD window never clips it.
pub fn tooltip_frame(
    anchor: LogicalRect,
    text_width: f64,
    text_height: f64,
    right_aligned: bool,
    progress: f64,
    bounds: LogicalRect,
) -> LogicalRect {
    let width = (text_width + TOOLTIP_PADDING_X * 2. + 2.).min(TOOLTIP_MAX_WIDTH);
    let height = text_height + TOOLTIP_PADDING_Y * 2. + 2.;
    let x = if right_aligned {
        anchor.x + anchor.width - width
    } else {
        anchor.x + (anchor.width - width) / 2.
    };
    let x = x.clamp(bounds.x, (bounds.x + bounds.width - width).max(bounds.x));
    let y = (anchor.y + anchor.height + TOOLTIP_GAP)
        .min(bounds.y + bounds.height - height)
        .max(bounds.y);
    let slide = TOOLTIP_SLIDE * (1. - progress.clamp(0., 1.));
    LogicalRect::new(x, y - slide, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(state: RecordingState) -> Input {
        Input {
            state,
            busy: false,
            has_microphone: true,
            microphone_muted: false,
            hide_available: true,
        }
    }

    fn enabled(view: &View) -> Vec<Control> {
        view.controls
            .iter()
            .filter(|control| control.enabled)
            .map(|control| control.control)
            .collect()
    }

    #[test]
    fn running_and_paused_takes_enable_every_control() {
        let running = present(&input(RecordingState::Recording));
        assert_eq!(enabled(&running), Control::ALL);
        assert_eq!(running.status_label, "Recording");
        assert_eq!(running.dot, Dot::Signal);
        assert!(running.pulsing && running.timer_running && running.restart_confirms);
        let paused = present(&input(RecordingState::Paused));
        assert_eq!(enabled(&paused), Control::ALL);
        assert_eq!(
            paused.control(Control::PauseResume).label,
            "Resume recording"
        );
        assert_eq!(paused.control(Control::PauseResume).icon, Some("resume"));
        assert_eq!(paused.dot, Dot::Accent);
        assert!(!paused.pulsing && !paused.timer_running);
    }

    #[test]
    fn failed_take_offers_retry_and_delete_only() {
        let failed = present(&input(RecordingState::Failed));
        assert_eq!(enabled(&failed), [Control::Restart, Control::Delete]);
        let retry = failed.control(Control::Restart);
        assert_eq!(
            (retry.label, retry.tooltip),
            (RETRY_RECORDING, RETRY_RECORDING)
        );
        assert!(
            !failed.restart_confirms,
            "shipping retries without a confirmation"
        );
        assert_eq!(failed.status_label, "Failed");
        assert_eq!(
            (failed.dot_token, failed.dot_halo),
            ("glass-text-subtle", false)
        );
        assert!(!failed.pulsing && !failed.timer_running);
    }

    #[test]
    fn saving_and_busy_states_disable_controls() {
        let saving = present(&input(RecordingState::Finalizing));
        assert!(enabled(&saving).is_empty());
        assert_eq!(saving.status_label, "Saving…");
        assert_eq!(saving.dot_token, "info");
        assert!(
            saving.pulsing,
            "shipping keeps pulsing the info dot while saving"
        );
        let busy = present(&Input {
            busy: true,
            ..input(RecordingState::Recording)
        });
        assert!(enabled(&busy).is_empty());
        assert_eq!(status_label(RecordingState::Countdown), "Starting…");
    }

    #[test]
    fn microphone_and_hide_labels_explain_unavailable_controls() {
        let view = present(&Input {
            has_microphone: false,
            hide_available: false,
            ..input(RecordingState::Recording)
        });
        let microphone = view.control(Control::Microphone);
        assert!(!microphone.enabled && !view.show_meter);
        assert_eq!(microphone.label, MICROPHONE_UNAVAILABLE);
        assert_eq!(microphone.tooltip, "Mute microphone");
        let hide = view.control(Control::Hide);
        assert!(!hide.enabled);
        assert_eq!(
            (hide.label, hide.tooltip),
            (HIDE_UNAVAILABLE, "Hide controls")
        );
        let muted = present(&Input {
            microphone_muted: true,
            ..input(RecordingState::Recording)
        });
        let microphone = muted.control(Control::Microphone);
        assert!(microphone.selected);
        assert_eq!(microphone.label, "Unmute microphone");
        assert_eq!(microphone.icon, Some("microphone-muted"));
    }

    #[test]
    fn tooltips_use_shipping_copy_and_alignment() {
        let view = present(&input(RecordingState::Recording));
        let tooltips: Vec<_> = view
            .controls
            .iter()
            .map(|control| (control.tooltip, control.tooltip_right_aligned))
            .collect();
        assert_eq!(
            tooltips,
            [
                ("Stop and save", false),
                ("Pause recording", false),
                ("Restart recording", false),
                ("Take a region screenshot", false),
                ("Mute microphone", true),
                ("Delete recording", true),
                ("Hide controls", true),
            ]
        );
        assert!(view.controls.iter().all(|control| {
            control
                .icon
                .is_none_or(|icon| crate::icons::polylines(icon).is_some())
        }));
    }

    #[test]
    fn error_messages_match_shipping_cleanup() {
        assert_eq!(
            error_message("Error: recording failed: the microphone is busy"),
            "The microphone is busy"
        );
        assert_eq!(
            error_message("  background task failed: an encoder crashed "),
            "An encoder crashed"
        );
        assert_eq!(error_message("ERROR:   "), DEFAULT_ERROR);
        assert_eq!(error_message(""), DEFAULT_ERROR);
        assert_eq!(
            error_message("no microphone device is available"),
            "no microphone device is available"
        );
        assert_eq!(error_message("theory"), "theory");
    }

    #[test]
    fn error_line_clears_on_actions_and_shows_changed_warnings_once() {
        let mut line = ErrorLine::default();
        assert!(line.apply(ErrorEvent::Warning {
            warning: Some("The selected microphone disconnected.".into()),
        }));
        assert_eq!(
            line.line(None).as_deref(),
            Some("The selected microphone disconnected.")
        );
        assert!(line.apply(ErrorEvent::ActionStarted));
        assert_eq!(line.line(None), None);
        // The same warning in later snapshots does not reappear.
        assert!(!line.apply(ErrorEvent::Warning {
            warning: Some("The selected microphone disconnected.".into()),
        }));
        assert_eq!(line.line(None), None);
        assert!(line.apply(ErrorEvent::ActionFailed {
            message: "Error: resume failed".into(),
        }));
        assert_eq!(line.line(None).as_deref(), Some("resume failed"));
        assert!(line.apply(ErrorEvent::SessionChanged));
        assert_eq!(
            line.line(Some("recording failed: the display disappeared"))
                .as_deref(),
            Some("The display disappeared"),
            "the session's own failure shows when the host has no error"
        );
    }

    #[test]
    fn tooltip_sits_below_its_button_and_stays_in_the_window() {
        let bounds = LogicalRect::new(0., 0., 430., 102.);
        let button = LogicalRect::new(144., 39., 38., 34.);
        let shown = tooltip_frame(button, 90., 13., false, 1., bounds);
        assert_eq!(
            shown,
            LogicalRect::new(109., 77., 108., 25.),
            "kept inside the window"
        );
        let high = LogicalRect::new(144., 31., 38., 34.);
        assert_eq!(tooltip_frame(high, 90., 13., false, 1., bounds).y, 71.);
        let hidden = tooltip_frame(button, 90., 13., false, 0., bounds);
        assert_eq!(hidden.y, shown.y - TOOLTIP_SLIDE);
        let hide = LogicalRect::new(384., 39., 38., 34.);
        let right = tooltip_frame(hide, 70., 13., true, 1., bounds);
        assert_eq!(right.x + right.width, 422.);
        let long = tooltip_frame(button, 400., 13., false, 1., bounds);
        assert_eq!(long.width, TOOLTIP_MAX_WIDTH);
        let low = LogicalRect::new(10., 70., 38., 34.);
        let clamped = tooltip_frame(low, 300., 13., false, 1., bounds);
        assert!(clamped.x >= 0. && clamped.y + clamped.height <= 102.);
    }
}
