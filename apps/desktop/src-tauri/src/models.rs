use std::path::{Path, PathBuf};

use ces_capture::{CaptureMode, DisplayDescriptor, WindowDescriptor};
use directories::{ProjectDirs, UserDirs};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppSettings {
    pub output_directory: String,
    pub region_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
    pub launch_at_login: bool,
    #[serde(default)]
    pub last_screen_permission_request_id: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            output_directory: default_output_directory().to_string_lossy().into_owned(),
            region_shortcut: "Ctrl+Shift+4".to_owned(),
            window_shortcut: "Ctrl+Shift+W".to_owned(),
            display_shortcut: "Ctrl+Shift+3".to_owned(),
            launch_at_login: false,
            last_screen_permission_request_id: None,
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
    pub clipboard_copied: bool,
    #[serde(skip)]
    pub image_png: Vec<u8>,
    #[serde(skip)]
    pub preview_png: Vec<u8>,
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
            ProjectDirs::from("io", "github", "ces")
                .map(|dirs| dirs.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("CES")
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
        &pictures.join("CES"),
        &user_dirs.home_dir().join("CES"),
    );
}

fn migrate_output_directory(settings: &mut AppSettings, legacy: &Path, current: &Path) {
    if Path::new(&settings.output_directory) == legacy {
        settings.output_directory = current.to_string_lossy().into_owned();
    }
}

pub fn settings_path() -> PathBuf {
    ProjectDirs::from("io", "github", "ces")
        .map(|dirs| dirs.config_dir().join("settings.json"))
        .unwrap_or_else(|| PathBuf::from("settings.json"))
}

pub fn snapshot_url(session_id: &str) -> String {
    format!("ces-capture://localhost/session/{session_id}")
}

pub fn artifact_url(artifact_id: &str) -> String {
    format!("ces-capture://localhost/artifact/{artifact_id}")
}

pub fn artifact_full_url(artifact_id: &str) -> String {
    format!("ces-capture://localhost/artifact-full/{artifact_id}")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{AppSettings, migrate_output_directory};

    #[test]
    fn migrates_only_the_legacy_default_output_directory() {
        let legacy = Path::new("/Users/example/Pictures/CES");
        let current = Path::new("/Users/example/CES");
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
                "output_directory": "/Users/example/CES",
                "region_shortcut": "Ctrl+Shift+4",
                "window_shortcut": "Ctrl+Shift+W",
                "display_shortcut": "Ctrl+Shift+3",
                "launch_at_login": false
            }"#,
        )
        .expect("legacy settings should deserialize");

        assert!(settings.last_screen_permission_request_id.is_none());
    }
}
