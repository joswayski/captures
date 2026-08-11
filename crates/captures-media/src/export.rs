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
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
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
    WebM,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    #[default]
    Preserve,
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

#[cfg(test)]
mod tests {
    use super::{
        GifExportAttempt, QualityPreset, SizeBudgetError, calculate_size_budget,
        gif_export_attempts,
    };

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
}
