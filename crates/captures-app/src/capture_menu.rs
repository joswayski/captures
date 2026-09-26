//! Shipping New Capture menu copy and small presentation policies, ported from
//! the Tauri `RecordingSelector`, `CaptureGuidance` and
//! `CaptureSelectorVisibilityNote` (`apps/desktop/ui/src/App.tsx`) so the AppKit
//! and wgpu hosts render the same labels, states and pointer behavior.
//!
//! Everything here is pure: hosts own layout, drawing and input.

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
