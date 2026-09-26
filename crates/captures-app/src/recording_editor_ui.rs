//! Shared recording-editor copy and presentation that both native hosts render.
//!
//! Ports the shipping Tauri editor's labels and formatting (`App.tsx`
//! `RecordingEditor`, `lib/recordingEditor.ts` and `lib/format.ts`). Everything
//! here is pure: no media, files, host windows or clocks.

use captures_media::{ExportStage, QualityPreset};
use serde::{Deserialize, Serialize};

pub const TITLE_RECORDING: &str = "Edit recording";
pub const TITLE_GIF: &str = "Edit GIF";
pub const GIF_AUDIO_NOTE: &str = "GIFs do not include recorded audio.";
pub const FILENAME_ERROR: &str = "Enter a filename without folders or reserved characters.";
pub const ESTIMATE_HELP: &str = "Estimated saved file size for the current edits and settings";
pub const DELTA_HELP: &str = "Change versus the original recording file";
pub const GIF_FRAME_RATES: [u16; 7] = [8, 10, 12, 15, 20, 24, 30];
pub const GIF_MAXIMUM_WIDTHS: [u32; 5] = [320, 480, 640, 800, 1200];
/// Shipping `.timeline-track` height and handle/label geometry, in points.
pub const TIMELINE_TRACK_HEIGHT: f32 = 76.;

/// Header title: shipping shows "Edit GIF" for GIF sources.
pub fn title(mime_type: &str) -> &'static str {
    let essence = mime_type.split(';').next().unwrap_or_default().trim();
    if essence.eq_ignore_ascii_case("image/gif") {
        TITLE_GIF
    } else {
        TITLE_RECORDING
    }
}

/// Shipping's `.recording-editor-warning` for a source that dropped frames
/// (`toLocaleString` grouping, en-US).
pub fn dropped_frames_warning(dropped_frames: u64) -> Option<String> {
    if dropped_frames == 0 {
        return None;
    }
    let digits = dropped_frames.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    Some(format!(
        "This source dropped {grouped} frame{} during capture. The original timing is preserved.",
        if dropped_frames == 1 { "" } else { "s" }
    ))
}

