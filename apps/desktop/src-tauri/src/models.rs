use std::path::PathBuf;

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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            output_directory: default_output_directory().to_string_lossy().into_owned(),
            region_shortcut: "Ctrl+Shift+4".to_owned(),
            window_shortcut: "Ctrl+Shift+W".to_owned(),
            display_shortcut: "Ctrl+Shift+3".to_owned(),
            launch_at_login: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaptureArtifact {
    pub id: String,
    pub path: String,
    pub preview_url: String,
    pub width: u32,
    pub height: u32,
    pub created_at: String,
    pub mode: CaptureMode,
    pub clipboard_copied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActiveSession {
    pub id: String,
    pub mode: CaptureMode,
    pub display: DisplayDescriptor,
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
        .and_then(|dirs| dirs.picture_dir().map(PathBuf::from))
        .unwrap_or_else(|| {
            ProjectDirs::from("io", "github", "ces")
                .map(|dirs| dirs.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("CES")
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
