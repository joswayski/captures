use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

use captures_capture::CaptureMode;
use captures_recording::MaxResolution;
use directories::{ProjectDirs, UserDirs};
use global_hotkey::hotkey::HotKey;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod theme;

pub const CURRENT_SETTINGS_SCHEMA_VERSION: u8 = 5;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("settings I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("settings data is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("settings schema {found} is newer than supported schema {supported}")]
    NewerSchema { found: u8, supported: u8 },
    #[error("{0}")]
    Validation(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppSettings {
    #[serde(default)]
    pub settings_schema_version: u8,
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub theme: ColorTheme,
    #[serde(default)]
    pub custom_theme: CustomThemeSettings,
    pub output_directory: String,
    #[serde(default = "default_new_capture_shortcut")]
    pub new_capture_shortcut: String,
    pub region_shortcut: String,
    pub window_shortcut: String,
    pub display_shortcut: String,
    #[serde(default = "default_true")]
    pub auto_copy_to_clipboard: bool,
    #[serde(default)]
    pub auto_start_on_selection: bool,
    #[serde(default = "default_true")]
    pub show_mini_previews: bool,
    #[serde(default)]
    pub mini_preview_placement: MiniPreviewPlacement,
    #[serde(default)]
    pub include_mini_previews_in_captures: bool,
    #[serde(default)]
    pub include_recording_controls_in_captures: bool,
    pub launch_at_login: bool,
    #[serde(default)]
    pub last_screen_permission_request_id: Option<String>,
    #[serde(default)]
    pub pending_capture_after_restart: Option<CaptureMode>,
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default = "default_screenshot_countdown_seconds")]
    pub screenshot_countdown_seconds: u8,
    #[serde(default = "default_true")]
    pub freeze_screen: bool,
    #[serde(default = "default_true")]
    pub show_cursor_in_screenshots: bool,
    #[serde(default)]
    pub screenshot_format: ScreenshotFormat,
    #[serde(default = "default_true")]
    pub show_update_changelog: bool,
    #[serde(default)]
    pub recording: RecordingSettings,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MiniPreviewPlacement {
    #[default]
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}
impl MiniPreviewPlacement {
    pub const fn is_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopRight)
    }
    pub const fn is_right(self) -> bool {
        matches!(self, Self::BottomRight | Self::TopRight)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorTheme {
    #[default]
    Mustard,
    Ember,
    Rose,
    #[serde(alias = "saffron")]
    Violet,
    Cobalt,
    Aqua,
    Mint,
    Lime,
    Mono,
    Custom,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CustomThemeSettings {
    #[serde(default = "default_custom_theme_accent")]
    pub accent: String,
    #[serde(default = "default_custom_theme_signal")]
    pub signal: String,
}
impl CustomThemeSettings {
    pub fn is_valid(&self) -> bool {
        is_hex_color(&self.accent) && is_hex_color(&self.signal)
    }
}
impl Default for CustomThemeSettings {
    fn default() -> Self {
        Self {
            accent: default_custom_theme_accent(),
            signal: default_custom_theme_signal(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}
impl ScreenshotFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }
    pub fn extension_matches(self, e: &str) -> bool {
        match self {
            Self::Png => e.eq_ignore_ascii_case("png"),
            Self::Jpeg => e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"),
            Self::Webp => e.eq_ignore_ascii_case("webp"),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoFormat {
    #[default]
    Mp4,
    Gif,
    #[serde(rename = "webm", alias = "web_m")]
    WebM,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordingSettings {
    #[serde(default = "default_video_shortcut")]
    pub video_shortcut: String,
    #[serde(default = "default_record_window_shortcut")]
    pub window_shortcut: String,
    #[serde(default = "default_record_display_shortcut")]
    pub display_shortcut: String,
    #[serde(default = "default_gif_shortcut")]
    pub gif_shortcut: String,
    #[serde(default)]
    pub video_format: VideoFormat,
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
            window_shortcut: default_record_window_shortcut(),
            display_shortcut: default_record_display_shortcut(),
            gif_shortcut: default_gif_shortcut(),
            video_format: VideoFormat::default(),
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
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            settings_schema_version: CURRENT_SETTINGS_SCHEMA_VERSION,
            appearance: Appearance::default(),
            theme: ColorTheme::default(),
            custom_theme: CustomThemeSettings::default(),
            output_directory: default_output_directory().to_string_lossy().into_owned(),
            new_capture_shortcut: default_new_capture_shortcut(),
            region_shortcut: default_region_shortcut(),
            window_shortcut: default_window_shortcut(),
            display_shortcut: default_display_shortcut(),
            auto_copy_to_clipboard: true,
            auto_start_on_selection: false,
            show_mini_previews: true,
            mini_preview_placement: MiniPreviewPlacement::default(),
            include_mini_previews_in_captures: false,
            include_recording_controls_in_captures: false,
            launch_at_login: false,
            last_screen_permission_request_id: None,
            pending_capture_after_restart: None,
            onboarding_completed: false,
            screenshot_countdown_seconds: 0,
            freeze_screen: true,
            show_cursor_in_screenshots: true,
            screenshot_format: ScreenshotFormat::default(),
            show_update_changelog: true,
            recording: RecordingSettings::default(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_custom_theme_accent() -> String {
    "#32d3ff".to_owned()
}
fn default_custom_theme_signal() -> String {
    "#ff4fc3".to_owned()
}
fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}
const SHORTCUT_MODIFIER: &str = "CommandOrControl";
fn captures_extra_shortcut(key: &str) -> String {
    format!("{SHORTCUT_MODIFIER}+Shift+{key}")
}
pub fn default_new_capture_shortcut() -> String {
    if cfg!(target_os = "linux") {
        "PrintScreen".into()
    } else {
        captures_extra_shortcut("Space")
    }
}
pub fn default_region_shortcut() -> String {
    if cfg!(target_os = "macos") {
        captures_extra_shortcut("4")
    } else {
        "Super+Shift+S".into()
    }
}
pub fn default_window_shortcut() -> String {
    if cfg!(target_os = "macos") {
        captures_extra_shortcut("W")
    } else {
        "Alt+PrintScreen".into()
    }
}
pub fn default_display_shortcut() -> String {
    if cfg!(target_os = "macos") {
        captures_extra_shortcut("3")
    } else if cfg!(target_os = "windows") {
        "PrintScreen".into()
    } else {
        "Shift+PrintScreen".into()
    }
}
pub fn default_video_shortcut() -> String {
    if cfg!(target_os = "macos") {
        captures_extra_shortcut("5")
    } else if cfg!(target_os = "windows") {
        "Super+Alt+R".into()
    } else {
        "Control+Shift+Alt+R".into()
    }
}
pub fn default_record_window_shortcut() -> String {
    format!("{SHORTCUT_MODIFIER}+Shift+Alt+W")
}
pub fn default_record_display_shortcut() -> String {
    format!("{SHORTCUT_MODIFIER}+Shift+Alt+3")
}
pub fn default_gif_shortcut() -> String {
    captures_extra_shortcut("6")
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
const fn default_screenshot_countdown_seconds() -> u8 {
    0
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
pub fn default_native_settings_path() -> PathBuf {
    ProjectDirs::from("io", "github", "Captures Native")
        .map(|d| d.config_dir().join("settings.json"))
        .unwrap_or_else(|| PathBuf::from(".captures-native/settings.json"))
}
pub fn shipping_settings_path() -> PathBuf {
    ProjectDirs::from("io", "github", "captures")
        .map(|d| d.config_dir().join("settings.json"))
        .unwrap_or_else(|| PathBuf::from("settings.json"))
}

pub fn load(path: &Path) -> Result<AppSettings, SettingsError> {
    let data = match fs::read(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppSettings::default()),
        Err(e) => return Err(e.into()),
    };
    let mut value: serde_json::Value = serde_json::from_slice(&data)?;
    let version = value
        .get("settings_schema_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if version > u64::from(CURRENT_SETTINGS_SCHEMA_VERSION) {
        return Err(SettingsError::NewerSchema {
            found: u8::try_from(version).unwrap_or(u8::MAX),
            supported: CURRENT_SETTINGS_SCHEMA_VERSION,
        });
    }
    let mut settings: AppSettings = serde_json::from_value(value.take())?;
    migrate_legacy_output_directory(&mut settings);
    if migrate_settings(&mut settings) {
        write_atomic(path, &settings)?;
    }
    Ok(settings)
}
/// Shipping desktop compatibility loader: malformed or unreadable files fall
/// back to defaults, and a failed migration write does not discard the loaded
/// settings.
pub fn load_shipping(path: &Path) -> AppSettings {
    let mut settings = fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default();
    migrate_legacy_output_directory(&mut settings);
    if migrate_settings(&mut settings)
        && let Err(error) = write_atomic(path, &settings)
    {
        eprintln!("failed to persist migrated settings: {error}");
    }
    settings
}
/// Atomically writes the complete settings model. This trusted primitive is
/// used by the shipping desktop app, including internal onboarding and
/// permission bookkeeping updates.
pub fn write_atomic(path: &Path, settings: &AppSettings) -> Result<(), SettingsError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&serde_json::to_vec_pretty(settings)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| SettingsError::Io(e.error))?;
    Ok(())
}

/// Validated native-host save. Internal fields always come from trusted disk
/// state (or shipping defaults when this is the first save).
pub fn save(path: &Path, settings: &AppSettings) -> Result<AppSettings, SettingsError> {
    let mut next = settings.clone();
    let old = if path.exists() {
        load(path)?
    } else {
        AppSettings::default()
    };
    next.settings_schema_version = old.settings_schema_version;
    next.last_screen_permission_request_id = old.last_screen_permission_request_id;
    next.pending_capture_after_restart = old.pending_capture_after_restart;
    next.onboarding_completed = old.onboarding_completed;
    if next.output_directory.trim().is_empty() {
        next.output_directory = default_output_directory().to_string_lossy().into_owned();
    }
    validate(&next)?;
    write_atomic(path, &next)?;
    Ok(next)
}
pub fn validate(s: &AppSettings) -> Result<(), SettingsError> {
    let values = [
        &s.new_capture_shortcut,
        &s.region_shortcut,
        &s.window_shortcut,
        &s.display_shortcut,
        &s.recording.video_shortcut,
        &s.recording.window_shortcut,
        &s.recording.display_shortcut,
    ];
    let mut parsed = HashSet::new();
    for value in values {
        if value.trim().is_empty() {
            return Err(SettingsError::Validation(
                "all shortcuts must be set".into(),
            ));
        }
        let hotkey =
            HotKey::from_str(value).map_err(|e| SettingsError::Validation(e.to_string()))?;
        if !parsed.insert(hotkey) {
            return Err(SettingsError::Validation("shortcuts must be unique".into()));
        }
    }
    if !matches!(s.recording.video_fps, 15 | 30 | 60)
        || !matches!(s.recording.gif_fps, 8..=30)
        || s.recording.gif_max_width < 320
        || !(64..=256).contains(&s.recording.gif_max_colors)
        || s.recording.countdown_seconds > 10
        || s.screenshot_countdown_seconds > 10
    {
        return Err(SettingsError::Validation(
            "capture settings are outside their supported range".into(),
        ));
    }
    if !s.custom_theme.is_valid() {
        return Err(SettingsError::Validation(
            "custom theme colors must use #RRGGBB values".into(),
        ));
    }
    Ok(())
}
pub fn migrate_legacy_output_directory(s: &mut AppSettings) {
    if let Some(d) = UserDirs::new()
        && let Some(p) = d.picture_dir()
        && Path::new(&s.output_directory) == p.join("Captures")
    {
        s.output_directory = d.home_dir().join("Captures").to_string_lossy().into_owned();
    }
}
pub fn migrate_settings(s: &mut AppSettings) -> bool {
    if s.settings_schema_version >= CURRENT_SETTINGS_SCHEMA_VERSION {
        return false;
    }
    if s.settings_schema_version < 1 {
        if s.recording.video_fps == 30 {
            s.recording.video_fps = 60
        }
        if s.recording.video_max_resolution == MaxResolution::P1080 {
            s.recording.video_max_resolution = MaxResolution::Original
        }
    }
    if s.settings_schema_version < 2 {
        s.onboarding_completed = true
    }
    if s.settings_schema_version < 3 {
        migrate_control_shift_factory_shortcuts(s)
    }
    if s.settings_schema_version < 4 {
        migrate_to_platform_native_shortcuts(s)
    }
    s.settings_schema_version = CURRENT_SETTINGS_SCHEMA_VERSION;
    true
}
fn migrate_control_shift_factory_shortcuts(settings: &mut AppSettings) {
    for (value, old, next) in [
        (
            &mut settings.new_capture_shortcut,
            "Ctrl+Shift+Space",
            "CommandOrControl+Shift+Space",
        ),
        (
            &mut settings.region_shortcut,
            "Ctrl+Shift+4",
            "CommandOrControl+Shift+4",
        ),
        (
            &mut settings.window_shortcut,
            "Ctrl+Shift+W",
            "CommandOrControl+Shift+W",
        ),
        (
            &mut settings.display_shortcut,
            "Ctrl+Shift+3",
            "CommandOrControl+Shift+3",
        ),
        (
            &mut settings.recording.video_shortcut,
            "Ctrl+Shift+5",
            "CommandOrControl+Shift+5",
        ),
        (
            &mut settings.recording.gif_shortcut,
            "Ctrl+Shift+6",
            "CommandOrControl+Shift+6",
        ),
    ] {
        replace_factory_shortcut(value, &[old], next);
    }
}
fn migrate_to_platform_native_shortcuts(settings: &mut AppSettings) {
    let changes = [
        (
            &mut settings.new_capture_shortcut,
            "Ctrl+Shift+Space",
            "CommandOrControl+Shift+Space",
            default_new_capture_shortcut(),
        ),
        (
            &mut settings.region_shortcut,
            "Ctrl+Shift+4",
            "CommandOrControl+Shift+4",
            default_region_shortcut(),
        ),
        (
            &mut settings.window_shortcut,
            "Ctrl+Shift+W",
            "CommandOrControl+Shift+W",
            default_window_shortcut(),
        ),
        (
            &mut settings.display_shortcut,
            "Ctrl+Shift+3",
            "CommandOrControl+Shift+3",
            default_display_shortcut(),
        ),
        (
            &mut settings.recording.video_shortcut,
            "Ctrl+Shift+5",
            "CommandOrControl+Shift+5",
            default_video_shortcut(),
        ),
        (
            &mut settings.recording.gif_shortcut,
            "Ctrl+Shift+6",
            "CommandOrControl+Shift+6",
            default_gif_shortcut(),
        ),
    ];
    for (value, old, command, next) in changes {
        replace_factory_shortcut(value, &[old, command], next);
    }
}
fn replace_factory_shortcut(value: &mut String, factories: &[&str], next: impl Into<String>) {
    if factories
        .iter()
        .any(|factory| shortcuts_equivalent(value, factory))
    {
        *value = next.into();
    }
}
fn shortcuts_equivalent(left: &str, right: &str) -> bool {
    match (
        canonical_shortcut_parts(left),
        canonical_shortcut_parts(right),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}
fn canonical_shortcut_parts(shortcut: &str) -> Option<(BTreeSet<String>, String)> {
    let mut tokens: Vec<String> = shortcut
        .split('+')
        .map(canonical_shortcut_token)
        .filter(|token| !token.is_empty())
        .collect();
    let key = tokens.pop().filter(|token| !token.is_empty())?;
    Some((tokens.into_iter().collect(), key))
}
fn canonical_shortcut_token(token: &str) -> String {
    let normalized = token.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "control" | "ctrl" => "control".to_owned(),
        "shift" => "shift".to_owned(),
        "alt" | "option" => "alt".to_owned(),
        "super" | "cmd" | "command" | "meta" | "win" => "super".to_owned(),
        "commandorcontrol" | "commandorctrl" | "cmdorctrl" | "cmdorcontrol" => {
            "commandorcontrol".to_owned()
        }
        "printscreen" | "prtscn" | "prtsc" | "print" => "printscreen".to_owned(),
        other => other
            .strip_prefix("digit")
            .or_else(|| other.strip_prefix("key"))
            .unwrap_or(other)
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistence_and_protection() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        let s = AppSettings {
            onboarding_completed: true,
            last_screen_permission_request_id: Some("protected".into()),
            ..AppSettings::default()
        };
        write_atomic(&p, &s).unwrap();
        let mut edit = load(&p).unwrap();
        edit.onboarding_completed = false;
        edit.last_screen_permission_request_id = None;
        edit.recording.video_fps = 30;
        let saved = save(&p, &edit).unwrap();
        assert!(saved.onboarding_completed);
        assert_eq!(load(&p).unwrap().recording.video_fps, 30);
        assert_eq!(
            saved.last_screen_permission_request_id.as_deref(),
            Some("protected")
        );
    }
    #[test]
    fn rejects_invalid_and_preserves_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        save(&p, &AppSettings::default()).unwrap();
        let before = fs::read(&p).unwrap();
        let mut s = AppSettings::default();
        s.custom_theme.accent = "bad".into();
        assert!(save(&p, &s).is_err());
        assert_eq!(before, fs::read(&p).unwrap());
    }
    #[test]
    fn rejects_malformed_and_newer() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        fs::write(&p, b"{").unwrap();
        assert!(matches!(load(&p), Err(SettingsError::Json(_))));
        fs::write(&p, r#"{"settings_schema_version":99}"#).unwrap();
        assert!(matches!(load(&p), Err(SettingsError::NewerSchema { .. })));
    }

    #[test]
    fn trusted_writes_internal_state_and_first_native_save_cannot_forge_it() {
        let d = tempfile::tempdir().unwrap();
        let trusted = d.path().join("trusted.json");
        let settings = AppSettings {
            onboarding_completed: true,
            pending_capture_after_restart: Some(CaptureMode::Region),
            ..AppSettings::default()
        };
        write_atomic(&trusted, &settings).unwrap();
        assert!(load(&trusted).unwrap().onboarding_completed);

        let first = d.path().join("first.json");
        let saved = save(&first, &settings).unwrap();
        assert!(!saved.onboarding_completed);
        assert!(saved.pending_capture_after_restart.is_none());
    }

    #[test]
    fn invalid_existing_values_can_be_repaired() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let mut invalid = AppSettings::default();
        invalid.recording.video_fps = 12;
        write_atomic(&p, &invalid).unwrap();
        let mut repair = load(&p).unwrap();
        repair.recording.video_fps = 30;
        assert_eq!(save(&p, &repair).unwrap().recording.video_fps, 30);
    }

    #[test]
    fn native_save_does_not_overwrite_malformed_or_newer_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        for bytes in [b"{".as_slice(), br#"{"settings_schema_version":99}"#] {
            fs::write(&p, bytes).unwrap();
            assert!(save(&p, &AppSettings::default()).is_err());
            assert_eq!(fs::read(&p).unwrap(), bytes);
        }
    }
}
