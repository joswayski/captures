use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub settings_schema_version: u8,
    pub appearance: String,
    pub theme: String,
    pub custom_theme: CustomTheme,
    pub output_directory: String,
    pub new_capture_shortcut: String,
    pub region_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
    pub auto_copy_to_clipboard: bool,
    pub auto_start_on_selection: bool,
    pub show_mini_previews: bool,
    pub mini_preview_placement: String,
    pub include_mini_previews_in_captures: bool,
    pub include_recording_controls_in_captures: bool,
    pub launch_at_login: bool,
    pub last_screen_permission_request_id: Option<String>,
    pub pending_capture_after_restart: Option<serde_json::Value>,
    pub onboarding_completed: bool,
    pub screenshot_countdown_seconds: u8,
    pub freeze_screen: bool,
    pub show_cursor_in_screenshots: bool,
    pub screenshot_format: String,
    pub show_update_changelog: bool,
    pub recording: RecordingSettings,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CustomTheme {
    pub accent: String,
    pub signal: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RecordingSettings {
    pub video_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
    pub gif_shortcut: String,
    pub video_format: String,
    pub video_fps: u16,
    pub video_max_resolution: String,
    pub gif_fps: u16,
    pub gif_max_width: u32,
    pub gif_max_colors: u16,
    pub countdown_seconds: u8,
    pub show_cursor: bool,
    pub capture_system_audio: bool,
    pub microphone_device_id: Option<String>,
    pub mono_audio: bool,
    pub highlight_clicks: bool,
    pub show_keystrokes: bool,
    pub open_editor_after_recording: bool,
}
impl Default for CustomTheme {
    fn default() -> Self {
        Self {
            accent: "#32d3ff".into(),
            signal: "#ff4fc3".into(),
        }
    }
}
impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            video_shortcut: default_video_shortcut(),
            window_shortcut: format!("{}+Shift+Alt+W", shortcut_modifier()),
            display_shortcut: format!("{}+Shift+Alt+3", shortcut_modifier()),
            gif_shortcut: format!("{}+Shift+6", shortcut_modifier()),
            video_format: "mp4".into(),
            video_fps: 60,
            video_max_resolution: "original".into(),
            gif_fps: 15,
            gif_max_width: 800,
            gif_max_colors: 256,
            countdown_seconds: 3,
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
impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_schema_version: 5,
            appearance: "system".into(),
            theme: "mustard".into(),
            custom_theme: Default::default(),
            output_directory: default_output_directory(),
            new_capture_shortcut: default_new_capture_shortcut(),
            region_shortcut: default_region_shortcut(),
            window_shortcut: default_window_shortcut(),
            display_shortcut: default_display_shortcut(),
            auto_copy_to_clipboard: true,
            auto_start_on_selection: false,
            show_mini_previews: true,
            mini_preview_placement: "bottom_left".into(),
            include_mini_previews_in_captures: false,
            include_recording_controls_in_captures: false,
            launch_at_login: false,
            last_screen_permission_request_id: None,
            pending_capture_after_restart: None,
            onboarding_completed: false,
            screenshot_countdown_seconds: 0,
            freeze_screen: true,
            show_cursor_in_screenshots: true,
            screenshot_format: "png".into(),
            show_update_changelog: true,
            recording: Default::default(),
        }
    }
}

fn shortcut_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "Command"
    } else {
        "Control"
    }
}
fn default_output_directory() -> String {
    std::env::var_os(if cfg!(target_os = "windows") {
        "USERPROFILE"
    } else {
        "HOME"
    })
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("."))
    .join("Pictures")
    .join("Captures")
    .to_string_lossy()
    .into_owned()
}
fn default_new_capture_shortcut() -> String {
    if cfg!(target_os = "linux") {
        "PrintScreen".into()
    } else {
        format!("{}+Shift+Space", shortcut_modifier())
    }
}
fn default_region_shortcut() -> String {
    if cfg!(target_os = "macos") {
        "Command+Shift+4".into()
    } else {
        "Super+Shift+S".into()
    }
}
fn default_window_shortcut() -> String {
    if cfg!(target_os = "macos") {
        "Command+Shift+W".into()
    } else {
        "Alt+PrintScreen".into()
    }
}
fn default_display_shortcut() -> String {
    if cfg!(target_os = "macos") {
        "Command+Shift+3".into()
    } else if cfg!(target_os = "windows") {
        "PrintScreen".into()
    } else {
        "Shift+PrintScreen".into()
    }
}
fn default_video_shortcut() -> String {
    if cfg!(target_os = "macos") {
        "Command+Shift+5".into()
    } else if cfg!(target_os = "windows") {
        "Super+Alt+R".into()
    } else {
        "Control+Shift+Alt+R".into()
    }
}

pub fn path(profile: &Path) -> PathBuf {
    profile.join("settings.json")
}
pub fn load(profile: &Path) -> Result<Settings> {
    let p = path(profile);
    if !p.exists() {
        return Ok(Settings::default());
    }
    let bytes = fs::read(&p).with_context(|| format!("read {}", p.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", p.display()))
}
pub fn save(profile: &Path, value: &Settings) -> Result<()> {
    fs::create_dir_all(profile)?;
    let target = path(profile);
    let mut tmp = tempfile::NamedTempFile::new_in(profile)?;
    serde_json::to_writer_pretty(tmp.as_file_mut(), value)?;
    tmp.as_file_mut().sync_all()?;
    tmp.persist(&target)
        .map_err(|error| error.error)
        .with_context(|| format!("replace {}", target.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_round_trip_and_use_shipping_keys() {
        let d = tempfile::tempdir().unwrap();
        let s = Settings::default();
        save(d.path(), &s).unwrap();
        assert_eq!(load(d.path()).unwrap(), s);
        let v: serde_json::Value =
            serde_json::from_slice(&fs::read(path(d.path())).unwrap()).unwrap();
        assert!(v.get("auto_copy_to_clipboard").is_some());
    }
    #[test]
    fn malformed_file_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        fs::write(path(d.path()), "{").unwrap();
        assert!(load(d.path()).is_err());
        assert_eq!(fs::read_to_string(path(d.path())).unwrap(), "{");
        assert_eq!(
            load(d.path()).unwrap_err().root_cause().to_string(),
            "EOF while parsing an object at line 1 column 1"
        );
    }

    #[test]
    fn defaults_match_shipping_linux_shortcuts_and_recording_quality() {
        let settings = Settings::default();
        if cfg!(target_os = "linux") {
            assert_eq!(settings.new_capture_shortcut, "PrintScreen");
            assert_eq!(settings.region_shortcut, "Super+Shift+S");
        }
        assert_eq!(settings.recording.video_fps, 60);
        assert_eq!(settings.recording.gif_fps, 15);
        assert_eq!(settings.recording.gif_max_colors, 256);
    }
}
