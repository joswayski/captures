//! Shipping New Capture menu copy and small presentation policies, ported from
//! the Tauri `RecordingSelector`, `CaptureGuidance` and
//! `CaptureSelectorVisibilityNote` (`apps/desktop/ui/src/App.tsx`) so the AppKit
//! and wgpu hosts render the same labels, states and pointer behavior.
//!
//! Everything here is pure: hosts own layout, drawing and input.

use crate::motion::{Animation, Motion, MotionTokens, Transition, Tween};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Screenshot or Record, the menu's capture-type segment.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuMode {
    #[default]
    Screenshot,
    Recording,
}

impl MenuMode {
    const fn output(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshots",
            Self::Recording => "recordings",
        }
    }
}

/// Preferences rows the capture menu links to. Opening one scrolls Preferences to
/// that row and highlights it for [`PREFERENCE_HIGHLIGHT`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreferenceTarget {
    AutoStartOnSelection,
    IncludeRecordingControlsInCaptures,
}

impl PreferenceTarget {
    /// The shipping `open_preferences` target name.
    pub const fn id(self) -> &'static str {
        match self {
            Self::AutoStartOnSelection => "auto-start-on-selection",
            Self::IncludeRecordingControlsInCaptures => "include-recording-controls-in-captures",
        }
    }

    /// The persisted setting the highlighted row edits.
    pub const fn setting_key(self) -> &'static str {
        match self {
            Self::AutoStartOnSelection => "auto_start_on_selection",
            Self::IncludeRecordingControlsInCaptures => "include_recording_controls_in_captures",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [
            Self::AutoStartOnSelection,
            Self::IncludeRecordingControlsInCaptures,
        ]
        .into_iter()
        .find(|target| target.id() == id)
    }
}

/// Shipping `PREFERENCE_HIGHLIGHT_MS`.
pub const PREFERENCE_HIGHLIGHT: Duration = Duration::from_millis(2_400);

/// "These controls **won’t** show in screenshots". `emphasis` is drawn bold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VisibilityNote {
    pub lead: &'static str,
    pub emphasis: &'static str,
    pub trail: String,
    /// Linux recording copy: the bar cannot be excluded, so point at Hide controls.
    pub hint: Option<&'static str>,
    /// Set when the note links to the Preferences row that changes it. Hosts
    /// that cannot exclude the controls show plain text.
    pub target: Option<PreferenceTarget>,
}

impl VisibilityNote {
    pub fn text(&self) -> String {
        format!(
            "{}{}{}{}",
            self.lead,
            self.emphasis,
            self.trail,
            self.hint.unwrap_or_default()
        )
    }
}

pub fn visibility_note(
    mode: MenuMode,
    can_exclude_controls: bool,
    controls_excluded: bool,
) -> VisibilityNote {
    let excluded = can_exclude_controls && controls_excluded;
    VisibilityNote {
        lead: "These controls ",
        emphasis: if excluded { "won’t" } else { "will" },
        trail: format!(" show in {}", mode.output()),
        hint: (!excluded && !can_exclude_controls && mode == MenuMode::Recording)
            .then_some(" · Use Hide controls to keep them out"),
        target: can_exclude_controls
            .then_some(PreferenceTarget::IncludeRecordingControlsInCaptures),
    }
}

pub const NOTE_SEPARATOR: &str = "·";
pub const CONFIRM_NOTE: &str = "Press Enter to confirm";
pub const AUTO_START_NOTE: &str = "Auto-capture is on. Selecting a target starts immediately.";

/// The text after the visibility note and its separator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConfirmNote {
    pub text: &'static str,
    pub target: Option<PreferenceTarget>,
}

pub const fn confirm_note(auto_start: bool) -> ConfirmNote {
    if auto_start {
        ConfirmNote {
            text: AUTO_START_NOTE,
            target: Some(PreferenceTarget::AutoStartOnSelection),
        }
    } else {
        ConfirmNote {
            text: CONFIRM_NOTE,
            target: None,
        }
    }
}

/// Transient menu state that changes the primary button.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PrimaryState {
    pub starting: bool,
    pub switching_display: bool,
    pub error: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PrimaryAction {
    pub label: &'static str,
    pub accessibility_label: &'static str,
    /// Auto-start hides the button unless a start is in flight or failed.
    pub hidden: bool,
}

