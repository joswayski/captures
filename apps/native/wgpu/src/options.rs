use std::{path::PathBuf, time::Duration};

pub const USAGE: &str = "Captures wgpu native host\n\
  --live [--history-root PATH]\n\
  --scene preferences|history|hud|preview|editor|idle\n\
  --appearance light|dark|system --theme mustard|ember|rose|violet|cobalt|aqua|mint|lime|mono\n\
  --history-count 0..10000 --exercise --quit-after SECONDS\n\
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
    Idle,
}

impl Scene {
    pub const VISIBLE: [Self; 5] = [
        Self::Preferences,
        Self::History,
        Self::Hud,
        Self::Preview,
        Self::Editor,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Preferences => "preferences",
            Self::History => "history",
            Self::Hud => "hud",
            Self::Preview => "preview",
            Self::Editor => "editor",
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
            Self::Idle => "Hidden window",
        }
    }
}

#[derive(Debug)]
pub struct Options {
    pub live: bool,
    pub history_root: Option<PathBuf>,
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
    pub settings_file: Option<PathBuf>,
    pub appearance_override: bool,
    pub theme_override: bool,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            live: false,
            history_root: None,
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
            settings_file: None,
            appearance_override: false,
            theme_override: false,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--live" => options.live = true,
                "--history-root" => {
                    options.history_root = Some(args.next().ok_or("Missing history root")?.into())
                }
                "--exercise" => options.exercise = true,
                "--floating" => options.floating = true,
                "--reduced-motion" => options.reduced_motion = true,
                "--scene" => {
                    let value = args.next().ok_or("Missing scene")?;
                    options.scene = Scene::VISIBLE
                        .into_iter()
                        .chain([Scene::Idle])
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
        if options.live
            && (options.exercise
                || options.floating
                || options.scene != Scene::Preferences
                || options.history_count != 1000
                || options.reduced_motion)
        {
            return Err("--live cannot be combined with fixture scenes or exercise options".into());
        }
        if options.history_root.is_some() && !options.live {
            return Err("--history-root requires --live".into());
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
    fn validates_limits_and_incompatible_modes() {
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
            parse(&["--screenshot", "test.png"]).unwrap().quit_after,
            Some(Duration::from_secs(16))
        );
    }
}
