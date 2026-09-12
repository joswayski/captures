//! GPUI-only preferences with an isolated on-disk profile.
//!
//! This intentionally does not read or mutate the shipping Tauri profile or
//! the older GTK experiment's profile.
use captures_recording::MaxResolution;
use gpui::Global;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: String,
    pub theme: String,
    pub custom_accent: String,
    pub custom_signal: String,
    pub output_directory: PathBuf,
    pub new_capture_shortcut: String,
    pub region_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
    pub auto_copy_to_clipboard: bool,
    pub auto_start_on_selection: bool,
    pub show_mini_previews: bool,
    pub mini_preview_placement: u32,
    pub include_mini_previews_in_captures: bool,
    pub include_recording_controls_in_captures: bool,
    pub launch_at_login: bool,
    pub screenshot_countdown_seconds: u8,
    pub freeze_screen: bool,
    pub show_cursor_in_screenshots: bool,
    pub screenshot_format: String,
    pub show_update_changelog: bool,
    pub recording: RecordingSettings,
}

impl Global for Settings {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub video_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
    /// Retained for compatibility with the existing native settings contract.
    pub gif_shortcut: String,
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
        let shortcuts = if cfg!(target_os = "macos") {
            [
                "Super+Shift+5",
                "Super+Shift+Alt+W",
                "Super+Shift+Alt+3",
                "Super+Shift+6",
            ]
        } else if cfg!(target_os = "windows") {
            [
                "Super+Alt+R",
                "Control+Shift+Alt+W",
                "Control+Shift+Alt+3",
                "Control+Shift+6",
            ]
        } else {
            [
                "Control+Shift+Alt+R",
                "Control+Shift+Alt+W",
                "Control+Shift+Alt+3",
                "Control+Shift+6",
            ]
        };
        Self {
            video_shortcut: shortcuts[0].into(),
            window_shortcut: shortcuts[1].into(),
            display_shortcut: shortcuts[2].into(),
            gif_shortcut: shortcuts[3].into(),
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
        let shortcuts = if cfg!(target_os = "macos") {
            [
                "Super+Shift+Space",
                "Super+Shift+4",
                "Super+Shift+W",
                "Super+Shift+3",
            ]
        } else if cfg!(target_os = "windows") {
            [
                "Control+Shift+Space",
                "Super+Shift+S",
                "Alt+PrintScreen",
                "PrintScreen",
            ]
        } else {
            [
                "PrintScreen",
                "Super+Shift+S",
                "Alt+PrintScreen",
                "Shift+PrintScreen",
            ]
        };
        Self {
            appearance: "system".into(),
            theme: "mustard".into(),
            custom_accent: "#32d3ff".into(),
            custom_signal: "#ff4fc3".into(),
            output_directory: data_dir().join("captures"),
            new_capture_shortcut: shortcuts[0].into(),
            region_shortcut: shortcuts[1].into(),
            window_shortcut: shortcuts[2].into(),
            display_shortcut: shortcuts[3].into(),
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
            show_update_changelog: true,
            recording: RecordingSettings::default(),
        }
    }
}

/// Directory for the isolated GPUI profile.
pub fn data_dir() -> PathBuf {
    std::env::var_os("CAPTURES_GPUI_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .expect("the current account must have a local application-data directory")
                .data_local_dir()
                .join("captures-gpui")
        })
}

impl Settings {
    pub fn load() -> Result<Self, String> {
        Self::load_at(&data_dir())
    }