pub const fn primary_action(
    mode: MenuMode,
    auto_start: bool,
    state: PrimaryState,
) -> PrimaryAction {
    let screenshot = matches!(mode, MenuMode::Screenshot);
    let retrying = auto_start && state.error;
    let label = if state.starting {
        if screenshot {
            "Capturing…"
        } else {
            "Starting…"
        }
    } else if state.switching_display {
        "Switching…"
    } else if retrying {
        if screenshot {
            "Retry capture"
        } else {
            "Retry recording"
        }
    } else if screenshot {
        "Capture"
    } else {
        "Start recording"
    };
    let accessibility_label = if retrying {
        if screenshot {
            "Retry capture"
        } else {
            "Retry recording"
        }
    } else if screenshot {
        "Take screenshot"
    } else {
        "Start recording"
    };
    PrimaryAction {
        label,
        accessibility_label,
        hidden: auto_start && !state.starting && !state.error,
    }
}

pub const FIELD_FPS: &str = "FPS";
pub const FIELD_MAX_RESOLUTION: &str = "Max resolution";
pub const FIELD_MICROPHONE: &str = "Microphone";
pub const FPS_ACCESSIBILITY_LABEL: &str = "Frames per second";
pub const MAX_RESOLUTION_ACCESSIBILITY_LABEL: &str = "Maximum resolution";
/// Shipping order, highest first.
pub const FPS_OPTIONS: [u16; 3] = [60, 30, 15];
/// Persisted `MaxResolution` value and label.
pub const RESOLUTION_OPTIONS: [(&str, &str); 3] = [
    ("original", "Original"),
    ("p1080", "1080p"),
    ("p720", "720p"),
];

/// A labelled On/Off switch in the recording-options row.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingToggle {
    ShowCursor,
    ShowClicks,
    DesktopAudio,
}

impl RecordingToggle {
    pub const ALL: [Self; 3] = [Self::ShowCursor, Self::ShowClicks, Self::DesktopAudio];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ShowCursor => "Show cursor",
            Self::ShowClicks => "Show clicks",
            Self::DesktopAudio => "Desktop audio",
        }
    }

    pub const fn accessibility_label(self) -> &'static str {
        match self {
            Self::ShowCursor => "Show cursor",
            Self::ShowClicks => "Show clicks",
            Self::DesktopAudio => "Record desktop audio",
        }
    }

    /// Tooltip on a disabled switch.
    pub const fn unavailable_reason(self) -> &'static str {
        match self {
            Self::ShowCursor => "Cursor capture is unavailable in this desktop session",
            Self::ShowClicks => "Click highlights are unavailable in this desktop session",
            Self::DesktopAudio => "Desktop audio recording is unavailable in this desktop session",
        }
    }
}

/// Text beside a recording switch.
pub const fn toggle_status(available: bool, on: bool) -> &'static str {
    if !available {
        "Unavailable"
    } else if on {
        "On"
    } else {
        "Off"
    }
}

/// Cursor/clicks coupling after `changed` was set: clicks need a visible
/// cursor, so turning clicks on shows the cursor and hiding the cursor turns
/// clicks off. Returns `(show_cursor, highlight_clicks)`.
pub const fn couple_pointer_options(
    changed: RecordingToggle,
    show_cursor: bool,
    highlight_clicks: bool,
) -> (bool, bool) {
    match changed {
        RecordingToggle::ShowClicks if highlight_clicks => (true, true),
        RecordingToggle::ShowCursor if !show_cursor => (false, false),
        _ => (show_cursor, highlight_clicks),
    }
}

/// One microphone menu row. `id: None` is Off.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MicrophoneEntry {
    pub id: Option<String>,
    pub label: String,
    pub enabled: bool,
}

pub const MICROPHONES_LOADING: &str = "Loading microphones…";
pub const MICROPHONE_LOADING: &str = "Loading microphone…";
pub const SELECTED_MICROPHONE: &str = "Selected microphone";

