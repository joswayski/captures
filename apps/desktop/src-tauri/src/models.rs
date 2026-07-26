use std::path::{Path, PathBuf};

use captures_capture::{CaptureMode, DisplayDescriptor, WindowDescriptor};
use captures_recording::{MaxResolution, RecordingKind, RecordingTarget};
use directories::{ProjectDirs, UserDirs};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const HISTORY_RETENTION_DAYS: i64 = 30;
const CURRENT_SETTINGS_SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppSettings {
    #[serde(default)]
    pub settings_schema_version: u8,
    pub output_directory: String,
    pub region_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
    #[serde(default = "default_auto_copy_to_clipboard")]
    pub auto_copy_to_clipboard: bool,
    pub launch_at_login: bool,
    #[serde(default)]
    pub last_screen_permission_request_id: Option<String>,
    #[serde(default)]
    pub pending_capture_after_restart: Option<CaptureMode>,
    #[serde(default)]
    pub recording: RecordingSettings,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordingSettings {
    #[serde(default = "default_video_shortcut")]
    pub video_shortcut: String,
    #[serde(default = "default_gif_shortcut")]
    pub gif_shortcut: String,
    #[serde(default = "default_video_fps")]
    pub video_fps: u16,
    #[serde(default)]
    pub video_max_resolution: MaxResolution,
    #[serde(default = "default_gif_fps")]
    pub gif_fps: u16,
    #[serde(default = "default_gif_max_width")]
    pub gif_max_width: u32,
    #[serde(default = "default_gif_max_colors")]
    pub gif_max_colors: u16,
    #[serde(default = "default_countdown_seconds")]
    pub countdown_seconds: u8,
    #[serde(default = "default_true")]
    pub show_cursor: bool,
    #[serde(default)]
    pub capture_system_audio: bool,
    #[serde(default)]
    pub microphone_device_id: Option<String>,
    #[serde(default)]
    pub mono_audio: bool,
    #[serde(default)]
    pub highlight_clicks: bool,
    #[serde(default)]
    pub show_keystrokes: bool,
    #[serde(default = "default_true")]
    pub open_editor_after_recording: bool,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            video_shortcut: default_video_shortcut(),
            gif_shortcut: default_gif_shortcut(),
            video_fps: default_video_fps(),
            video_max_resolution: MaxResolution::Original,
            gif_fps: default_gif_fps(),
            gif_max_width: default_gif_max_width(),
            gif_max_colors: default_gif_max_colors(),
            countdown_seconds: default_countdown_seconds(),
            show_cursor: true,
            capture_system_audio: false,
            microphone_device_id: None,
            mono_audio: false,
            highlight_clicks: false,
            show_keystrokes: false,
            open_editor_after_recording: true,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            settings_schema_version: CURRENT_SETTINGS_SCHEMA_VERSION,
            output_directory: default_output_directory().to_string_lossy().into_owned(),
            region_shortcut: "Ctrl+Shift+4".to_owned(),
            window_shortcut: "Ctrl+Shift+W".to_owned(),
            display_shortcut: "Ctrl+Shift+3".to_owned(),
            auto_copy_to_clipboard: true,
            launch_at_login: false,
            last_screen_permission_request_id: None,
            pending_capture_after_restart: None,
            recording: RecordingSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordingSelectionSession {
    pub id: String,
    pub kind: RecordingKind,
    pub display: DisplayDescriptor,
    pub window_coordinate_scale: f64,
    pub snapshot_url: String,
    pub windows: Vec<WindowDescriptor>,
}

#[derive(Debug)]
pub struct RecordingSelection {
    pub summary: RecordingSelectionSession,
    pub image: RgbaImage,
    pub snapshot_png: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordingArtifact {
    pub id: String,
    pub kind: RecordingKind,
    pub path: String,
    pub media_url: String,
    pub poster_url: String,
    pub mime_type: String,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    #[serde(default)]
    pub dropped_frames: u64,
    pub has_system_audio: bool,
    pub has_microphone_audio: bool,
    pub created_at: String,
    pub target: RecordingTarget,
    pub missing: bool,
}

#[derive(Debug)]
pub struct RecordingArtifactData {
    pub summary: RecordingArtifact,
    pub poster_png: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    #[default]
    Screenshot,
    Video,
    Gif,
}

impl From<RecordingKind> for ArtifactKind {
    fn from(kind: RecordingKind) -> Self {
        match kind {
            RecordingKind::Video => Self::Video,
            RecordingKind::Gif => Self::Gif,
        }
    }
}

impl ArtifactKind {
    pub const fn recording_kind(self) -> Option<RecordingKind> {
        match self {
            Self::Screenshot => None,
            Self::Video => Some(RecordingKind::Video),
            Self::Gif => Some(RecordingKind::Gif),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaptureArtifact {
    pub id: String,
    pub path: Option<String>,
    pub preview_url: String,
    pub full_url: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub created_at: String,
    pub mode: CaptureMode,
    pub history_saved: bool,
    pub clipboard_copy_status: ClipboardCopyStatus,
    #[serde(skip)]
    pub image_png: Vec<u8>,
    #[serde(skip)]
    pub preview_png: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HistoryEntry {
    pub id: String,
    #[serde(default)]
    pub kind: ArtifactKind,
    pub preview_url: String,
    pub full_url: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub created_at: String,
    #[serde(default)]
    pub mode: Option<CaptureMode>,
    #[serde(default)]
    pub saved_path: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub target: Option<RecordingTarget>,
    #[serde(default)]
    pub has_system_audio: bool,
    #[serde(default)]
    pub has_microphone_audio: bool,
    #[serde(default)]
    pub dropped_frames: u64,
}

impl HistoryEntry {
    pub fn from_recording(artifact: &RecordingArtifact) -> Self {
        Self {
            id: artifact.id.clone(),
            kind: artifact.kind.into(),
            preview_url: recording_poster_url(&artifact.id),
            full_url: recording_media_url(&artifact.id),
            width: artifact.width,
            height: artifact.height,
            size_bytes: artifact.size_bytes,
            created_at: artifact.created_at.clone(),
            mode: None,
            saved_path: Some(artifact.path.clone()),
            mime_type: Some(artifact.mime_type.clone()),
            duration_ms: Some(artifact.duration_ms),
            target: Some(artifact.target.clone()),
            has_system_audio: artifact.has_system_audio,
            has_microphone_audio: artifact.has_microphone_audio,
            dropped_frames: artifact.dropped_frames,
        }
    }

    pub fn recording_artifact(&self) -> Option<RecordingArtifact> {
        let kind = self.kind.recording_kind()?;
        let path = self.saved_path.clone()?;
        Some(RecordingArtifact {
            id: self.id.clone(),
            kind,
            path: path.clone(),
            media_url: recording_media_url(&self.id),
            poster_url: recording_poster_url(&self.id),
            mime_type: self.mime_type.clone()?,
            duration_ms: self.duration_ms?,
            width: self.width,
            height: self.height,
            size_bytes: self.size_bytes,
            dropped_frames: self.dropped_frames,
            has_system_audio: self.has_system_audio,
            has_microphone_audio: self.has_microphone_audio,
            created_at: self.created_at.clone(),
            target: self.target.clone()?,
            missing: !Path::new(&path).is_file(),
        })
    }

    pub fn summary(&self) -> Option<ArtifactSummary> {
        match self.kind {
            ArtifactKind::Screenshot => Some(ArtifactSummary::Screenshot {
                id: self.id.clone(),
                preview_url: self.preview_url.clone(),
                full_url: self.full_url.clone(),
                width: self.width,
                height: self.height,
                size_bytes: self.size_bytes,
                created_at: self.created_at.clone(),
                mode: self.mode?,
            }),
            ArtifactKind::Video | ArtifactKind::Gif => {
                let artifact = self.recording_artifact()?;
                let fields = RecordingArtifactSummaryFields {
                    id: artifact.id,
                    poster_url: artifact.poster_url,
                    media_url: artifact.media_url,
                    saved_path: artifact.path,
                    mime_type: artifact.mime_type,
                    duration_ms: artifact.duration_ms,
                    width: artifact.width,
                    height: artifact.height,
                    size_bytes: artifact.size_bytes,
                    dropped_frames: artifact.dropped_frames,
                    has_system_audio: artifact.has_system_audio,
                    has_microphone_audio: artifact.has_microphone_audio,
                    created_at: artifact.created_at,
                    target: artifact.target,
                    missing: artifact.missing,
                };
                Some(if self.kind == ArtifactKind::Video {
                    ArtifactSummary::Video { fields }
                } else {
                    ArtifactSummary::Gif { fields }
                })
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingArtifactSummaryFields {
    pub id: String,
    pub poster_url: String,
    pub media_url: String,
    pub saved_path: String,
    pub mime_type: String,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub dropped_frames: u64,
    pub has_system_audio: bool,
    pub has_microphone_audio: bool,
    pub created_at: String,
    pub target: RecordingTarget,
    pub missing: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactSummary {
    Screenshot {
        id: String,
        preview_url: String,
        full_url: String,
        width: u32,
        height: u32,
        size_bytes: u64,
        created_at: String,
        mode: CaptureMode,
    },
    Video {
        #[serde(flatten)]
        fields: RecordingArtifactSummaryFields,
    },
    Gif {
        #[serde(flatten)]
        fields: RecordingArtifactSummaryFields,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardCopyStatus {
    Skipped,
    Pending,
    Copied,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClipboardState {
    pub revision: isize,
    pub artifact_id: Option<String>,
}

const fn default_auto_copy_to_clipboard() -> bool {
    true
}

fn default_video_shortcut() -> String {
    "Ctrl+Shift+5".to_owned()
}

fn default_gif_shortcut() -> String {
    "Ctrl+Shift+6".to_owned()
}

const fn default_video_fps() -> u16 {
    60
}

const fn default_gif_fps() -> u16 {
    15
}

const fn default_gif_max_width() -> u32 {
    800
}

const fn default_gif_max_colors() -> u16 {
    256
}

const fn default_countdown_seconds() -> u8 {
    3
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActiveSession {
    pub id: String,
    pub mode: CaptureMode,
    pub display: DisplayDescriptor,
    pub window_coordinate_scale: f64,
    pub snapshot_url: String,
    pub windows: Vec<WindowDescriptor>,
}

#[derive(Debug)]
pub struct CaptureSession {
    pub id: Uuid,
    pub mode: CaptureMode,
    pub display: DisplayDescriptor,
    pub image: RgbaImage,
    pub snapshot_png: Vec<u8>,
    pub windows: Vec<WindowDescriptor>,
}

pub fn default_output_directory() -> PathBuf {
    UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| {
            ProjectDirs::from("io", "github", "captures")
                .map(|dirs| dirs.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("Captures")
}

pub fn migrate_legacy_output_directory(settings: &mut AppSettings) {
    let Some(user_dirs) = UserDirs::new() else {
        return;
    };
    let Some(pictures) = user_dirs.picture_dir() else {
        return;
    };

    migrate_output_directory(
        settings,
        &pictures.join("Captures"),
        &user_dirs.home_dir().join("Captures"),
    );
}

pub fn migrate_settings(settings: &mut AppSettings) -> bool {
    if settings.settings_schema_version >= CURRENT_SETTINGS_SCHEMA_VERSION {
        return false;
    }

    // Recording originally shipped with 30 FPS / 1080p defaults. Upgrade only
    // values that still match those defaults so an existing custom downgrade
    // remains user-owned.
    if settings.recording.video_fps == 30 {
        settings.recording.video_fps = 60;
    }
    if settings.recording.video_max_resolution == MaxResolution::P1080 {
        settings.recording.video_max_resolution = MaxResolution::Original;
    }
    settings.settings_schema_version = CURRENT_SETTINGS_SCHEMA_VERSION;
    true
}

fn migrate_output_directory(settings: &mut AppSettings, legacy: &Path, current: &Path) {
    if Path::new(&settings.output_directory) == legacy {
        settings.output_directory = current.to_string_lossy().into_owned();
    }
}

pub fn settings_path() -> PathBuf {
    ProjectDirs::from("io", "github", "captures")
        .map(|dirs| dirs.config_dir().join("settings.json"))
        .unwrap_or_else(|| PathBuf::from("settings.json"))
}

pub fn history_directory() -> PathBuf {
    ProjectDirs::from("io", "github", "captures")
        .map(|dirs| dirs.data_local_dir().join("capture-history"))
        .unwrap_or_else(|| PathBuf::from(".captures-history"))
}

pub fn recording_recovery_directory() -> PathBuf {
    ProjectDirs::from("io", "github", "captures")
        .map(|dirs| dirs.data_local_dir().join("recording-recovery"))
        .unwrap_or_else(|| PathBuf::from(".captures-recording-recovery"))
}

pub fn snapshot_url(session_id: &str) -> String {
    capture_asset_url(&format!("session/{session_id}"))
}

#[cfg(target_os = "macos")]
pub fn recording_selection_url(session_id: &str) -> String {
    capture_asset_url(&format!("recording-selection/{session_id}"))
}

pub fn recording_media_url(artifact_id: &str) -> String {
    capture_asset_url(&format!("media/{artifact_id}"))
}

pub fn recording_poster_url(artifact_id: &str) -> String {
    capture_asset_url(&format!("poster/{artifact_id}"))
}

pub fn artifact_url(artifact_id: &str) -> String {
    capture_asset_url(&format!("artifact/{artifact_id}"))
}

pub fn artifact_full_url(artifact_id: &str) -> String {
    capture_asset_url(&format!("artifact-full/{artifact_id}"))
}

pub fn history_preview_url(artifact_id: &str) -> String {
    capture_asset_url(&format!("history-preview/{artifact_id}"))
}

pub fn history_full_url(artifact_id: &str) -> String {
    capture_asset_url(&format!("history-full/{artifact_id}"))
}

fn capture_asset_url(path: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        format!("http://captures-capture.localhost/{path}")
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("captures-capture://localhost/{path}")
    }
}

#[cfg(test)]
mod tests {
    use captures_capture::CaptureMode;
    use captures_recording::{RecordingKind, RecordingTarget};
    use std::path::Path;

    #[cfg(target_os = "macos")]
    use super::recording_selection_url;
    use super::{
        AppSettings, HistoryEntry, RecordingArtifact, migrate_output_directory, migrate_settings,
        recording_media_url, recording_poster_url, snapshot_url,
    };

    #[test]
    fn uses_the_platform_custom_protocol_origin_for_capture_assets() {
        let urls = [
            snapshot_url("session-id"),
            recording_media_url("artifact-id"),
            recording_poster_url("artifact-id"),
        ];

        #[cfg(target_os = "windows")]
        assert!(
            urls.iter()
                .all(|url| url.starts_with("http://captures-capture.localhost/"))
        );

        #[cfg(not(target_os = "windows"))]
        assert!(
            urls.iter()
                .all(|url| url.starts_with("captures-capture://localhost/"))
        );

        #[cfg(target_os = "macos")]
        assert!(
            recording_selection_url("selection-id").starts_with("captures-capture://localhost/")
        );
    }

    #[test]
    fn migrates_only_the_legacy_default_output_directory() {
        let legacy = Path::new("/Users/example/Pictures/Captures");
        let current = Path::new("/Users/example/Captures");
        let mut settings = AppSettings {
            output_directory: legacy.to_string_lossy().into_owned(),
            ..AppSettings::default()
        };

        migrate_output_directory(&mut settings, legacy, current);
        assert_eq!(settings.output_directory, current.to_string_lossy());

        settings.output_directory = "/Volumes/Captures".to_owned();
        migrate_output_directory(&mut settings, legacy, current);
        assert_eq!(settings.output_directory, "/Volumes/Captures");
    }

    #[test]
    fn loads_settings_written_before_permission_tracking() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "output_directory": "/Users/example/Captures",
                "region_shortcut": "Ctrl+Shift+4",
                "window_shortcut": "Ctrl+Shift+W",
                "display_shortcut": "Ctrl+Shift+3",
                "launch_at_login": false
            }"#,
        )
        .expect("legacy settings should deserialize");

        assert!(settings.last_screen_permission_request_id.is_none());
        assert!(settings.pending_capture_after_restart.is_none());
        assert!(settings.auto_copy_to_clipboard);
        assert_eq!(settings.recording.video_fps, 60);
        assert_eq!(
            settings.recording.video_max_resolution,
            captures_recording::MaxResolution::Original
        );
        assert_eq!(settings.recording.gif_fps, 15);
        assert_eq!(settings.recording.gif_max_width, 800);
        assert!(settings.recording.open_editor_after_recording);
    }

    #[test]
    fn upgrades_legacy_recording_quality_defaults_once() {
        let mut settings = AppSettings {
            settings_schema_version: 0,
            ..AppSettings::default()
        };
        settings.recording.video_fps = 30;
        settings.recording.video_max_resolution = captures_recording::MaxResolution::P1080;

        assert!(migrate_settings(&mut settings));
        assert_eq!(settings.recording.video_fps, 60);
        assert_eq!(
            settings.recording.video_max_resolution,
            captures_recording::MaxResolution::Original
        );

        settings.recording.video_fps = 30;
        settings.recording.video_max_resolution = captures_recording::MaxResolution::P1080;
        assert!(!migrate_settings(&mut settings));
        assert_eq!(settings.recording.video_fps, 30);
        assert_eq!(
            settings.recording.video_max_resolution,
            captures_recording::MaxResolution::P1080
        );
    }

    #[test]
    fn preserves_existing_recording_quality_downgrades_during_migration() {
        let mut settings = AppSettings {
            settings_schema_version: 0,
            ..AppSettings::default()
        };
        settings.recording.video_fps = 15;
        settings.recording.video_max_resolution = captures_recording::MaxResolution::P720;

        assert!(migrate_settings(&mut settings));
        assert_eq!(settings.recording.video_fps, 15);
        assert_eq!(
            settings.recording.video_max_resolution,
            captures_recording::MaxResolution::P720
        );
    }

    #[test]
    fn fills_defaults_in_partially_written_recording_settings() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "output_directory": "/Users/example/Captures",
                "region_shortcut": "Ctrl+Shift+4",
                "window_shortcut": "Ctrl+Shift+W",
                "display_shortcut": "Ctrl+Shift+3",
                "launch_at_login": false,
                "recording": { "video_fps": 60, "show_cursor": false }
            }"#,
        )
        .expect("partially written recording settings should deserialize");

        assert_eq!(settings.recording.video_fps, 60);
        assert!(!settings.recording.show_cursor);
        assert_eq!(settings.recording.gif_fps, 15);
        assert_eq!(settings.recording.gif_max_colors, 256);
        assert_eq!(settings.recording.video_shortcut, "Ctrl+Shift+5");
        assert!(settings.recording.open_editor_after_recording);
    }

    #[test]
    fn persists_a_capture_queued_for_permission_restart() {
        let settings = AppSettings {
            pending_capture_after_restart: Some(CaptureMode::Region),
            ..AppSettings::default()
        };

        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let restored: AppSettings =
            serde_json::from_str(&json).expect("settings should deserialize");

        assert_eq!(
            restored.pending_capture_after_restart,
            Some(CaptureMode::Region)
        );
    }

    #[test]
    fn persists_disabled_automatic_clipboard_copying() {
        let settings = AppSettings {
            auto_copy_to_clipboard: false,
            ..AppSettings::default()
        };

        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let restored: AppSettings =
            serde_json::from_str(&json).expect("settings should deserialize");

        assert!(!restored.auto_copy_to_clipboard);
    }

    #[test]
    fn recording_history_serializes_as_a_discriminated_artifact_summary() {
        let artifact = RecordingArtifact {
            id: "8d11b283-3ac8-4510-8780-4910a7ed4305".to_owned(),
            kind: RecordingKind::Video,
            path: "/missing/Captures_recording.mp4".to_owned(),
            media_url: String::new(),
            poster_url: String::new(),
            mime_type: "video/mp4".to_owned(),
            duration_ms: 1_500,
            width: 1_920,
            height: 1_080,
            size_bytes: 42,
            dropped_frames: 3,
            has_system_audio: true,
            has_microphone_audio: true,
            created_at: "2026-07-22T18:00:00Z".to_owned(),
            target: RecordingTarget::Display {
                display_id: "1".to_owned(),
            },
            missing: false,
        };
        let summary = HistoryEntry::from_recording(&artifact)
            .summary()
            .expect("recording history should produce a summary");
        let json = serde_json::to_value(summary).expect("summary should serialize");

        assert_eq!(json["kind"], "video");
        assert_eq!(json["mime_type"], "video/mp4");
        assert_eq!(json["duration_ms"], 1_500);
        assert_eq!(json["dropped_frames"], 3);
        assert_eq!(json["missing"], true);
        assert!(json.get("fields").is_none());
    }
}