    fn load_at(root: &Path) -> Result<Self, String> {
        match std::fs::read(root.join("settings.json")) {
            Ok(bytes) => Self::from_bytes(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("Could not read GPUI settings: {error}")),
        }
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let settings: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("Could not parse GPUI settings: {error}"))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.output_directory.is_absolute() {
            return Err("Choose an absolute save directory.".into());
        }
        if !["system", "light", "dark"].contains(&self.appearance.as_str()) {
            return Err("Invalid appearance.".into());
        }
        if ![
            "mustard", "ember", "rose", "violet", "cobalt", "aqua", "mint", "lime", "mono",
            "custom",
        ]
        .contains(&self.theme.as_str())
        {
            return Err("Invalid color theme.".into());
        }
        if !is_hex_color(&self.custom_accent) || !is_hex_color(&self.custom_signal) {
            return Err("Custom theme colors must use #RRGGBB values.".into());
        }
        if self.mini_preview_placement > 3
            || self.screenshot_countdown_seconds > 10
            || self.recording.countdown_seconds > 10
        {
            return Err("Capture settings are outside their supported range.".into());
        }
        if !matches!(self.recording.video_fps, 15 | 30 | 60)
            || !(8..=30).contains(&self.recording.gif_fps)
            || self.recording.gif_max_width < 320
            || !(64..=256).contains(&self.recording.gif_max_colors)
        {
            return Err("Recording settings are outside their supported range.".into());
        }
        if !["png", "jpeg", "webp"].contains(&self.screenshot_format.as_str())
            || !["mp4", "gif", "webm"].contains(&self.recording.video_format.as_str())
        {
            return Err("Unsupported output format.".into());
        }

        let shortcuts = [
            &self.new_capture_shortcut,
            &self.region_shortcut,
            &self.window_shortcut,
            &self.display_shortcut,
            &self.recording.video_shortcut,
            &self.recording.window_shortcut,
            &self.recording.display_shortcut,
            &self.recording.gif_shortcut,
        ];
        let parsed = shortcuts
            .iter()
            .map(|shortcut| {
                if shortcut.trim().is_empty() {
                    Err("All shortcuts must be set.".to_owned())
                } else {
                    shortcut
                        .parse::<global_hotkey::hotkey::HotKey>()
                        .map_err(|error| format!("Invalid shortcut {shortcut}: {error}"))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        if parsed
            .iter()
            .enumerate()
            .any(|(index, shortcut)| parsed[index + 1..].contains(shortcut))
        {
            return Err("Shortcuts must be unique.".into());
        }
        Ok(())
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_at(&data_dir())
    }

    fn save_at(&self, root: &Path) -> Result<(), String> {
        self.validate()?;
        atomic_write(
            &root.join("settings.json"),
            &serde_json::to_vec_pretty(self)
                .map_err(|error| format!("Could not encode GPUI settings: {error}"))?,
        )
    }
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing parent directory.")?;
    crate::desktop::private_directory(parent).map_err(|error| error.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    crate::desktop::private_file(file.path()).map_err(|error| error.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "captures-gpui-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn shipping_ranges_and_formats_are_validated_at_boundaries() {
        let mut settings = Settings::default();
        for fps in [15, 30, 60] {
            settings.recording.video_fps = fps;
            assert!(settings.validate().is_ok());
        }
        settings.recording.video_format = "webm".into();
        assert!(settings.validate().is_ok());
        settings.recording.gif_fps = 7;
        assert!(settings.validate().is_err());
        settings.recording.gif_fps = 8;
        settings.recording.gif_max_colors = 63;
        assert!(settings.validate().is_err());
        settings.recording.gif_max_colors = 64;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn duplicate_shortcuts_are_rejected_after_parsing() {
        let mut settings = Settings::default();
        settings.recording.gif_shortcut = settings.region_shortcut.clone();
        assert_eq!(
            settings.validate().unwrap_err(),
            "Shortcuts must be unique."
        );
    }

    #[test]
    fn complete_document_roundtrips_and_atomic_write_leaves_no_staging_file() {
        let mut settings = Settings {
            appearance: "light".into(),
            ..Settings::default()
        };
        settings.recording.microphone_device_id = Some("default".into());
        settings.recording.gif_fps = 24;
        let bytes = serde_json::to_vec_pretty(&settings).unwrap();
        assert_eq!(Settings::from_bytes(&bytes).unwrap(), settings);

        let directory = scratch("settings");
        let path = directory.join("settings.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn save_and_load_persist_the_complete_isolated_document() {
        let directory = scratch("persistence");
        let mut settings = Settings {
            appearance: "dark".into(),
            theme: "cobalt".into(),
            output_directory: directory.join("captures"),
            ..Settings::default()
        };
        settings.recording.video_format = "webm".into();
        settings.recording.gif_fps = 24;
        settings.recording.capture_system_audio = true;

        settings.save_at(&directory).unwrap();
        assert_eq!(Settings::load_at(&directory).unwrap(), settings);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