/// Shipping microphone options: Off (or Unavailable), a disabled loading row
/// while devices enumerate, the saved device when it is not listed, then devices.
pub fn microphone_entries(
    available: bool,
    loading: bool,
    selected: Option<&str>,
    devices: &[(&str, &str)],
) -> Vec<MicrophoneEntry> {
    let mut entries = vec![MicrophoneEntry {
        id: None,
        label: if available { "Off" } else { "Unavailable" }.into(),
        enabled: true,
    }];
    if loading {
        entries.push(MicrophoneEntry {
            id: Some("__loading".into()),
            label: MICROPHONES_LOADING.into(),
            enabled: false,
        });
    }
    if let Some(selected) = selected
        && !devices.iter().any(|(id, _)| *id == selected)
    {
        entries.push(MicrophoneEntry {
            id: Some(selected.into()),
            label: if loading {
                MICROPHONE_LOADING
            } else {
                SELECTED_MICROPHONE
            }
            .into(),
            enabled: true,
        });
    }
    entries.extend(devices.iter().map(|(id, name)| MicrophoneEntry {
        id: Some((*id).into()),
        label: (*name).into(),
        enabled: true,
    }));
    entries
}

/// The label shown on the closed microphone select.
pub fn microphone_selected_label(
    available: bool,
    loading: bool,
    selected: Option<&str>,
    devices: &[(&str, &str)],
) -> String {
    microphone_entries(available, loading, selected, devices)
        .into_iter()
        .find(|entry| entry.enabled && entry.id.as_deref() == selected)
        .map_or_else(|| SELECTED_MICROPHONE.into(), |entry| entry.label)
}

/// Which `CaptureGuidance` chip is shown.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceTarget {
    Region,
    Window,
    /// Window mode while the pointer is over the desktop or shell chrome.
    Display,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Guidance {
    pub title: &'static str,
    pub hint: &'static str,
}

pub const fn guidance(target: GuidanceTarget, feedback: bool) -> Guidance {
    Guidance {
        title: match target {
            GuidanceTarget::Display => "Click to capture this display",
            GuidanceTarget::Window => "Select a window to continue",
            GuidanceTarget::Region if feedback => "Click and drag to select a region",
            GuidanceTarget::Region => "Drag to select a region",
        },
        hint: match target {
            GuidanceTarget::Region => "Shift for square · Esc to cancel",
            _ => "Esc to cancel",
        },
    }
}

/// Accessible name of the region overlay. AppKit `RegionSelection` uses the
/// same text; shipping exposes the overlay as an unlabelled `<main>`.
pub const REGION_SELECTOR_LABEL: &str = "Capture region selector";
/// Accessible name of the window overlay (AppKit `WindowSelection`).
pub const WINDOW_SELECTOR_LABEL: &str = "Capture window selector";
/// Name for the window overlay's current target when nothing is hovered.
pub const DISPLAY_TARGET: &str = "Entire display";

/// The window overlay's live target description (`Target: Safari`).
pub fn target_description(name: &str) -> String {
    format!("Target: {name}")
}

/// The region overlay's selection description, in logical pixels like the
/// visible size badge.
pub fn region_description(width: f64, height: f64) -> String {
    format!(
        "Selected region {} × {} logical pixels",
        width.round() as i64,
        height.round() as i64
    )
}

/// Fade when the pointer is this close to the chip (shipping 28 px).
pub const GUIDANCE_APPROACH_PAD: f64 = 28.0;
/// Extra slack before a faded chip restores, so it cannot thrash at the edge.
pub const GUIDANCE_LEAVE_SLACK: f64 = 12.0;

/// Shipping `isPointerOverCaptureGuidance`: hit-test the chip bounds (logical
/// top-left coordinates) with enter/leave hysteresis.
pub fn pointer_over_guidance(
    x: f64,
    y: f64,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    currently_over: bool,
) -> bool {
    let pad = if currently_over {
        GUIDANCE_APPROACH_PAD + GUIDANCE_LEAVE_SLACK
    } else {
        GUIDANCE_APPROACH_PAD
    };
    x >= left - pad && x <= right + pad && y >= top - pad && y <= bottom + pad
}

/// Shipping `.capture-guidance { top: 16% }`: the chip's resting top edge, as
/// a fraction of the overlay height.
pub const GUIDANCE_TOP_FRACTION: f64 = 0.16;
/// The chip's entrance offset before `data-ready` (`translate(-50%, -6px)`),
/// in points, negative upward.
pub const GUIDANCE_ENTER_OFFSET: f64 = -6.0;
/// Gap between the title and hint rows (`gap: 2px`).
pub const GUIDANCE_ROW_GAP: f64 = 2.0;
/// Shipping `showSelectionFeedback`: how long "Click and drag to select a
/// region", the accent border and the nudge last after a click without a drag.
pub const GUIDANCE_FEEDBACK: Duration = Duration::from_millis(1_800);
/// The feedback border: `rgba(var(--theme-accent-rgb), 0.8)`.
pub const GUIDANCE_FEEDBACK_BORDER_ALPHA: f64 = 0.8;

