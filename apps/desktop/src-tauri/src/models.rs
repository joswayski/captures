use std::path::{Path, PathBuf};

use captures_capture::{CaptureMode, DisplayDescriptor, WindowDescriptor};
use directories::{ProjectDirs, UserDirs};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const HISTORY_RETENTION_DAYS: i64 = 30;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppSettings {
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            output_directory: default_output_directory().to_string_lossy().into_owned(),
            region_shortcut: "Ctrl+Shift+4".to_owned(),
            window_shortcut: "Ctrl+Shift+W".to_owned(),
            display_shortcut: "Ctrl+Shift+3".to_owned(),
            auto_copy_to_clipboard: true,
            launch_at_login: false,
            last_screen_permission_request_id: None,
            pending_capture_after_restart: None,
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
    pub preview_url: String,
    pub full_url: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub created_at: String,
    pub mode: CaptureMode,
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

pub fn snapshot_url(session_id: &str) -> String {
    capture_asset_url(&format!("session/{session_id}"))
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
    use std::path::Path;

    use super::{AppSettings, migrate_output_directory, snapshot_url};

    #[test]
    fn uses_the_platform_custom_protocol_origin_for_capture_images() {
        #[cfg(target_os = "windows")]
        assert_eq!(
            snapshot_url("session-id"),
            "http://captures-capture.localhost/session/session-id"
        );

        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            snapshot_url("session-id"),
            "captures-capture://localhost/session/session-id"
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
}
