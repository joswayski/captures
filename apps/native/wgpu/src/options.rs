use std::{path::PathBuf, time::Duration};

pub const USAGE: &str = "Captures wgpu native host\n\
  --live [--history-root PATH] [--open-media PATH (repeatable; --open-image alias)]\n\
  --live --open-preferences (also open the Preferences window at launch)\n\
  --live --open-history (open Capture History at launch instead of Preferences)\n\
  --live -- FILE... (Open With; everything after -- is a local path)\n\
  --scene preferences|history|hud|preview|sharing|editor|capture-controls|region|window|update|countdown|idle\n\
  --update-state available|single|closing|manual|downloading|restarting|error|checking|up-to-date\n\
  --update-tray top|bottom|none (update scene only; stub status source)\n\
  --appearance light|dark|system --theme mustard|ember|rose|violet|cobalt|aqua|mint|lime|mono\n\
  --history-count 0..10000 --exercise --quit-after SECONDS\n\
  --capture-controls-recording --hud-state unmuted|muted|busy|no-microphone|saving|failed\n\
  --settings-file PATH\n\
  --native-update-manifest-url URL --native-update-public-key-file PATH --native-update-current-version VERSION (explicit live profile; check only)\n\
  --native-update-staging-directory ABSOLUTE_PATH (existing scratch directory; enables explicit temporary download/verification, never installation)\n\
  --native-update-ready-file ABSOLUTE_PATH --native-update-ready-token UUID_V4 (health launches only)\n\
  --floating (HUD/preview only) --reduced-motion\n\
  --screenshot FILE.png --screenshot-after SECONDS";

pub const THEMES: [&str; 9] = [
    "mustard", "ember", "rose", "violet", "cobalt", "aqua", "mint", "lime", "mono",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    Preferences,
    History,
    Hud,
    Preview,
    Sharing,
    Editor,
    CaptureControls,
    Region,
    Window,
    Update,
    Countdown,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudState {
    Unmuted,
    Muted,
    Busy,
    NoMicrophone,
    Saving,
    Failed,
}

impl Scene {
    pub const VISIBLE: [Self; 10] = [
        Self::Preferences,
        Self::History,
        Self::Hud,
        Self::Preview,
        Self::Sharing,
        Self::Editor,
        Self::CaptureControls,
        Self::Region,
        Self::Window,
        Self::Update,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Preferences => "preferences",
            Self::History => "history",
            Self::Hud => "hud",
            Self::Preview => "preview",
            Self::Sharing => "sharing",
            Self::Editor => "editor",
            Self::CaptureControls => "capture-controls",
            Self::Region => "region",
            Self::Window => "window",
            Self::Update => "update",
            Self::Countdown => "countdown",
            Self::Idle => "idle",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Preferences => "Preferences",
            Self::History => "Capture history",
            Self::Hud => "Recording controls",
            Self::Preview => "Mini previews",
            Self::Sharing => "Share capture fixture",
            Self::Editor => "Editor rendering probe",
            Self::CaptureControls => "New Capture controls",
            Self::Region => "Region selector fixture",
            Self::Window => "Window selector fixture",
            Self::Update => "Update notice",
            Self::Countdown => "Screenshot countdown",
            Self::Idle => "Hidden window",
        }
    }
}