/// The chip's resting top edge in an overlay `height` points tall.
pub fn guidance_top(height: f64) -> f64 {
    height * GUIDANCE_TOP_FRACTION
}

/// The direct overlays' hint row. Shipping commits on release or click; the
/// manual (confirm with Enter) mode appends the confirm note.
pub fn direct_hint(target: GuidanceTarget, confirm: bool) -> String {
    let hint = guidance(target, false).hint;
    if confirm {
        format!("{hint} {NOTE_SEPARATOR} {CONFIRM_NOTE}")
    } else {
        hint.into()
    }
}

/// Where the guidance chip is drawn this frame, relative to its resting
/// frame. `translate_x` is the feedback nudge and `translate_y` the entrance
/// slide, both in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuidancePose {
    pub opacity: f64,
    pub translate_x: f64,
    pub translate_y: f64,
    /// Feedback state: accent border at [`GUIDANCE_FEEDBACK_BORDER_ALPHA`].
    pub accent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Leg {
    from: f64,
    to: f64,
    at_ms: f64,
}

/// Shipping `CaptureGuidance` motion for hosts that sample poses per frame.
///
/// The chip mounts transparent and 6 points high, then fades and slides in.
/// While `shown` is false (a region drag, or the pointer within
/// [`GUIDANCE_APPROACH_PAD`]) it fades out in place; showing again fades back.
/// Like a CSS transition, a change mid-flight restarts from the current
/// value. Shipping re-keys the component whenever the feedback attempt
/// changes, so [`GuidanceChip::remount`] replays the entrance, and a feedback
/// mount also plays [`Motion::CaptureGuidanceNudge`]. Under reduced motion
/// every change lands at once and the nudge rests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuidanceChip {
    fade: Tween,
    slide: Tween,
    nudge: Animation,
    reduced: bool,
    shown: bool,
    feedback: bool,
    mounted_ms: f64,
    opacity: Leg,
    offset: Leg,
}

impl GuidanceChip {
    /// Mount at `now_ms`. `None` names a missing motion token.
    pub fn new(
        tokens: &impl MotionTokens,
        reduced: bool,
        now_ms: f64,
        shown: bool,
        feedback: bool,
    ) -> Option<Self> {
        let mut chip = Self {
            fade: Transition::CaptureGuidanceFade.resolve(tokens)?,
            slide: Transition::CaptureGuidanceSlide.resolve(tokens)?,
            nudge: Motion::CaptureGuidanceNudge.resolve(tokens)?,
            reduced,
            shown,
            feedback,
            mounted_ms: now_ms,
            opacity: Leg {
                from: 0.,
                to: 0.,
                at_ms: now_ms,
            },
            offset: Leg {
                from: GUIDANCE_ENTER_OFFSET,
                to: GUIDANCE_ENTER_OFFSET,
                at_ms: now_ms,
            },
        };
        chip.remount(now_ms, shown, feedback);
        Some(chip)
    }

    /// Shipping's re-key: start over from the entrance.
    pub fn remount(&mut self, now_ms: f64, shown: bool, feedback: bool) {
        self.mounted_ms = now_ms;
        self.feedback = feedback;
        self.shown = shown;
        // `data-ready` lands on the next frame; both faded and shown states
        // settle the slide at rest.
        self.opacity = Leg {
            from: 0.,
            to: if shown { 1. } else { 0. },
            at_ms: now_ms,
        };
        self.offset = Leg {
            from: GUIDANCE_ENTER_OFFSET,
            to: 0.,
            at_ms: now_ms,
        };
    }

