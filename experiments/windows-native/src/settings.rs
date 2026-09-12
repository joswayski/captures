use captures_recording::MaxResolution;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: String,
    pub theme: String,
    pub custom_accent: String,
    pub custom_signal: String,
    pub output_directory: PathBuf,
    pub auto_copy_to_clipboard: bool,
    pub auto_start_on_selection: bool,
    pub show_mini_previews: bool,
    pub mini_preview_placement: u8,
    pub include_mini_previews_in_captures: bool,
    pub include_recording_controls_in_captures: bool,
    pub launch_at_login: bool,
    pub screenshot_countdown_seconds: u8,
    pub freeze_screen: bool,
    pub show_cursor_in_screenshots: bool,
    pub screenshot_format: String,
    pub recording: RecordingSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub video_format: String,
    pub video_fps: u16,
    pub video_max_resolution: MaxResolution,
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

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            video_format: "mp4".into(),
            video_fps: 60,
            video_max_resolution: MaxResolution::Original,
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
            appearance: "system".into(),
            theme: "mustard".into(),
            custom_accent: "#32d3ff".into(),
            custom_signal: "#ff4fc3".into(),
            output_directory: data_dir().join("captures"),
            auto_copy_to_clipboard: true,
            auto_start_on_selection: false,
            show_mini_previews: true,
            mini_preview_placement: 0,
            include_mini_previews_in_captures: false,
            include_recording_controls_in_captures: false,
            launch_at_login: false,
            screenshot_countdown_seconds: 0,
            freeze_screen: true,
            show_cursor_in_screenshots: true,
            screenshot_format: "png".into(),
            recording: RecordingSettings::default(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("CAPTURES_WINDOWS_NATIVE_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Captures Windows Native Experiment")
        })
}

impl Settings {
    pub fn load() -> Result<Self, String> {
        let path = data_dir().join("settings.json");
        match fs::read(path) {
            Ok(bytes) => {
                let value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                Self::validate_value(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn validate_value(value: Self) -> Result<Self, String> {
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.output_directory.is_absolute() {
            return Err("Choose an absolute save directory.".into());
        }
        if !["system", "light", "dark"].contains(&self.appearance.as_str()) {
            return Err("Invalid appearance.".into());
        }
        if ![
            "mustard", "ember", "rose", "violet", "cobalt", "aqua", "mint", "mono", "custom",
        ]
        .contains(&self.theme.as_str())
        {
            return Err("Invalid color theme.".into());
        }
        if self.mini_preview_placement > 3
            || self.screenshot_countdown_seconds > 10
            || self.recording.countdown_seconds > 10
        {
            return Err("Invalid placement or countdown.".into());
        }
        if !matches!(self.recording.video_fps, 15 | 30 | 60)
            || !(1..=30).contains(&self.recording.gif_fps)
            || !(64..=256).contains(&self.recording.gif_max_colors)
            || self.recording.gif_max_width < 320
        {
            return Err("Invalid recording limits.".into());
        }
        if !["png", "jpeg", "webp"].contains(&self.screenshot_format.as_str())
            || !["mp4", "gif"].contains(&self.recording.video_format.as_str())
        {
            return Err("Unsupported output format.".into());
        }
        if [self.custom_accent.as_str(), self.custom_signal.as_str()]
            .iter()
            .any(|color| {
                color.len() != 7
                    || !color.starts_with('#')
                    || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            })
        {
            return Err("Theme colors must be #RRGGBB.".into());
        }
        Ok(())
    }

    pub fn save(&self) -> Result<(), String> {
        self.validate()?;
        atomic_write(
            &data_dir().join("settings.json"),
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("settings path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary.write_all(bytes).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_output_and_unsupported_video_rate() {
        let mut settings = Settings {
            output_directory: "relative".into(),
            ..Settings::default()
        };
        assert!(settings.validate().unwrap_err().contains("absolute"));
        settings.output_directory = std::env::temp_dir();
        settings.recording.video_fps = 24;
        assert!(settings.validate().unwrap_err().contains("recording"));
    }

    #[test]
    fn malformed_custom_color_fails_even_when_theme_is_not_custom() {
        let settings = Settings {
            custom_signal: "red".into(),
            ..Settings::default()
        };
        assert!(settings.validate().unwrap_err().contains("#RRGGBB"));
    }
}
