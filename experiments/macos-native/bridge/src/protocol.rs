use std::path::PathBuf;

use captures_capture::LogicalRect;
use captures_media::{CropRect, ExportFormat, QualityPreset};
use captures_recording::RecordingOptions;
use serde::Deserialize;
use serde_json::{Value, json};

pub type BridgeResult<T> = Result<T, String>;

#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub op: String,
}

#[derive(Debug, Deserialize)]
pub struct DescribeRequest {
    #[serde(default)]
    pub request_permission: bool,
}

#[derive(Debug, Deserialize)]
pub struct MicrophonePermissionRequest {
    #[serde(default)]
    pub request: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScreenshotTarget {
    Display {
        display_id: String,
    },
    Region {
        display_id: String,
        rect: LogicalRect,
    },
    Window {
        window_id: String,
    },
    FrozenRegion {
        freeze_id: String,
        rect: LogicalRect,
    },
}

#[derive(Debug, Deserialize)]
pub struct FreezeCreateRequest {
    pub display_id: String,
    #[serde(default)]
    pub cursor: bool,
}

#[derive(Debug, Deserialize)]
pub struct ScreenshotRequest {
    pub target: ScreenshotTarget,
    #[serde(default)]
    pub cursor: bool,
    pub output_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct RecordStartRequest {
    pub options: RecordingOptions,
    #[serde(default)]
    pub exclude_app: bool,
    pub output_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct RecordMuteRequest {
    pub muted: bool,
}

#[derive(Debug, Deserialize)]
pub struct MediaPathRequest {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct MediaExportRequest {
    pub path: PathBuf,
    pub output: PathBuf,
    pub format: ExportFormat,
    pub start_ms: u64,
    pub end_ms: u64,
    pub crop: Option<CropRect>,
    pub width: Option<u32>,
    pub fps: Option<u16>,
    #[serde(default)]
    pub quality: QualityPreset,
    pub max_bytes: Option<u64>,
    #[serde(default = "one")]
    pub system_volume: f32,
    #[serde(default = "one")]
    pub microphone_volume: f32,
    #[serde(default)]
    pub mono: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
}

#[derive(Debug, Deserialize)]
pub struct ImageEncodeRequest {
    pub path: PathBuf,
    pub output: PathBuf,
    pub format: ImageFormat,
    pub quality: Option<u8>,
    pub max_bytes: Option<u64>,
}

const fn one() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct RecoverRequest {
    pub id: String,
    pub output_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct RecoverDiscardRequest {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct FreezeDiscardRequest {
    pub id: String,
}

pub fn parse<T: for<'de> Deserialize<'de>>(value: &Value) -> BridgeResult<T> {
    serde_json::from_value(value.clone()).map_err(|error| format!("invalid request: {error}"))
}

pub fn success_json(value: Value) -> String {
    json!({ "ok": true, "value": value }).to_string()
}

pub fn failure_json(error: &str) -> String {
    json!({ "ok": false, "error": error }).to_string()
}