    /// Fade in or out from wherever the chip is now.
    pub fn set_shown(&mut self, shown: bool, now_ms: f64) {
        if shown == self.shown {
            return;
        }
        self.shown = shown;
        let opacity = self.opacity_at(now_ms);
        self.opacity = Leg {
            from: opacity,
            to: if shown { 1. } else { 0. },
            at_ms: now_ms,
        };
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    pub fn feedback(&self) -> bool {
        self.feedback
    }

    fn opacity_at(&self, now_ms: f64) -> f64 {
        let leg = self.opacity;
        self.fade
            .value(leg.from, leg.to, now_ms - leg.at_ms, self.reduced)
    }

    pub fn pose(&self, now_ms: f64) -> GuidancePose {
        let leg = self.offset;
        GuidancePose {
            opacity: self.opacity_at(now_ms).clamp(0., 1.),
            translate_x: if self.feedback {
                self.nudge
                    .pose_at(now_ms - self.mounted_ms, self.reduced)
                    .translate_x
            } else {
                0.
            },
            translate_y: self
                .slide
                .value(leg.from, leg.to, now_ms - leg.at_ms, self.reduced),
            accent: self.feedback,
        }
    }

    /// True while a host must keep painting frames.
    pub fn animating(&self, now_ms: f64) -> bool {
        self.fade.running(now_ms - self.opacity.at_ms, self.reduced)
            || self.slide.running(now_ms - self.offset.at_ms, self.reduced)
            || (self.feedback && self.nudge.running(now_ms - self.mounted_ms, self.reduced))
    }
}

/// Full-screen display identity: name, then "W × H" (and "· N FPS" in Record).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DisplayIdentity {
    pub name: String,
    pub detail: String,
}

