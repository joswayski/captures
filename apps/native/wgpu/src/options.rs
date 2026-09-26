use std::{path::PathBuf, time::Duration};

pub const USAGE: &str = "Captures wgpu native host\n\
  --live [--history-root PATH] [--open-media PATH (repeatable; --open-image alias)]\n\
  --live -- FILE... (Open With; everything after -- is a local path)\n\
  --scene preferences|history|hud|preview|editor|capture-controls|region|window|update|countdown|idle\n\
  --update-state available|single|closing|manual|downloading|restarting|error|checking|up-to-date\n\
  --update-tray top|bottom|none (update scene only; stub status source)\n\
  --appearance light|dark|system --theme mustard|ember|rose|violet|cobalt|aqua|mint|lime|mono\n\
  --history-count 0..10000 --exercise --quit-after SECONDS\n\
  --capture-controls-recording --hud-state unmuted|muted|busy|no-microphone\n\
  --settings-file PATH\n\
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
}

impl Scene {
    pub const VISIBLE: [Self; 9] = [
        Self::Preferences,
        Self::History,
        Self::Hud,
        Self::Preview,
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
        };
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
        if options.history_root.is_some() && !options.live {
            return Err("--history-root requires --live".into());
        }
        if !options.open_media.is_empty() && !options.live {
            return Err("--open-media/--open-image requires --live".into());
        }
        if options.permission_dialog.is_some() && (!options.live || options.screenshot.is_none()) {
            return Err(
                "--permission-dialog requires a --live --screenshot rendering probe".into(),
            );
        }
        if options.screenshot.is_some() {
            let deadline = options
                .quit_after
                .get_or_insert(options.screenshot_after + Duration::from_secs(15));
            if *deadline <= options.screenshot_after {
                return Err("Quit deadline must be after the screenshot deadline".into());
            }
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
        for args in [
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