#[derive(Debug)]
pub struct Options {
    pub live: bool,
    pub history_root: Option<PathBuf>,
    pub open_media: Vec<PathBuf>,
    pub scene: Scene,
    pub appearance: String,
    pub theme: String,
    pub history_count: usize,
    pub exercise: bool,
    pub quit_after: Option<Duration>,
    pub screenshot: Option<PathBuf>,
    pub screenshot_after: Duration,
    pub floating: bool,
    pub reduced_motion: bool,
    pub capture_controls_recording: bool,
    pub hud_state: HudState,
    pub settings_file: Option<PathBuf>,
    pub appearance_override: bool,
    pub theme_override: bool,
    pub permission_dialog: Option<String>,
    pub update_state: Option<String>,
    pub update_tray: Option<crate::update_notice::FixtureTray>,
    /// Open the Preferences window beside Capture History once setup is done.
    pub open_preferences: bool,
    /// Open Capture History at launch, as its tray item or Preferences button
    /// would, instead of shipping's launch Preferences window.
    pub open_history: bool,
    pub native_update_health: Option<captures_app::updater::HealthAcknowledgement>,
    pub native_update_checks: Option<captures_app::updater::UpdateClient>,
    pub native_update_staging_directory: Option<PathBuf>,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            live: false,
            history_root: None,
            open_media: Vec::new(),
            scene: Scene::Preferences,
            appearance: "dark".into(),
            theme: "mustard".into(),
            history_count: 1000,
            exercise: false,
            quit_after: None,
            screenshot: None,
            screenshot_after: Duration::from_secs(1),
            floating: false,
            reduced_motion: false,
            capture_controls_recording: false,
            hud_state: HudState::Unmuted,
            settings_file: None,
            appearance_override: false,
            theme_override: false,
            permission_dialog: None,
            update_state: None,
            update_tray: None,
            open_preferences: false,
            open_history: false,
            native_update_health: None,
            native_update_checks: None,
            native_update_staging_directory: None,
        };
        let mut native_update_ready_file = None;
        let mut native_update_ready_token = None;
        let mut update_endpoint = None;
        let mut update_key_file = None;
        let mut update_current_version = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--" => {
                    for path in args.by_ref() {
                        if path.is_empty() {
                            return Err("Empty media path".into());
                        }
                        options.open_media.push(path.into());
                    }
                }
                "--live" => options.live = true,
                "--open-preferences" => options.open_preferences = true,
                "--open-history" => options.open_history = true,
                "--permission-dialog" => {
                    let value = args.next().ok_or("Missing permission dialog state")?;
                    if !matches!(value.as_str(), "ready" | "error") {
                        return Err("Permission dialog state must be ready or error".into());
                    }
                    options.permission_dialog = Some(value);
                }
                "--history-root" => {
                    options.history_root = Some(args.next().ok_or("Missing history root")?.into())
                }
                "--open-media" | "--open-image" => {
                    let path = args
                        .next()
                        .filter(|path| !path.is_empty())
                        .ok_or("Missing media path")?;
                    options.open_media.push(path.into());
                }
                "--update-state" => {
                    let value = args.next().ok_or("Missing update state")?;
                    if !captures_app::update_notice::FIXTURES.contains(&value.as_str()) {
                        return Err("Unknown update state".into());
                    }
                    options.update_state = Some(value);
                }
                "--update-tray" => {
                    options.update_tray = Some(
                        crate::update_notice::FixtureTray::parse(
                            &args.next().ok_or("Missing update tray")?,
                        )
                        .ok_or("Update tray must be top, bottom or none")?,
                    );
                }
                "--exercise" => options.exercise = true,
                "--floating" => options.floating = true,
                "--reduced-motion" => options.reduced_motion = true,
                "--capture-controls-recording" => options.capture_controls_recording = true,
                "--hud-state" => {
                    options.hud_state = match args.next().ok_or("Missing HUD state")?.as_str() {
                        "unmuted" => HudState::Unmuted,
                        "muted" => HudState::Muted,
                        "busy" => HudState::Busy,
                        "no-microphone" => HudState::NoMicrophone,
                        "saving" => HudState::Saving,
                        "failed" => HudState::Failed,
                        _ => return Err("Unknown HUD state".into()),
                    }
                }
                "--scene" => {
                    let value = args.next().ok_or("Missing scene")?;
                    options.scene = Scene::VISIBLE
                        .into_iter()
                        .chain([Scene::Idle, Scene::Countdown])
                        .find(|s| s.name() == value)
                        .ok_or("Unknown scene")?;
                }
                "--appearance" => {
                    options.appearance = args.next().ok_or("Missing appearance")?;
                    options.appearance_override = true;
                }
                "--theme" => {
                    options.theme = args.next().ok_or("Missing theme")?;
                    options.theme_override = true;
                }
                "--history-count" => {
                    options.history_count = args
                        .next()
                        .ok_or("Missing count")?
                        .parse()
                        .map_err(|_| "Invalid count")?;
                    if options.history_count > 10000 {
                        return Err("Count exceeds 10000".into());
                    }
                }
                "--quit-after" | "--screenshot-after" => {
                    let seconds: f64 = args
                        .next()
                        .ok_or("Missing duration")?
                        .parse()
                        .map_err(|_| "Invalid duration")?;
                    if !seconds.is_finite() || seconds <= 0. || seconds > 86400. {
                        return Err("Duration must be positive and at most one day".into());
                    }
                    let duration = Duration::from_secs_f64(seconds);
                    if arg == "--quit-after" {
                        options.quit_after = Some(duration);
                    } else {
                        options.screenshot_after = duration;
                    }
                }
                "--screenshot" => {
                    options.screenshot = Some(args.next().ok_or("Missing screenshot path")?.into())
                }
                "--settings-file" => {
                    options.settings_file = Some(args.next().ok_or("Missing settings path")?.into())
                }
                "--native-update-staging-directory" => {
                    if options.native_update_staging_directory.is_some() {
                        return Err("Duplicate native update staging directory".into());
                    }
                    options.native_update_staging_directory = Some(
                        args.next()
                            .ok_or("Missing native update staging directory")?
                            .into(),
                    );
                }
                "--native-update-manifest-url"
                | "--native-update-public-key-file"
                | "--native-update-current-version" => {
                    let slot = match arg.as_str() {
                        "--native-update-manifest-url" => &mut update_endpoint,
                        "--native-update-public-key-file" => &mut update_key_file,
                        _ => &mut update_current_version,
                    };
                    if slot.is_some() {
                        return Err("Duplicate native update-check option".into());
                    }
                    *slot = Some(
                        args.next()
                            .filter(|v| !v.is_empty())
                            .ok_or("Missing native update-check value")?,
                    );
                }
                "--native-update-ready-file" => {
                    if native_update_ready_file.is_some() {
                        return Err("Duplicate update ready file".into());
                    }
                    native_update_ready_file = Some(PathBuf::from(
                        args.next().ok_or("Missing update ready file")?,
                    ));
                }
                "--native-update-ready-token" => {
                    if native_update_ready_token.is_some() {
                        return Err("Duplicate update ready token".into());
                    }
                    native_update_ready_token =
                        Some(args.next().ok_or("Missing update ready token")?);
                }
                _ => return Err(format!("Unknown option {arg}")),
            }
        }
        if !["light", "dark", "system"].contains(&options.appearance.as_str())
            || !THEMES.contains(&options.theme.as_str())
        {
            return Err("Unknown appearance/theme".into());
        }
        if options.floating && ![Scene::Hud, Scene::Preview].contains(&options.scene) {
            return Err("Floating mode is only for HUD/preview".into());
        }
        if options.scene == Scene::Idle && (options.screenshot.is_some() || options.exercise) {
            return Err("Hidden idle has no screenshot or scripted actions".into());
        }
        if options.scene == Scene::Countdown && options.exercise {
            return Err(
                "Countdown is a static rendering probe; use --live for timing/cancellation".into(),
            );
        }
        if (options.update_state.is_some() || options.update_tray.is_some())
            && options.scene != Scene::Update
        {
            return Err("--update-state/--update-tray require --scene update".into());
        }
        if options.scene == Scene::Update && options.exercise {
            return Err("The update notice fixture has no scripted exercise".into());
        }
        if options.live
            && (options.exercise
                || options.floating
                || !matches!(options.scene, Scene::Preferences | Scene::Idle)
                || options.history_count != 1000)
        {
            return Err("--live cannot be combined with fixture scenes or exercise options".into());
        }
        if options.open_preferences && (!options.live || options.scene == Scene::Idle) {
            return Err("--open-preferences requires a visible --live launch".into());
        }
        if options.open_history && (!options.live || options.scene == Scene::Idle) {
            return Err("--open-history requires a visible --live launch".into());
        }
        if options.history_root.is_some() && !options.live {
            return Err("--history-root requires --live".into());
        }
        if !options.open_media.is_empty() && !options.live {
            return Err("--open-media/--open-image requires --live".into());
        }
        // Opens permission recovery as a denied capture would, for rendering
        // probes and interactive smokes (History has no permissions button).
        if options.permission_dialog.is_some() && !options.live {
            return Err("--permission-dialog requires --live".into());
        }
        if options.screenshot.is_some() {
            let deadline = options
                .quit_after
                .get_or_insert(options.screenshot_after + Duration::from_secs(15));
            if *deadline <= options.screenshot_after {
                return Err("Quit deadline must be after the screenshot deadline".into());
            }
        }
        match (native_update_ready_file, native_update_ready_token) {
            (None, None) => {}
            (Some(file), Some(token)) => {
                let explicit_paths = options
                    .history_root
                    .as_ref()
                    .zip(options.settings_file.as_ref())
                    .is_some_and(|(history, settings)| {
                        history.is_absolute() && settings.is_absolute()
                    });
                if !options.live
                    || !explicit_paths
                    || options.exercise
                    || options.floating
                    || options.scene != Scene::Preferences
                    || !options.open_media.is_empty()
                    || options.update_state.is_some()
                    || options.update_tray.is_some()
                    || options.screenshot.is_some()
                {
                    return Err("Native update readiness requires plain --live with explicit absolute --history-root and --settings-file".into());
                }
                options.native_update_health = Some(
                    captures_app::updater::HealthAcknowledgement::new(file, token)
                        .map_err(|error| error.to_string())?,
                );
            }
            _ => return Err(
                "--native-update-ready-file and --native-update-ready-token must appear together"
                    .into(),
            ),
        }
        match (update_endpoint, update_key_file, update_current_version) {
            (None, None, None) => {}
            (Some(endpoint), Some(key_file), Some(version)) => {
                let explicit_paths = options
                    .history_root
                    .as_ref()
                    .zip(options.settings_file.as_ref())
                    .is_some_and(|(history, settings)| {
                        history.is_absolute() && settings.is_absolute()
                    });
                if !options.live
                    || !explicit_paths
                    || options.scene != Scene::Preferences
                    || options.native_update_health.is_some()
                    || !options.open_media.is_empty()
                {
                    return Err("Native update checks require plain --live with explicit absolute --history-root and --settings-file; no health launch or media open".into());
                }
                options.native_update_checks = Some(
                    captures_app::updater::checks::client_from_key_file(
                        &endpoint,
                        PathBuf::from(key_file).as_path(),
                        captures_app::updater::Renderer::Wgpu,
                        &version,
                    )
                    .map_err(|error| error.to_string())?,
                );
            }
            _ => {
                return Err(
                    "The manifest URL, public key file and current version must appear together"
                        .into(),
                );
            }
        }
        if let Some(path) = &options.native_update_staging_directory {
            if options.native_update_checks.is_none() {
                return Err(
                    "Temporary acquisition requires the complete native update-check configuration"
                        .into(),
                );
            }
            captures_app::updater::checks::validate_staging_directory(path)
                .map_err(|e| e.to_string())?;
        }
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn live_previews_accept_explicit_reduced_motion() {
        let options = parse(&["--live", "--reduced-motion"]).unwrap();
        assert!(options.live && options.reduced_motion);
    }

    #[test]
    fn update_checks_require_complete_explicit_configuration_without_fixtures_or_health() {
        let root = tempfile::tempdir().unwrap();
        let key = root.path().join("public.key");
        std::fs::write(&key, "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3").unwrap();
        let args = vec![
            "--live".into(),
            "--history-root".into(),
            root.path().join("history").display().to_string(),
            "--settings-file".into(),
            root.path().join("settings.json").display().to_string(),
            "--native-update-manifest-url".into(),
            "http://127.0.0.1:9/native.json".into(),
            "--native-update-public-key-file".into(),
            key.display().to_string(),
            "--native-update-current-version".into(),
            "2026.9.99".into(),
        ];
        assert!(
            Options::parse(args.clone())
                .unwrap()
                .native_update_checks
                .is_some()
        );
        assert!(parse(&["--live"]).unwrap().native_update_checks.is_none());
        for extra in [
            vec!["--scene", "idle"],
            vec!["--scene", "update"],
            vec!["--exercise"],
            vec!["--open-media", "/tmp/capture.png"],
            vec!["--native-update-current-version", "2026.9.99"],
            vec![
                "--native-update-ready-file",
                "/tmp/ready",
                "--native-update-ready-token",
                "73147c85-13e0-4a67-b129-5e7ead486dc1",
            ],
        ] {
            let mut invalid = args.clone();
            invalid.extend(extra.into_iter().map(String::from));
            assert!(Options::parse(invalid).is_err());
        }
        let mut staged = args.clone();
        staged.extend([
            "--native-update-staging-directory".into(),
            root.path().display().to_string(),
        ]);
        assert_eq!(
            Options::parse(staged.clone())
                .unwrap()
                .native_update_staging_directory
                .as_deref(),
            Some(root.path())
        );
        assert!(
            parse(&[
                "--live",
                "--native-update-staging-directory",
                root.path().to_str().unwrap()
            ])
            .is_err()
        );
        for path in [
            PathBuf::from("relative"),
            key.clone(),
            root.path().join("missing"),
        ] {
            let mut invalid = args.clone();
            invalid.extend([
                "--native-update-staging-directory".into(),
                path.display().to_string(),
            ]);
            assert!(Options::parse(invalid).is_err());
        }
        staged.extend([
            "--native-update-staging-directory".into(),
            root.path().display().to_string(),
        ]);
        assert!(Options::parse(staged).is_err());
        assert!(Options::parse(args[1..].to_vec()).is_err());
        assert!(Options::parse(args[..args.len() - 2].to_vec()).is_err());
        let mut invalid = args;
        invalid[2] = "relative".into();
        assert!(Options::parse(invalid).is_err());
        assert!(
            !root.path().join("settings.json").exists() && !root.path().join("history").exists()
        );
    }

    #[test]
    fn native_update_health_requires_a_plain_explicit_live_profile() {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let history = root.path().join("history");
        let settings = root.path().join("settings.json");
        let token = "73147c85-13e0-4a67-b129-5e7ead486dc1";
        let args = || {
            vec![
                "--live".into(),
                "--history-root".into(),
                history.to_string_lossy().into_owned(),
                "--settings-file".into(),
                settings.to_string_lossy().into_owned(),
                "--native-update-ready-file".into(),
                ready.to_string_lossy().into_owned(),
                "--native-update-ready-token".into(),
                token.into(),
            ]
        };
        assert!(
            Options::parse(args())
                .unwrap()
                .native_update_health
                .is_some()
        );
        for extra in [
            vec!["--exercise"],
            vec!["--floating", "--scene", "hud"],
            vec!["--open-media", "/tmp/image.png"],
            vec!["--scene", "update", "--update-state", "available"],
        ] {
            let mut invalid = args();
            invalid.extend(extra.into_iter().map(String::from));
            assert!(Options::parse(invalid).is_err());
        }
        let mut missing_token = args();
        missing_token.truncate(missing_token.len() - 2);
        assert!(Options::parse(missing_token).is_err());
        let mut invalid_token = args();
        *invalid_token.last_mut().unwrap() = token.to_uppercase();
        assert!(Options::parse(invalid_token).is_err());
        assert!(
            parse(&[
                "--live",
                "--history-root",
                "relative",
                "--settings-file",
                "/tmp/settings",
                "--native-update-ready-file",
                "/tmp/ready",
                "--native-update-ready-token",
                token,
            ])
            .is_err()
        );
    }

    #[test]
    fn open_with_paths_cannot_be_interpreted_as_options() {
        let options = parse(&[
            "--live",
            "--open-media",
            "first.png",
            "--",
            "é space.png",
            "--exercise",
            "--",
            "second.webm",
        ])
        .unwrap();
        assert_eq!(
            options.open_media,
            [
                "first.png",
                "é space.png",
                "--exercise",
                "--",
                "second.webm"
            ]
            .map(PathBuf::from)
        );
        assert!(!options.exercise);
        assert!(parse(&["--", "file.png"]).is_err());
        assert!(parse(&["--live", "--", ""]).is_err());
        assert!(parse(&["--live", "--typo"]).is_err());
        assert_eq!(
            parse(&["--live", "--permission-dialog", "ready"])
                .unwrap()
                .permission_dialog
                .as_deref(),
            Some("ready")
        );
        assert!(parse(&["--permission-dialog", "ready"]).is_err());
        assert!(parse(&["--live", "--permission-dialog", "other"]).is_err());
        assert!(parse(&["--live", "--"]).unwrap().open_media.is_empty());
    }

    #[test]
    fn external_media_paths_and_alias_preserve_order_spaces_and_require_live_mode() {
        let options = parse(&[
            "--open-image",
            "relative image.PNG",
            "--live",
            "--open-media",
            "/tmp/second.webm",
            "--open-media",
            "relative image.PNG",
        ])
        .unwrap();
        assert_eq!(
            options.open_media,
            [
                PathBuf::from("relative image.PNG"),
                PathBuf::from("/tmp/second.webm"),
                PathBuf::from("relative image.PNG"),
            ]
        );
        assert!(parse(&[]).unwrap().open_media.is_empty());
        for args in [
            vec!["--open-media", "movie.mp4"],
            vec!["--live", "--open-media"],
            vec!["--live", "--open-media", ""],
            vec!["--live", "--scene", "history", "--open-media", "movie.gif"],
            vec!["--open-image", "image.png"],
            vec!["--live", "--open-image"],
            vec!["--live", "--open-image", ""],
            vec!["--live", "--scene", "history", "--open-image", "image.png"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn validates_limits_and_incompatible_modes() {
        assert_eq!(
            parse(&["--live", "--scene", "idle"]).unwrap().scene,
            Scene::Idle
        );
        assert!(parse(&["--live", "--scene", "idle", "--exercise"]).is_err());
        assert_eq!(parse(&["--history-count", "0"]).unwrap().history_count, 0);
        assert_eq!(
            parse(&["--history-count", "10000"]).unwrap().history_count,
            10000
        );
        assert!(parse(&["--live"]).unwrap().history_root.is_none());
        assert_eq!(
            parse(&["--live", "--history-root", "/tmp/captures"])
                .unwrap()
                .history_root,
            Some(PathBuf::from("/tmp/captures"))
        );
        assert!(
            parse(&["--live", "--open-preferences"])
                .unwrap()
                .open_preferences
        );
        let history = parse(&["--live", "--open-history"]).unwrap();
        assert!(history.open_history && !history.open_preferences);
        for args in [
            vec!["--open-preferences"],
            vec!["--live", "--scene", "idle", "--open-preferences"],
            vec!["--open-history"],
            vec!["--live", "--scene", "idle", "--open-history"],
            vec!["--live", "--exercise"],
            vec!["--live", "--scene", "history"],
            vec!["--history-root", "/tmp/captures"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        for args in [
            vec!["--history-count", "10001"],
            vec!["--history-count", "-1"],
            vec!["--quit-after", "NaN"],
            vec!["--quit-after", "0"],
            vec!["--theme", "not-a-theme"],
            vec!["--scene"],
            vec!["--floating"],
            vec!["--scene", "idle", "--exercise"],
            vec!["--scene", "countdown", "--exercise"],
            vec![
                "--screenshot",
                "test.png",
                "--screenshot-after",
                "5",
                "--quit-after",
                "5",
            ],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        assert!(parse(&["--scene", "preview", "--floating"]).is_ok());
        assert_eq!(
            parse(&["--scene", "hud", "--hud-state", "muted"])
                .unwrap()
                .hud_state,
            HudState::Muted
        );
        assert_eq!(
            parse(&["--scene", "hud", "--hud-state", "failed"])
                .unwrap()
                .hud_state,
            HudState::Failed
        );
        assert!(parse(&["--hud-state", "unknown"]).is_err());
        assert_eq!(
            parse(&["--scene", "countdown"]).unwrap().scene,
            Scene::Countdown
        );
        let update = parse(&[
            "--scene",
            "update",
            "--update-state",
            "error",
            "--update-tray",
            "bottom",
        ])
        .unwrap();
        assert_eq!(update.scene, Scene::Update);
        assert_eq!(update.update_state.as_deref(), Some("error"));
        assert_eq!(
            update.update_tray,
            Some(crate::update_notice::FixtureTray::Bottom)
        );
        for args in [
            vec!["--update-state", "error"],
            vec!["--scene", "update", "--update-state", "unknown"],
            vec!["--scene", "update", "--update-tray", "left"],
            vec!["--scene", "update", "--exercise"],
            vec!["--live", "--scene", "update"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        assert_eq!(
            parse(&["--screenshot", "test.png"]).unwrap().quit_after,
            Some(Duration::from_secs(16))
        );
    }
}