pub fn display_identity(
    name: &str,
    width: u32,
    height: u32,
    recording_fps: Option<u16>,
) -> DisplayIdentity {
    let name = name.trim();
    DisplayIdentity {
        name: if name.is_empty() { "Display" } else { name }.into(),
        detail: match recording_fps {
            Some(fps) => format!("{width} × {height} · {fps} FPS"),
            None => format!("{width} × {height}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_accessibility_copy_matches_appkit() {
        assert_eq!(REGION_SELECTOR_LABEL, "Capture region selector");
        assert_eq!(WINDOW_SELECTOR_LABEL, "Capture window selector");
        assert_eq!(target_description(DISPLAY_TARGET), "Target: Entire display");
        assert_eq!(
            region_description(319.6, 180.2),
            "Selected region 320 × 180 logical pixels"
        );
    }

    #[test]
    fn visibility_note_follows_capabilities_like_shipping() {
        let note = visibility_note(MenuMode::Screenshot, true, true);
        assert_eq!(note.text(), "These controls won’t show in screenshots");
        assert_eq!(note.emphasis, "won’t");
        assert_eq!(
            note.target,
            Some(PreferenceTarget::IncludeRecordingControlsInCaptures)
        );
        let note = visibility_note(MenuMode::Recording, true, false);
        assert_eq!(note.text(), "These controls will show in recordings");
        assert!(note.target.is_some());
        // Linux cannot exclude: never "won't", no link, Hide controls hint in Record.
        let note = visibility_note(MenuMode::Recording, false, true);
        assert_eq!(
            note.text(),
            "These controls will show in recordings · Use Hide controls to keep them out"
        );
        assert_eq!(note.target, None);
        let note = visibility_note(MenuMode::Screenshot, false, false);
        assert_eq!(note.text(), "These controls will show in screenshots");
        assert_eq!(note.target, None);
    }

    #[test]
    fn confirm_note_links_only_auto_start() {
        assert_eq!(confirm_note(false).text, "Press Enter to confirm");
        assert_eq!(confirm_note(false).target, None);
        assert_eq!(
            confirm_note(true).target,
            Some(PreferenceTarget::AutoStartOnSelection)
        );
        assert_eq!(
            PreferenceTarget::from_id("auto-start-on-selection"),
            Some(PreferenceTarget::AutoStartOnSelection)
        );
        assert_eq!(
            PreferenceTarget::IncludeRecordingControlsInCaptures.setting_key(),
            "include_recording_controls_in_captures"
        );
        assert_eq!(PreferenceTarget::from_id("freeze"), None);
    }

    #[test]
    fn primary_action_matches_shipping_labels_and_hide_rule() {
        let idle = PrimaryState::default();
        let screenshot = primary_action(MenuMode::Screenshot, false, idle);
        assert_eq!(
            (
                screenshot.label,
                screenshot.accessibility_label,
                screenshot.hidden
            ),
            ("Capture", "Take screenshot", false)
        );
        let record = primary_action(MenuMode::Recording, false, idle);
        assert_eq!((record.label, record.hidden), ("Start recording", false));
        assert!(primary_action(MenuMode::Recording, true, idle).hidden);
        let starting = PrimaryState {
            starting: true,
            ..idle
        };
        assert_eq!(
            primary_action(MenuMode::Screenshot, true, starting),
            PrimaryAction {
                label: "Capturing…",
                accessibility_label: "Take screenshot",
                hidden: false
            }
        );
        assert_eq!(
            primary_action(MenuMode::Recording, false, starting).label,
            "Starting…"
        );
        let switching = PrimaryState {
            switching_display: true,
            ..idle
        };
        assert_eq!(
            primary_action(MenuMode::Screenshot, false, switching).label,
            "Switching…"
        );
        assert!(primary_action(MenuMode::Screenshot, true, switching).hidden);
        let error = PrimaryState {
            error: true,
            ..idle
        };
        let retry = primary_action(MenuMode::Screenshot, true, error);
        assert_eq!(
            (retry.label, retry.accessibility_label, retry.hidden),
            ("Retry capture", "Retry capture", false)
        );
        assert_eq!(
            primary_action(MenuMode::Recording, true, error).label,
            "Retry recording"
        );
        assert_eq!(
            primary_action(MenuMode::Screenshot, false, error).label,
            "Capture"
        );
    }

    #[test]
    fn toggles_report_status_and_couple_cursor_with_clicks() {
        assert_eq!(toggle_status(false, true), "Unavailable");
        assert_eq!(toggle_status(true, true), "On");
        assert_eq!(toggle_status(true, false), "Off");
        assert_eq!(
            RecordingToggle::DesktopAudio.accessibility_label(),
            "Record desktop audio"
        );
        assert_eq!(
            couple_pointer_options(RecordingToggle::ShowClicks, false, true),
            (true, true)
        );
        assert_eq!(
            couple_pointer_options(RecordingToggle::ShowCursor, false, true),
            (false, false)
        );
        assert_eq!(
            couple_pointer_options(RecordingToggle::ShowCursor, true, false),
            (true, false)
        );
        assert_eq!(
            couple_pointer_options(RecordingToggle::DesktopAudio, false, true),
            (false, true)
        );
        assert_eq!(FPS_OPTIONS, [60, 30, 15]);
    }

    #[test]
    fn microphone_entries_cover_loading_and_missing_devices() {
        let devices = [("usb", "USB Mic")];
        let entries = microphone_entries(true, true, Some("gone"), &devices);
        let labels: Vec<_> = entries.iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Off",
                "Loading microphones…",
                "Loading microphone…",
                "USB Mic"
            ]
        );
        assert!(!entries[1].enabled);
        assert_eq!(
            microphone_selected_label(true, false, Some("gone"), &devices),
            "Selected microphone"
        );
        assert_eq!(
            microphone_selected_label(true, false, Some("usb"), &devices),
            "USB Mic"
        );
        assert_eq!(
            microphone_selected_label(false, false, None, &devices),
            "Unavailable"
        );
        assert_eq!(microphone_selected_label(true, true, None, &[]), "Off");
    }

    #[test]
    fn guidance_copy_and_pointer_hysteresis_match_shipping() {
        assert_eq!(
            guidance(GuidanceTarget::Region, false),
            Guidance {
                title: "Drag to select a region",
                hint: "Shift for square · Esc to cancel"
            }
        );
        assert_eq!(
            guidance(GuidanceTarget::Region, true).title,
            "Click and drag to select a region"
        );
        assert_eq!(
            guidance(GuidanceTarget::Window, false).title,
            "Select a window to continue"
        );
        assert_eq!(
            guidance(GuidanceTarget::Display, false),
            Guidance {
                title: "Click to capture this display",
                hint: "Esc to cancel"
            }
        );
        let over = |x, current| pointer_over_guidance(x, 50., 100., 40., 300., 90., current);
        assert!(!over(71., false));
        assert!(over(72., false));
        assert!(over(61., true), "leave slack keeps a faded chip hidden");
        assert!(!over(59., true));
    }

    /// The shipping `shared/design.css` motion tokens the chip uses.
    struct Shipping;
    impl MotionTokens for Shipping {
        fn duration_ms(&self, token: &str) -> Option<f64> {
            Some(match token {
                "dur-3" => 200.,
                "dur-4" => 280.,
                _ => return None,
            })
        }
        fn easing(&self, token: &str) -> Option<[f64; 4]> {
            Some(match token {
                "ease-out" => [0.16, 1., 0.3, 1.],
                "ease-standard" => [0.2, 0.8, 0.2, 1.],
                _ => return None,
            })
        }
    }

    #[test]
    fn guidance_chip_sits_at_sixteen_percent_with_direct_confirm_hint() {
        assert_eq!(guidance_top(900.), 144.);
        assert_eq!(GUIDANCE_FEEDBACK, Duration::from_millis(1_800));
        assert_eq!(
            direct_hint(GuidanceTarget::Region, false),
            "Shift for square · Esc to cancel"
        );
        assert_eq!(
            direct_hint(GuidanceTarget::Window, true),
            "Esc to cancel · Press Enter to confirm"
        );
    }

    #[test]
    fn guidance_chip_fades_in_ducks_and_nudges_like_shipping() {
        let mut chip = GuidanceChip::new(&Shipping, false, 1_000., true, false).unwrap();
        let start = chip.pose(1_000.);
        assert_eq!(
            (
                start.opacity,
                start.translate_x,
                start.translate_y,
                start.accent
            ),
            (0., 0., GUIDANCE_ENTER_OFFSET, false),
            "mounts transparent and 6 points high"
        );
        assert!(chip.animating(1_100.));
        let mid = chip.pose(1_100.);
        assert!(mid.opacity > 0. && mid.opacity < 1. && mid.translate_y < 0.);
        let rest = chip.pose(1_200.);
        assert_eq!((rest.opacity, rest.translate_y), (1., 0.));
        assert!(!chip.animating(1_200.), "an idle chip schedules nothing");

        // Pointer nearby: fade out in place, then back from mid-flight.
        chip.set_shown(false, 2_000.);
        assert!(chip.animating(2_050.));
        let ducking = chip.pose(2_050.).opacity;
        assert!(ducking > 0. && ducking < 1.);
        assert_eq!(chip.pose(2_050.).translate_y, 0., "ducking does not slide");
        assert_eq!(chip.pose(2_200.).opacity, 0.);
        chip.set_shown(true, 2_300.);
        chip.set_shown(false, 2_350.);
        let reversed = chip.pose(2_350.).opacity;
        assert!(
            reversed > 0. && reversed < 1.,
            "retargets from the current value"
        );
        assert!(chip.pose(2_400.).opacity < reversed);

        // An empty click re-keys the chip: entrance again plus the nudge.
        chip.remount(3_000., true, true);
        assert!(chip.feedback() && chip.pose(3_000.).accent);
        assert_eq!(chip.pose(3_000.).opacity, 0.);
        assert!((chip.pose(3_070.).translate_x + 8.).abs() < 1e-6);
        assert!((chip.pose(3_210.).translate_x - 8.).abs() < 1e-6);
        assert!(chip.animating(3_250.), "the nudge outlasts the fade");
        assert_eq!(chip.pose(3_280.).translate_x, 0.);
        assert!(!chip.animating(3_280.));
        chip.remount(4_800., true, false);
        assert!(!chip.pose(4_800.).accent);
    }

    #[test]
    fn guidance_chip_lands_at_once_under_reduced_motion() {
        let mut chip = GuidanceChip::new(&Shipping, true, 0., true, true).unwrap();
        let pose = chip.pose(0.);
        assert_eq!(
            (
                pose.opacity,
                pose.translate_x,
                pose.translate_y,
                pose.accent
            ),
            (1., 0., 0., true)
        );
        assert!(!chip.animating(0.));
        chip.set_shown(false, 10.);
        assert_eq!(chip.pose(10.).opacity, 0.);
        assert!(!chip.animating(10.));
        let hidden = GuidanceChip::new(&Shipping, true, 0., false, false).unwrap();
        assert_eq!(
            hidden.pose(0.).opacity,
            0.,
            "mounting under the pointer stays hidden"
        );
    }

    #[test]
    fn display_identity_uses_os_name_and_record_fps() {
        assert_eq!(
            display_identity(" Studio Display ", 2560, 1440, None),
            DisplayIdentity {
                name: "Studio Display".into(),
                detail: "2560 × 1440".into()
            }
        );
        assert_eq!(
            display_identity("", 1920, 1080, Some(60)),
            DisplayIdentity {
                name: "Display".into(),
                detail: "1920 × 1080 · 60 FPS".into()
            }
        );
    }
}
