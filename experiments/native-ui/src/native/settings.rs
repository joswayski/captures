//! Native settings mirror the shipping preference contract, with isolated storage.
use captures_recording::MaxResolution;
use serde::{Deserialize, Serialize};
use std::{
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub video_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
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
        Self {
            video_shortcut: if cfg!(target_os = "windows") {
                "Super+Alt+R"
            } else {
                "Control+Shift+Alt+R"
            }
            .into(),
            window_shortcut: "Control+Shift+Alt+W".into(),
            display_shortcut: "Control+Shift+Alt+3".into(),
            gif_shortcut: "Control+Shift+6".into(),
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
            new_capture_shortcut: if cfg!(target_os = "windows") {
                "Control+Shift+Space"
            } else {
                "PrintScreen"
            }
            .into(),
            region_shortcut: "Super+Shift+S".into(),
            window_shortcut: "Alt+PrintScreen".into(),
            display_shortcut: if cfg!(target_os = "windows") {
                "PrintScreen"
            } else {
                "Shift+PrintScreen"
            }
            .into(),
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

pub fn data_dir() -> PathBuf {
    std::env::var_os("CAPTURES_NATIVE_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            #[cfg(target_os = "windows")]
            return gtk::glib::user_data_dir().join("captures-windows-native");
            #[cfg(not(target_os = "windows"))]
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
                })
                .join("captures-linux-native")
        })
}

