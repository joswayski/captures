use serde::{Deserialize, Serialize};
use thiserror::Error;

const TARGET_HEADROOM_PERCENT: u64 = 5;
pub(crate) const MIN_VIDEO_BITRATE: u64 = 250_000;
pub(crate) const MIN_AUDIO_BITRATE: u64 = 64_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Screenshot,
    Video,
    Gif,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MediaMetadata {
    pub kind: MediaKind,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u64>,
    pub size_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(C)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CropResizeAxis {
    Width = 0,
    Height = 1,
}

impl TryFrom<u8> for CropResizeAxis {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Width),
            1 => Ok(Self::Height),
            _ => Err(()),
        }
    }
}

impl CropRect {
    /// Resize one numeric crop dimension while preserving the current ratio and
    /// fitting the coupled dimensions inside the source from this crop's origin.
    #[must_use]
    pub fn resize_aspect_locked(
        mut self,
        source_width: u32,
        source_height: u32,
        axis: CropResizeAxis,
        value: u32,
    ) -> Self {
        let ratio = f64::from(self.width) / f64::from(self.height.max(1));
        let max_width = source_width.saturating_sub(self.x).max(2);
        let max_height = source_height.saturating_sub(self.y).max(2);
        match axis {
            CropResizeAxis::Width => {
                self.width = value.clamp(2, max_width);
                self.height = ((f64::from(self.width) / ratio).round() as u32).max(2);
                if self.height > max_height {
                    self.height = max_height;
                    self.width = ((f64::from(self.height) * ratio).round() as u32).max(2);
                }
            }
            CropResizeAxis::Height => {
                self.height = value.clamp(2, max_height);
                self.width = ((f64::from(self.height) * ratio).round() as u32).max(2);
                if self.width > max_width {
                    self.width = max_width;
                    self.height = ((f64::from(self.width) / ratio).round() as u32).max(2);
                }
            }
        }
        self
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AudioEdit {
    pub system_volume: f32,
    pub microphone_volume: f32,
    pub mute_system_audio: bool,
    pub mute_microphone: bool,
    pub mono_output: bool,
    #[serde(default)]
    pub source_has_system_audio: bool,
    #[serde(default)]
    pub source_has_microphone_audio: bool,
}

impl Default for AudioEdit {
    fn default() -> Self {
        Self {
            system_volume: 1.0,
            microphone_volume: 1.0,
            mute_system_audio: false,
            mute_microphone: false,
            mono_output: false,
            source_has_system_audio: false,
            source_has_microphone_audio: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EditSpec {
    pub trim_start_ms: u64,
    pub trim_end_ms: Option<u64>,
    pub crop: Option<CropRect>,
    pub output_width: Option<u32>,
    pub output_height: Option<u32>,
    #[serde(default)]
    pub audio: AudioEdit,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Mp4,
    Gif,
    #[serde(rename = "webm", alias = "web_m")]
    WebM,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    #[default]
    Preserve,
    /// Lightest compress preset (near-original quality, modest size cut).
    Highest,
    High,
    Standard,
    Small,
    /// Strongest compress preset (smallest file, most visible compression).
    Tiny,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExportSpec {
    pub format: ExportFormat,
    #[serde(default)]
    pub quality: QualityPreset,
    pub max_size_bytes: Option<u64>,
    pub frames_per_second: Option<u16>,
    pub gif_max_colors: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportEstimate {
    pub size_bytes: u64,
    /// True when the whole trimmed range was encoded or the source is copied.
    pub exact: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStage {
    Preparing,
    Encoding,
    Verifying,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportProgress {
    pub stage: ExportStage,
    pub completed_per_mille: u16,
    pub attempt: u8,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SizeBudget {
    pub requested_bytes: u64,
    pub encoding_target_bytes: u64,
    pub video_bitrate: u64,
    pub audio_bitrate: u64,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SizeBudgetError {
    #[error("recording duration must be greater than zero")]
    EmptyDuration,
    #[error("the requested size cannot fit the minimum supported video and audio quality")]
    Unattainable,
}

pub fn calculate_size_budget(
    requested_bytes: u64,
    duration_ms: u64,
    has_audio: bool,
) -> Result<SizeBudget, SizeBudgetError> {
    if duration_ms == 0 {
        return Err(SizeBudgetError::EmptyDuration);
    }
    let encoding_target_bytes = requested_bytes.saturating_mul(100 - TARGET_HEADROOM_PERCENT) / 100;
    let total_bitrate = encoding_target_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        / duration_ms;
    let audio_bitrate = if has_audio { 128_000 } else { 0 };
    let minimum_audio_bitrate = if has_audio { MIN_AUDIO_BITRATE } else { 0 };
    if total_bitrate <= MIN_VIDEO_BITRATE.saturating_add(minimum_audio_bitrate) {
        return Err(SizeBudgetError::Unattainable);
    }
    let audio_bitrate = audio_bitrate.min(total_bitrate.saturating_sub(MIN_VIDEO_BITRATE));
    let video_bitrate = total_bitrate.saturating_sub(audio_bitrate);
    Ok(SizeBudget {
        requested_bytes,
        encoding_target_bytes,
        video_bitrate,
        audio_bitrate,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GifExportAttempt {
    pub max_colors: u16,
    pub frames_per_second: u16,
    pub max_width: u32,
}

pub fn gif_export_attempts(
    initial_colors: u16,
    initial_fps: u16,
    initial_width: u32,
) -> Vec<GifExportAttempt> {
    let colors = [initial_colors.clamp(64, 256), 128, 96, 64];
    let frames = [initial_fps.clamp(8, 30), 12, 10, 8];
    let widths = [initial_width.max(320), 640, 480, 320];
    let mut attempts = Vec::new();
    for max_colors in colors {
        for frames_per_second in frames {
            for max_width in widths {
                let attempt = GifExportAttempt {
                    max_colors,
                    frames_per_second,
                    max_width: max_width.min(initial_width.max(320)),
                };
                if attempts.last() != Some(&attempt) {
                    attempts.push(attempt);
                }
            }
        }
    }
    attempts
}

/// Longest trimmed duration that a size estimate encodes in full, which makes
/// the estimate exact instead of extrapolated.
pub const ESTIMATE_FULL_ENCODE_MAX_MS: u64 = 6_000;
/// Duration of each sampled window when the trimmed range is too long to
/// encode in full for an estimate.
pub const ESTIMATE_SAMPLE_WINDOW_MS: u64 = 2_000;

/// Sample windows `(start_ms, duration_ms)` inside the trimmed range used to
/// estimate an export's size without encoding the whole recording. Short
/// recordings return one window covering the full range.
pub fn estimate_sample_windows(trim_start_ms: u64, trimmed_duration_ms: u64) -> Vec<(u64, u64)> {
    if trimmed_duration_ms == 0 {
        return Vec::new();
    }
    if trimmed_duration_ms <= ESTIMATE_FULL_ENCODE_MAX_MS {
        return vec![(trim_start_ms, trimmed_duration_ms)];
    }
    let latest_start = trimmed_duration_ms - ESTIMATE_SAMPLE_WINDOW_MS;
    [25, 65]
        .into_iter()
        .map(|percent| {
            let offset = (trimmed_duration_ms * percent / 100).min(latest_start);
            (trim_start_ms + offset, ESTIMATE_SAMPLE_WINDOW_MS)
        })
        .collect()
}

/// Scale sampled output bytes up to the full trimmed duration.
pub fn extrapolate_sampled_size(sampled_bytes: u64, sampled_ms: u64, total_ms: u64) -> u64 {
    if sampled_ms == 0 {
        return sampled_bytes;
    }
    u64::try_from(u128::from(sampled_bytes) * u128::from(total_ms) / u128::from(sampled_ms))
        .unwrap_or(u64::MAX)
}

/// Give a sampled window a proportional share of a whole-export size cap so
/// bitrate selection matches the real save.
#[must_use]
pub fn sampled_export_spec(export: &ExportSpec, window_ms: u64, total_ms: u64) -> ExportSpec {
    let mut sample = export.clone();
    sample.max_size_bytes = export.max_size_bytes.map(|cap| {
        u64::try_from(u128::from(cap) * u128::from(window_ms) / u128::from(total_ms.max(1)))
            .unwrap_or(cap)
            .max(1)
    });
    sample
}

#[cfg(test)]
mod tests {
    use super::{
        CropRect, CropResizeAxis, ESTIMATE_SAMPLE_WINDOW_MS, ExportFormat, ExportSpec,
        GifExportAttempt, QualityPreset, SizeBudgetError, calculate_size_budget,
        estimate_sample_windows, extrapolate_sampled_size, gif_export_attempts,
        sampled_export_spec,
    };

    #[test]
    fn aspect_locked_crop_resize_preserves_ratio_rounding_and_origin_bounds() {
        for (source, initial, axis, value, expected) in [
            (
                (320, 180),
                (0, 0, 320, 180),
                CropResizeAxis::Width,
                160,
                (160, 90),
            ),
            (
                (640, 1_440),
                (0, 0, 640, 1_440),
                CropResizeAxis::Height,
                720,
                (320, 720),
            ),
            (
                (320, 180),
                (10, 60, 160, 90),
                CropResizeAxis::Width,
                300,
                (213, 120),
            ),
            (
                (320, 180),
                (200, 6, 80, 60),
                CropResizeAxis::Height,
                170,
                (120, 90),
            ),
            (
                (320, 180),
                (10, 6, 160, 90),
                CropResizeAxis::Width,
                0,
                (2, 2),
            ),
            (
                (320, 180),
                (10, 6, 90, 160),
                CropResizeAxis::Height,
                0,
                (2, 2),
            ),
            (
                (400, 300),
                (20, 30, 101, 61),
                CropResizeAxis::Width,
                73,
                (73, 44),
            ),
            (
                (320, 180),
                (300, 20, 100, 50),
                CropResizeAxis::Width,
                100,
                (20, 10),
            ),
        ] {
            let crop = CropRect {
                x: initial.0,
                y: initial.1,
                width: initial.2,
                height: initial.3,
            }
            .resize_aspect_locked(source.0, source.1, axis, value);
            assert_eq!((crop.width, crop.height), expected);
            assert_eq!((crop.x, crop.y), (initial.0, initial.1));
        }
    }

    #[test]
    fn preserve_quality_is_the_default() {
        assert_eq!(QualityPreset::default(), QualityPreset::Preserve);
    }

    #[test]
    fn reserves_headroom_and_accounts_for_audio() {
        let budget = calculate_size_budget(10_000_000, 60_000, true).expect("size fits");
        assert_eq!(budget.encoding_target_bytes, 9_500_000);
        assert_eq!(budget.audio_bitrate, 128_000);
        assert_eq!(budget.video_bitrate, 1_138_666);
    }

    #[test]
    fn rejects_impossible_size_targets() {
        assert_eq!(
            calculate_size_budget(1_000_000, 120_000, true),
            Err(SizeBudgetError::Unattainable)
        );
        assert_eq!(
            calculate_size_budget(10_000_000, 0, false),
            Err(SizeBudgetError::EmptyDuration)
        );
    }

    #[test]
    fn gif_attempts_end_at_the_defined_quality_floor() {
        let attempts = gif_export_attempts(256, 15, 800);
        assert_eq!(
            attempts.first(),
            Some(&GifExportAttempt {
                max_colors: 256,
                frames_per_second: 15,
                max_width: 800,
            })
        );
        assert_eq!(
            attempts.last(),
            Some(&GifExportAttempt {
                max_colors: 64,
                frames_per_second: 8,
                max_width: 320,
            })
        );
    }

    #[test]
    fn short_recordings_are_estimated_with_one_full_window() {
        assert_eq!(estimate_sample_windows(1_500, 4_000), vec![(1_500, 4_000)]);
        assert_eq!(estimate_sample_windows(0, 0), Vec::new());
    }

    #[test]
    fn long_recordings_sample_two_disjoint_windows_inside_the_trim() {
        let windows = estimate_sample_windows(10_000, 60_000);
        assert_eq!(
            windows,
            vec![
                (25_000, ESTIMATE_SAMPLE_WINDOW_MS),
                (49_000, ESTIMATE_SAMPLE_WINDOW_MS),
            ]
        );
        for (start, duration) in windows {
            assert!(start >= 10_000);
            assert!(start + duration <= 70_000);
        }
    }

    #[test]
    fn sample_windows_never_pass_the_end_of_the_trim() {
        for (start, duration) in estimate_sample_windows(0, 6_001) {
            assert!(start + duration <= 6_001);
        }
    }

    #[test]
    fn sampled_sizes_extrapolate_to_the_full_duration() {
        assert_eq!(
            extrapolate_sampled_size(1_000_000, 4_000, 60_000),
            15_000_000
        );
        assert_eq!(extrapolate_sampled_size(1_000_000, 0, 60_000), 1_000_000);
        assert_eq!(extrapolate_sampled_size(u64::MAX, 1, 2), u64::MAX);
    }

    #[test]
    fn sampled_size_budget_is_proportional_and_never_zero() {
        let export = ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::High,
            max_size_bytes: Some(12_000_000),
            frames_per_second: Some(30),
            gif_max_colors: None,
        };
        assert_eq!(
            sampled_export_spec(&export, 2_000, 60_000).max_size_bytes,
            Some(400_000)
        );
        assert_eq!(
            sampled_export_spec(&export, 1, u64::MAX).max_size_bytes,
            Some(1)
        );
        assert_eq!(
            sampled_export_spec(&export, 2_000, 60_000).quality,
            export.quality
        );
    }
}