/// `formatEditorTime`: millisecond precision under a minute, else `m:ss`.
pub fn format_editor_time(milliseconds: u64, duration_ms: u64) -> String {
    let minutes = milliseconds / 60_000;
    let seconds = (milliseconds % 60_000) / 1_000;
    if duration_ms < 60_000 {
        format!("{minutes}:{seconds:02}.{:03}", milliseconds % 1_000)
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TrimSummary {
    /// `0:00.000 – 0:03.000`
    pub range: String,
    /// `0:03.000 selected`
    pub selected: String,
}

/// The timeline card's `.timeline-summary` row.
pub fn trim_summary(start_ms: u64, end_ms: u64, duration_ms: u64) -> TrimSummary {
    let duration = duration_ms.max(1);
    TrimSummary {
        range: format!(
            "{} – {}",
            format_editor_time(start_ms, duration),
            format_editor_time(end_ms, duration)
        ),
        selected: format!(
            "{} selected",
            format_editor_time(end_ms.saturating_sub(start_ms).max(1), duration)
        ),
    }
}

/// `formatFileSize`: decimal units, one decimal below 100.
pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1_000. && unit < UNITS.len() - 1 {
        value /= 1_000.;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    let precision = if value >= 100. { 0 } else { 1 };
    format!("{} {}", to_fixed(value, precision), UNITS[unit])
}

/// JavaScript `Number.prototype.toFixed` for small positive values: rounds the
/// exact binary value half up, where Rust formatting rounds ties to even.
fn to_fixed(value: f64, precision: usize) -> String {
    let exact = format!("{value:.40}");
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let digits = fraction.as_bytes();
    let mut scaled: u64 = whole.parse().unwrap_or(0);
    for index in 0..precision {
        scaled = scaled * 10 + u64::from(digits.get(index).map_or(0, |d| d - b'0'));
    }
    if digits.get(precision).is_some_and(|digit| *digit >= b'5') {
        scaled += 1;
    }
    if precision == 0 {
        return scaled.to_string();
    }
    let divisor = 10_u64.pow(precision as u32);
    format!(
        "{}.{:0precision$}",
        scaled / divisor,
        scaled % divisor,
        precision = precision
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SizeDelta {
    pub percent: i64,
    /// `−38%` / `+12%` (U+2212 minus, like shipping).
    pub label: String,
}

impl SizeDelta {
    pub fn smaller(&self) -> bool {
        self.percent < 0
    }
}

/// `formatFileSizeDelta`: JavaScript `Math.round` (half ties toward +∞), hidden
/// for unknown/zero baselines and rounded-zero changes.
pub fn file_size_delta(estimated_bytes: Option<u64>, original_bytes: u64) -> Option<SizeDelta> {
    let estimated = estimated_bytes?;
    if original_bytes == 0 {
        return None;
    }
    let change = (estimated as f64 / original_bytes as f64 - 1.) * 100.;
    let rounded = change.round();
    let percent = if change < 0. && (change - rounded).abs() == 0.5 {
        rounded + 1.
    } else {
        rounded
    } as i64;
    (percent != 0).then(|| SizeDelta {
        percent,
        label: if percent < 0 {
            format!("−{}%", percent.unsigned_abs())
        } else {
            format!("+{percent}%")
        },
    })
}

/// Host state behind the Save quality card's `Est. size` value.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct EstimateInput {
    #[serde(default)]
    pub estimating: bool,
    /// Staged edits differ from the accepted preview.
    #[serde(default)]
    pub unapplied: bool,
    /// Maximum mode is selected but its typed limit is invalid.
    #[serde(default)]
    pub invalid_maximum: bool,
    /// Accepted Maximum file size, shown instead of an estimate.
    #[serde(default)]
    pub maximum_bytes: Option<u64>,
    #[serde(default)]
    pub estimate_bytes: Option<u64>,
    #[serde(default)]
    pub estimate_exact: bool,
    #[serde(default)]
    pub original_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EstimatePresentation {
    pub label: String,
    pub delta: Option<SizeDelta>,
    /// Muted while a value is pending or unavailable.
    pub muted: bool,
}

pub fn estimate(input: &EstimateInput) -> EstimatePresentation {
    let muted = |label: &str| EstimatePresentation {
        label: label.into(),
        delta: None,
        muted: true,
    };
    if input.estimating {
        return muted("Estimating…");
    }
    if input.invalid_maximum {
        return muted("Enter at least 100 KB");
    }
    if input.unapplied {
        return muted("Apply edits to estimate");
    }
    if let Some(cap) = input.maximum_bytes {
        return EstimatePresentation {
            label: format!("≤ {}", format_file_size(cap)),
            delta: None,
            muted: false,
        };
    }
    match input.estimate_bytes {
        Some(bytes) => EstimatePresentation {
            label: format!(
                "{}{}",
                if input.estimate_exact { "" } else { "≈ " },
                format_file_size(bytes)
            ),
            delta: file_size_delta(Some(bytes), input.original_bytes),
            muted: false,
        },
        None => muted("—"),
    }
}

/// `exportStageLabel`, used when progress carries no message of its own.
pub fn export_stage_label(stage: ExportStage) -> &'static str {
    match stage {
        ExportStage::Preparing => "Preparing…",
        ExportStage::Encoding => "Saving…",
        ExportStage::Verifying => "Checking file size…",
        ExportStage::Cancelled => "Save cancelled.",
        ExportStage::Failed => "Save failed.",
        ExportStage::Complete => "Saved.",
    }
}

/// Shipping's success toast after a save.
pub fn saved_message(gif: bool, size_bytes: u64) -> String {
    format!(
        "{} saved — {}.",
        if gif { "GIF" } else { "Video" },
        format_file_size(size_bytes)
    )
}

/// `recordingFilenameError`: a bare, portable filename stem.
pub fn filename_error(stem: &str) -> Option<&'static str> {
    let trimmed = stem.trim();
    let lower = trimmed.to_ascii_lowercase();
    let base = lower.split('.').next().unwrap_or_default();
    let reserved = matches!(base, "con" | "prn" | "aux" | "nul")
        || ((base.starts_with("com") || base.starts_with("lpt"))
            && base.len() == 4
            && matches!(base.as_bytes()[3], b'1'..=b'9'));
    let invalid = trimmed.is_empty()
        || trimmed != stem
        || trimmed == "."
        || trimmed == ".."
        || trimmed
            .chars()
            .any(|c| (c as u32) < 32 || "<>:\"/\\|?*".contains(c))
        || trimmed.ends_with('.')
        || trimmed.ends_with(' ')
        || reserved;
    invalid.then_some(FILENAME_ERROR)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityMode {
    Preserve,
    Compress,
    Maximum,
}

impl QualityMode {
    pub fn of(quality: QualityPreset, maximum: bool) -> Self {
        if maximum {
            Self::Maximum
        } else if quality == QualityPreset::Preserve {
            Self::Preserve
        } else {
            Self::Compress
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Preserve => "Preserve quality",
            Self::Compress => "Compress",
            Self::Maximum => "Maximum file size",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Preserve => {
                "Original quality with no extra compression unless an edit requires it."
            }
            Self::Compress => "Choose a smaller file with Tiny through Highest quality presets.",
            Self::Maximum => "Set a hard size limit for the saved file.",
        }
    }

    /// Shipping offers Preserve quality only for MP4.
    pub fn available(gif: bool) -> &'static [Self] {
        if gif {
            &[Self::Compress, Self::Maximum]
        } else {
            &[Self::Preserve, Self::Compress, Self::Maximum]
        }
    }
}

/// Compress presets in shipping menu order (smallest first).
pub const COMPRESS_PRESETS: [QualityPreset; 5] = [
    QualityPreset::Tiny,
    QualityPreset::Small,
    QualityPreset::Standard,
    QualityPreset::High,
    QualityPreset::Highest,
];

/// The preset Compress starts from, as in shipping.
pub const DEFAULT_COMPRESS_PRESET: QualityPreset = QualityPreset::Highest;

pub fn quality_label(quality: QualityPreset) -> &'static str {
    match quality {
        QualityPreset::Preserve => "Preserve quality",
        QualityPreset::Highest => "Highest",
        QualityPreset::High => "High",
        QualityPreset::Standard => "Balanced",
        QualityPreset::Small => "Smaller",
        QualityPreset::Tiny => "Tiny",
    }
}

pub fn quality_description(quality: QualityPreset) -> &'static str {
    match quality {
        QualityPreset::Preserve => QualityMode::Preserve.description(),
        QualityPreset::Highest => "Light compression. Near-original quality, a modest size cut.",
        QualityPreset::High => "Much smaller file with little visible quality loss.",
        QualityPreset::Standard => "Good quality with a meaningfully smaller file.",
        QualityPreset::Small => "Very small file with more visible compression.",
        QualityPreset::Tiny => "Smallest file with the most visible compression.",
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionChoice {
    Original,
    P1080,
    P720,
    Custom,
}

impl ResolutionChoice {
    pub const ALL: [Self; 4] = [Self::Original, Self::P1080, Self::P720, Self::Custom];

    /// `Original — 1920 × 1080` carries the current crop/output base.
    pub fn label(self, base_width: u32, base_height: u32) -> String {
        match self {
            Self::Original => format!("Original — {base_width} × {base_height}"),
            Self::P1080 => "1080p maximum".into(),
            Self::P720 => "720p maximum".into(),
            Self::Custom => "Custom".into(),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Original => "Keep the recording’s pixel dimensions.",
            Self::P1080 => "Scale down so the video is at most 1080 pixels tall.",
            Self::P720 => "Scale down so the video is at most 720 pixels tall.",
            Self::Custom => "Choose exact pixel dimensions.",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub description: String,
}

fn quality_value(quality: QualityPreset) -> &'static str {
    match quality {
        QualityPreset::Preserve => "preserve",
        QualityPreset::Highest => "highest",
        QualityPreset::High => "high",
        QualityPreset::Standard => "standard",
        QualityPreset::Small => "small",
        QualityPreset::Tiny => "tiny",
    }
}

/// Every menu the hosts build, with shipping labels and descriptions.
#[derive(Clone, Debug, Serialize)]
pub struct Menus {
    pub quality_modes: Vec<Choice>,
    pub quality_presets: Vec<Choice>,
    pub resolutions: Vec<Choice>,
    pub gif_frame_rates: Vec<Choice>,
    pub gif_maximum_widths: Vec<Choice>,
}

pub fn menus(gif: bool, base_width: u32, base_height: u32) -> Menus {
    Menus {
        quality_modes: QualityMode::available(gif)
            .iter()
            .map(|mode| Choice {
                value: serde_json::to_value(mode)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                label: mode.label().into(),
                description: mode.description().into(),
            })
            .collect(),
        quality_presets: COMPRESS_PRESETS
            .iter()
            .map(|&quality| Choice {
                value: quality_value(quality).into(),
                label: quality_label(quality).into(),
                description: quality_description(quality).into(),
            })
            .collect(),
        resolutions: ResolutionChoice::ALL
            .iter()
            .map(|choice| Choice {
                value: serde_json::to_value(choice)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                label: choice.label(base_width, base_height),
                description: choice.description().into(),
            })
            .collect(),
        gif_frame_rates: GIF_FRAME_RATES
            .iter()
            .map(|fps| Choice {
                value: fps.to_string(),
                label: format!("{fps} FPS"),
                description: String::new(),
            })
            .collect(),
        gif_maximum_widths: GIF_MAXIMUM_WIDTHS
            .iter()
            .map(|width| Choice {
                value: width.to_string(),
                label: format!("{width} px"),
                description: String::new(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_times_follow_shipping_formatting() {
        assert_eq!(title("video/mp4"), "Edit recording");
        assert_eq!(title("image/gif"), "Edit GIF");
        assert_eq!(title("IMAGE/GIF; charset=binary"), "Edit GIF");
        assert_eq!(format_editor_time(0, 3_000), "0:00.000");
        assert_eq!(format_editor_time(61_234, 59_999), "1:01.234");
        assert_eq!(format_editor_time(61_234, 60_000), "1:01");
        let summary = trim_summary(250, 2_750, 3_000);
        assert_eq!(summary.range, "0:00.250 – 0:02.750");
        assert_eq!(summary.selected, "0:02.500 selected");
        assert_eq!(trim_summary(5, 5, 0).selected, "0:00.001 selected");
        assert_eq!(dropped_frames_warning(0), None);
        assert_eq!(
            dropped_frames_warning(1).as_deref(),
            Some("This source dropped 1 frame during capture. The original timing is preserved.")
        );
        assert_eq!(
            dropped_frames_warning(1_234_567).as_deref(),
            Some(
                "This source dropped 1,234,567 frames during capture. The original timing is preserved."
            )
        );
    }

    #[test]
    fn file_sizes_and_deltas_match_shipping_rounding() {
        for (bytes, label) in [
            (0, "0 B"),
            (999, "999 B"),
            (1_000, "1.0 KB"),
            (12_345, "12.3 KB"),
            (123_456, "123 KB"),
            (10_000_000, "10.0 MB"),
            (123_456_789_012, "123 GB"),
            // toFixed rounds exact ties up; Rust formatting would give 1.2 / 100.
            (1_250, "1.3 KB"),
            (100_500, "101 KB"),
            (99_950, "100.0 KB"),
        ] {
            assert_eq!(format_file_size(bytes), label);
        }
        let label = |estimated, original| file_size_delta(estimated, original).map(|d| d.label);
        assert_eq!(label(Some(400_000), 1_000_000).as_deref(), Some("−60%"));
        assert_eq!(label(Some(1_250_000), 1_000_000).as_deref(), Some("+25%"));
        assert_eq!(label(Some(9), 8).as_deref(), Some("+13%"));
        assert_eq!(label(Some(7), 8).as_deref(), Some("−12%"));
        assert_eq!(label(Some(3), 8).as_deref(), Some("−62%"));
        assert_eq!(label(Some(0), 100).as_deref(), Some("−100%"));
        assert_eq!(label(None, 1_000_000), None);
        assert_eq!(label(Some(1_004_000), 1_000_000), None);
        assert_eq!(label(Some(250_000), 0), None);
        assert!(file_size_delta(Some(7), 8).unwrap().smaller());
    }

    #[test]
    fn estimate_presentation_prioritises_pending_staged_and_maximum_states() {
        let base = EstimateInput {
            estimate_bytes: Some(4_567),
            estimate_exact: true,
            original_bytes: 9_000,
            ..Default::default()
        };
        let shown = estimate(&base);
        assert_eq!(shown.label, "4.6 KB");
        assert_eq!(shown.delta.unwrap().label, "−49%");
        assert!(!shown.muted);
        let approximate = estimate(&EstimateInput {
            estimate_exact: false,
            ..base
        });
        assert_eq!(approximate.label, "≈ 4.6 KB");
        for (input, label) in [
            (
                EstimateInput {
                    estimating: true,
                    ..base
                },
                "Estimating…",
            ),
            (
                EstimateInput {
                    invalid_maximum: true,
                    ..base
                },
                "Enter at least 100 KB",
            ),
            (
                EstimateInput {
                    unapplied: true,
                    ..base
                },
                "Apply edits to estimate",
            ),
            (
                EstimateInput {
                    estimate_bytes: None,
                    ..base
                },
                "—",
            ),
        ] {
            let shown = estimate(&input);
            assert_eq!(shown.label, label);
            assert!(shown.delta.is_none() && shown.muted);
        }
        let capped = estimate(&EstimateInput {
            maximum_bytes: Some(10_000_000),
            ..base
        });
        assert_eq!(capped.label, "≤ 10.0 MB");
        assert!(capped.delta.is_none(), "a cap is not an estimate");
    }

    #[test]
    fn stage_saved_and_filename_copy_match_shipping() {
        assert_eq!(export_stage_label(ExportStage::Encoding), "Saving…");
        assert_eq!(
            export_stage_label(ExportStage::Verifying),
            "Checking file size…"
        );
        assert_eq!(saved_message(false, 1_234_567), "Video saved — 1.2 MB.");
        assert_eq!(saved_message(true, 999), "GIF saved — 999 B.");
        for valid in ["Captures_2026-01-01", "clip.v2", "COM0", "console"] {
            assert_eq!(filename_error(valid), None, "{valid}");
        }
        for invalid in [
            "", " clip", "clip ", "clip.", ".", "..", "a/b", "a\\b", "a:b", "nul", "COM1.txt",
            "lpt9", "tab\t",
        ] {
            assert_eq!(filename_error(invalid), Some(FILENAME_ERROR), "{invalid:?}");
        }
    }

    #[test]
    fn quality_modes_presets_and_menus_follow_shipping() {
        assert_eq!(
            QualityMode::of(QualityPreset::Preserve, false),
            QualityMode::Preserve
        );
        assert_eq!(
            QualityMode::of(QualityPreset::Tiny, false),
            QualityMode::Compress
        );
        assert_eq!(
            QualityMode::of(QualityPreset::Tiny, true),
            QualityMode::Maximum
        );
        let mp4 = menus(false, 1920, 1080);
        assert_eq!(
            mp4.quality_modes
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["preserve", "compress", "maximum"]
        );
        assert_eq!(
            mp4.quality_presets
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["Tiny", "Smaller", "Balanced", "High", "Highest"]
        );
        assert_eq!(mp4.resolutions[0].label, "Original — 1920 × 1080");
        assert_eq!(mp4.resolutions[1].value, "p1080");
        assert_eq!(mp4.gif_frame_rates[3].label, "15 FPS");
        assert_eq!(mp4.gif_maximum_widths[3].label, "800 px");
        let gif = menus(true, 320, 180);
        assert_eq!(gif.quality_modes[0].value, "compress");
    }
}