impl Settings {
    pub fn load() -> Result<Self, String> {
        let bytes = match std::fs::read(data_dir().join("settings.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.to_string()),
        };
        Self::from_bytes(&bytes)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let mut settings: Self =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        // Migrate only the earlier native experiment's schema, never Tauri data.
        if value.get("output_directory").is_none() {
            if let Some(output) = value.get("output").and_then(|v| v.as_str()) {
                settings.output_directory = output.into();
            }
            if let Some(fps) = value.get("fps").and_then(|v| v.as_u64()) {
                settings.recording.video_fps = fps.clamp(1, 60) as u16;
            }
            if let Some(seconds) = value.get("countdown").and_then(|v| v.as_u64()) {
                settings.screenshot_countdown_seconds = seconds.min(10) as u8;
                settings.recording.countdown_seconds = seconds.min(10) as u8;
            }
            if let Some(cursor) = value.get("cursor").and_then(|v| v.as_bool()) {
                settings.recording.show_cursor = cursor;
            }
            if let Some(corner) = value.get("corner").and_then(|v| v.as_u64()) {
                // Legacy native used left-first; current preferences use right-first.
                settings.mini_preview_placement = (corner.min(3) as u32) ^ 1;
            }
        }
        // Earlier builds offered WebM even though the native recorder rejects it.
        if settings.recording.video_format == "webm" {
            settings.recording.video_format = "mp4".into();
        }
        // The old spin control accepted 1–60, but video capture only accepts
        // 15/30/60. Round old valid values up without changing GIF frame rates.
        settings.recording.video_fps = match settings.recording.video_fps {
            1..=15 => 15,
            16..=30 => 30,
            31..=60 => 60,
            invalid => invalid,
        };
        settings.migrate_platform_shortcuts(cfg!(target_os = "windows"));
        settings.validate()?;
        Ok(settings)
    }

    fn migrate_platform_shortcuts(&mut self, windows: bool) {
        // Only replace the complete inherited factory set. A partial match may
        // be intentional customization; never create a duplicate PrintScreen binding.
        if windows
            && self.new_capture_shortcut == "PrintScreen"
            && self.display_shortcut == "Shift+PrintScreen"
            && self.recording.video_shortcut == "Control+Shift+Alt+R"
        {
            let mut migrated = self.clone();
            migrated.new_capture_shortcut = "Control+Shift+Space".into();
            migrated.display_shortcut = "PrintScreen".into();
            migrated.recording.video_shortcut = "Super+Alt+R".into();
            if crate::desktop::shortcuts(&migrated).is_ok() {
                *self = migrated;
            }
        }
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
        for color in [&self.custom_accent, &self.custom_signal] {
            if color.len() != 7
                || !color.starts_with('#')
                || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err("Theme colors must be #RRGGBB.".into());
            }
        }
        if self.mini_preview_placement > 3
            || self.screenshot_countdown_seconds > 10
            || self.recording.countdown_seconds > 10
        {
            return Err("Invalid corner or countdown.".into());
        }
        if !matches!(self.recording.video_fps, 15 | 30 | 60)
            || !(1..=30).contains(&self.recording.gif_fps)
            || !(2..=256).contains(&self.recording.gif_max_colors)
            || self.recording.gif_max_width < 16
        {
            return Err("Invalid recording or GIF limits.".into());
        }
        if !["png", "jpeg", "webp"].contains(&self.screenshot_format.as_str())
            || !["mp4", "gif"].contains(&self.recording.video_format.as_str())
        {
            return Err("Unsupported output format.".into());
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

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().ok_or("Missing parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    crate::windows::private_directory(parent).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temp = parent.join(format!(".write-{}-{stamp}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_webm_preferences_migrate_but_cannot_be_saved_again() {
        let mut settings = Settings::default();
        settings.recording.video_format = "webm".into();
        settings.recording.video_fps = 30;
        assert!(settings.validate().is_err());
        let migrated = Settings::from_bytes(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(migrated.recording.video_format, "mp4");
        assert_eq!(migrated.recording.video_fps, 30);
        settings.recording.video_format = "gif".into();
        let gif = Settings::from_bytes(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(gif.recording.video_format, "gif");
    }

    #[test]
    fn video_fps_migration_respects_recorder_boundaries_and_preserves_gif_fps() {
        for (old, expected) in [
            (1, 15),
            (15, 15),
            (16, 30),
            (24, 30),
            (30, 30),
            (31, 60),
            (60, 60),
        ] {
            let mut settings = Settings::default();
            settings.recording.video_fps = old;
            settings.recording.gif_fps = 17;
            assert_eq!(settings.validate().is_ok(), old == expected);
            let migrated = Settings::from_bytes(&serde_json::to_vec(&settings).unwrap()).unwrap();
            assert_eq!(migrated.recording.video_fps, expected);
            assert_eq!(migrated.recording.gif_fps, 17);
        }
        for invalid in [0, 61] {
            let mut settings = Settings::default();
            settings.recording.video_fps = invalid;
            assert!(Settings::from_bytes(&serde_json::to_vec(&settings).unwrap()).is_err());
        }
    }

    #[test]
    fn windows_shortcut_migration_preserves_linux_and_custom_bindings() {
        let mut legacy = Settings {
            new_capture_shortcut: "PrintScreen".into(),
            display_shortcut: "Shift+PrintScreen".into(),
            ..Settings::default()
        };
        legacy.recording.video_shortcut = "Control+Shift+Alt+R".into();
        legacy.migrate_platform_shortcuts(false);
        assert_eq!(legacy.new_capture_shortcut, "PrintScreen");
        let mut customized = legacy.clone();
        customized.display_shortcut = "Control+Shift+Space".into();
        customized.migrate_platform_shortcuts(true);
        assert_eq!(customized.new_capture_shortcut, "PrintScreen");
        assert_eq!(customized.display_shortcut, "Control+Shift+Space");
        let mut conflict = legacy.clone();
        conflict.region_shortcut = "Super+Alt+R".into();
        conflict.migrate_platform_shortcuts(true);
        assert_eq!(conflict.recording.video_shortcut, "Control+Shift+Alt+R");
        assert!(crate::desktop::shortcuts(&conflict).is_ok());
        legacy.migrate_platform_shortcuts(true);
        legacy.migrate_platform_shortcuts(true);
        assert_eq!(legacy.new_capture_shortcut, "Control+Shift+Space");
        assert_eq!(legacy.display_shortcut, "PrintScreen");
        assert_eq!(legacy.recording.video_shortcut, "Super+Alt+R");
        assert!(crate::desktop::shortcuts(&legacy).is_ok());
    }

    #[test]
    fn migrates_native_preferences_without_resetting_output_or_capture_choices() {
        let output = if cfg!(target_os = "windows") {
            r"C:\Temp\chosen"
        } else {
            "/tmp/chosen"
        };
        let fixture = serde_json::json!({"output": output, "fps": 60, "countdown": 5, "cursor": false, "corner": 3, "appearance": "dark"});
        let settings = Settings::from_bytes(&serde_json::to_vec(&fixture).unwrap()).unwrap();
        assert_eq!(settings.output_directory, PathBuf::from(output));
        assert_eq!(settings.recording.video_fps, 60);
        assert_eq!(settings.recording.countdown_seconds, 5);
        assert_eq!(settings.screenshot_countdown_seconds, 5);
        assert!(!settings.recording.show_cursor);
        assert_eq!(settings.mini_preview_placement, 2);
        assert_eq!(settings.appearance, "dark");
        assert!(settings.auto_copy_to_clipboard);
    }
    #[test]
    fn full_preferences_roundtrip_preserves_distinct_video_and_gif_settings() {
        let mut s = Settings::default();
        s.recording.video_fps = 60;
        s.recording.gif_fps = 12;
        s.recording.microphone_device_id = Some("microphone-2".into());
        s.freeze_screen = false;
        let copy = Settings::from_bytes(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(copy.recording.video_fps, 60);
        assert_eq!(copy.recording.gif_fps, 12);
        assert_eq!(
            copy.recording.microphone_device_id,
            Some("microphone-2".into())
        );
        assert!(!copy.freeze_screen);
    }
    #[test]
    fn atomic_save_replaces_only_its_target_and_leaves_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
