//! Captures' Windows host: Win32 windows, DirectComposition, Direct2D,
//! DirectWrite and UI Automation, with Captures' own token-drawn surfaces and
//! controls. Runs beside the wgpu workbench until it passes the feature,
//! screenshot, performance and accessibility gates in `docs/native-rewrite.md`.
//! Release builds are GUI-subsystem; debug builds keep a console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod appearance;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod countdown;
#[cfg(windows)]
mod dialog;
#[cfg(windows)]
mod editor;
#[cfg(windows)]
mod editor_uia;
#[cfg(windows)]
mod feedback;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod history;
#[cfg(windows)]
mod history_uia;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod hud;
#[cfg(windows)]
mod hud_uia;
#[cfg(windows)]
mod keys;
#[cfg(windows)]
mod live;
#[cfg(windows)]
mod panel_uia;
#[cfg(windows)]
mod previews;
#[cfg(windows)]
mod recorder;
#[cfg(windows)]
mod recording;
#[cfg(windows)]
mod recording_uia;
#[cfg(windows)]
mod scene;
#[cfg(windows)]
mod screenshot;
#[cfg(windows)]
mod setup;
#[cfg(windows)]
mod startup_notice;
// The editing model builds everywhere so its tests run on any OS.
mod textfield;
#[cfg(windows)]
mod tokens;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod uia;
#[cfg(windows)]
mod uia_text;
#[cfg(windows)]
mod update_notice;
#[cfg(windows)]
mod updates;

/// The window `--scene` opens first.
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    Preferences,
    History,
    /// `--scene editor --artifact ID`.
    Editor,
    /// `--scene recording --artifact ID`.
    Recording,
    /// `--scene capture --screenshot PATH`: the New Capture bar over a
    /// fixture desktop; `--tool display` selects Full screen and `--tool
    /// window` Window (`--select x` hovers its window, `desktop` the display,
    /// `picked` picks the window with the pointer on the desktop), `--tool
    /// countdown` shows the countdown and `--tool previews` the mini-preview
    /// stack (`--select x` hovers its first card).
    Capture,
    /// `--scene setup`: first-run setup (`--select checking|error|focus|hover`
    /// for screenshots).
    Setup,
    /// `--scene feedback`: Send Feedback (`--select typed|live|sent|error|idea`
    /// for screenshots).
    Feedback,
    /// `--scene update --update NAME`: the floating update notice on a
    /// shared `update_notice::fixture` status (`--tool fallback` without a
    /// tray icon, `--select notes-hidden` with What's new collapsed).
    Update,
}

#[cfg(windows)]
#[derive(Clone)]
pub struct Options {
    scene: Scene,
    /// `--artifact`: the History artifact `--scene editor` opens.
    artifact: Option<String>,
    appearance: Option<String>,
    settings_file: std::path::PathBuf,
    history_root: Option<std::path::PathBuf>,
    screenshot: Option<std::path::PathBuf>,
    size: Option<(u32, u32)>,
    /// `--find`: render with Find open and this query typed.
    find: Option<String>,
    /// `--tool KEY`: the editor's starting tool (a rail or shape key).
    tool: Option<String>,
    /// `--select LAYER`: the editor's starting selection (a layer ID).
    select: Option<String>,
    /// `--crop X,Y,W,H`: the editor's Crop tool with that rectangle staged.
    crop: Option<[f64; 4]>,
    /// `--update NAME`: the `--scene update` fixture.
    update: Option<String>,
    /// `--profile DIR` or `CAPTURES_NATIVE_PROFILE`: a disposable profile for
    /// tests on a real PC. Settings, history and saves stay under DIR, and the
    /// launch-at-login registry entry is never touched.
    profile: bool,
    /// `--live`: the resident app with its notification-area icon.
    live: bool,
    /// `--captures-autostart`: a quiet resident launch that opens no window.
    quiet: bool,
    /// Open With: local files to import into History and edit, from
    /// `--open-media PATH`, everything after `--`, or (in a Preview build)
    /// plain path and `file:` URI arguments.
    open: Vec<std::path::PathBuf>,
    /// The installed Preview profile replaces the default paths once the
    /// startup recovery companion is armed (`main`).
    preview_profile: bool,
    /// `--native-update-ready-file` and `--native-update-ready-token` from the
    /// Preview installer, or the startup recovery companion's own file.
    health: Option<captures_app::updater::HealthAcknowledgement>,
    /// Windows to restore after an update restart, taken after election.
    restart: Option<captures_app::updater::RestartIntent>,
}

#[cfg(windows)]
fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    parse_args(args, captures_app::preview_channel::version().is_some())
}

/// [`parse`] for a Preview build (`preview`) or a development build.
#[cfg(windows)]
fn parse_args(args: impl Iterator<Item = String>, preview: bool) -> Result<Options, String> {
    let mut options = Options {
        scene: Scene::Preferences,
        artifact: None,
        appearance: None,
        settings_file: captures_settings::default_native_settings_path(),
        history_root: None,
        screenshot: None,
        size: None,
        find: None,
        tool: None,
        select: None,
        crop: None,
        update: None,
        profile: false,
        // An installed Preview is the resident app, as its launchers expect.
        live: preview,
        quiet: false,
        open: Vec::new(),
        preview_profile: false,
        health: None,
        restart: None,
    };
    let mut profile = std::env::var_os("CAPTURES_NATIVE_PROFILE").map(std::path::PathBuf::from);
    let mut explicit = false;
    let (mut ready_file, mut ready_token) = (None, None);
    let mut args = args.skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--open-media" => {
                let path = value()?;
                if path.is_empty() {
                    return Err("--open-media needs a path".into());
                }
                options.open.push(path.into());
            }
            // Everything after `--` is a file to open.
            "--" => options.open.extend(args.by_ref().map(Into::into)),
            "--native-update-ready-file" if ready_file.is_none() => {
                ready_file = Some(std::path::PathBuf::from(value()?));
            }
            "--native-update-ready-token" if ready_token.is_none() => {
                ready_token = Some(value()?);
            }
            "--native-update-ready-file" | "--native-update-ready-token" => {
                return Err(format!("duplicate {flag}"));
            }
            // Open With and file associations pass the file itself: a path,
            // or a `file:` URI from some launchers. Only an installed Preview
            // registers them, so development builds keep refusing strays.
            path if preview && !path.is_empty() && !path.starts_with('-') => {
                options.open.push(path.into());
            }
            "--scene" => {
                options.scene = match value()?.as_str() {
                    "preferences" => Scene::Preferences,
                    "history" => Scene::History,
                    "editor" => Scene::Editor,
                    "recording" => Scene::Recording,
                    "capture" => Scene::Capture,
                    "setup" => Scene::Setup,
                    "feedback" => Scene::Feedback,
                    "update" => Scene::Update,
                    scene => return Err(format!("unsupported scene {scene}")),
                };
            }
            "--appearance" => {
                let appearance = value()?;
                if appearance != "light" && appearance != "dark" {
                    return Err("--appearance is light or dark".into());
                }
                options.appearance = Some(appearance);
            }
            "--settings-file" => {
                options.settings_file = value()?.into();
                explicit = true;
            }
            "--history-root" => {
                options.history_root = Some(value()?.into());
                explicit = true;
            }
            "--profile" => profile = Some(value()?.into()),
            "--live" => options.live = true,
            "--captures-autostart" => options.quiet = true,
            "--screenshot" => options.screenshot = Some(value()?.into()),
            "--artifact" => options.artifact = Some(value()?),
            "--find" => options.find = Some(value()?),
            "--tool" => options.tool = Some(value()?),
            "--select" => options.select = Some(value()?),
            "--update" => {
                let name = value()?;
                if !captures_app::update_notice::FIXTURES.contains(&name.as_str()) {
                    return Err(format!(
                        "--update is one of {}",
                        captures_app::update_notice::FIXTURES.join(", ")
                    ));
                }
                options.update = Some(name);
            }
            "--crop" => {
                let crop = value()?;
                let parts: Vec<f64> = crop
                    .split(',')
                    .map(|v| v.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|_| "--crop is X,Y,W,H")?;
                options.crop = Some(parts.try_into().map_err(|_| "--crop is X,Y,W,H")?);
            }
            "--size" => {
                let size = value()?;
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                let parse = |v: &str| v.parse::<u32>().ok().filter(|v| (1..=16384).contains(v));
                options.size = Some((parse(w).ok_or("bad width")?, parse(h).ok_or("bad height")?));
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    if let Some(dir) = profile {
        if explicit {
            return Err("--profile replaces --settings-file and --history-root".into());
        }
        use_profile(&mut options, &dir)?;
    }
    if options.scene == Scene::Capture && options.screenshot.is_none() {
        return Err("--scene capture renders with --screenshot; use --live to capture".into());
    }
    if options.quiet && !options.live {
        return Err("--captures-autostart needs --live".into());
    }
    if options.scene == Scene::Update && options.screenshot.is_none() {
        return Err("--scene update renders with --screenshot".into());
    }
    if matches!(options.scene, Scene::Editor | Scene::Recording) && options.artifact.is_none() {
        return Err("--scene editor/recording needs --artifact ID".into());
    }
    options.open = std::mem::take(&mut options.open)
        .into_iter()
        .map(captures_app::preview_channel::local_media_path)
        .collect::<Result<_, _>>()?;
    if !options.open.is_empty() && (!options.live || options.screenshot.is_some()) {
        return Err("Opening files needs the resident app (--live)".into());
    }
    match (ready_file, ready_token) {
        (None, None) => {}
        (Some(file), Some(token)) => {
            // The Preview installer relaunches plain `--live` on the
            // installed profile; a development check needs explicit paths.
            let absolute = options
                .history_root
                .as_ref()
                .is_some_and(|p| p.is_absolute())
                && options.settings_file.is_absolute();
            let installed = preview && !explicit && !options.profile;
            let paths = absolute || installed;
            if !options.live
                || options.quiet
                || options.screenshot.is_some()
                || !options.open.is_empty()
                || !paths
            {
                return Err("Update readiness needs a plain --live launch on the installed Preview profile or explicit absolute --history-root and --settings-file".into());
            }
            options.health = Some(
                captures_app::updater::HealthAcknowledgement::new(file, token)
                    .map_err(|error| error.to_string())?,
            );
        }
        _ => {
            return Err(
                "--native-update-ready-file and --native-update-ready-token go together".into(),
            );
        }
    }
    // An installed Preview's resident app uses its imported Preview profile
    // unless a development profile or explicit paths were given.
    options.preview_profile = options.live && !explicit && !options.profile;
    Ok(options)
}

/// Point every path at `dir`, creating it, its history and its Captures
/// folder. A new profile's settings save into `dir/Captures`.
#[cfg(windows)]
fn use_profile(options: &mut Options, dir: &std::path::Path) -> Result<(), String> {
    let dir = std::path::absolute(dir).map_err(|e| format!("--profile: {e}"))?;
    let (history, captures) = (dir.join("history"), dir.join("Captures"));
    for path in [&history, &captures] {
        std::fs::create_dir_all(path).map_err(|e| format!("--profile {}: {e}", path.display()))?;
    }
    let settings_file = dir.join("settings.json");
    if !settings_file.exists() {
        let settings = captures_settings::AppSettings {
            output_directory: captures.to_string_lossy().into_owned(),
            ..Default::default()
        };
        captures_settings::save(&settings_file, &settings).map_err(|e| e.to_string())?;
    }
    options.settings_file = settings_file;
    options.history_root = Some(history);
    options.profile = true;
    Ok(())
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    // The installer builder refuses a package whose host is not this one.
    if std::env::args().nth(1).as_deref() == Some("--preview-build-info") {
        let version = captures_app::preview_channel::version();
        println!(
            "{}",
            serde_json::json!({"renderer": "win32", "version": version})
        );
        return std::process::ExitCode::SUCCESS;
    }
    // An installed Preview holds its package for its whole lifetime, so the
    // update helper never replaces files under a running host.
    let _preview_use = match captures_app::preview_channel::package_use() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("Captures could not start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut options = match parse(std::env::args()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::from(2);
        }
    };
    // An installed Preview arms its separate, renderer-free recovery helper
    // before profile import can fail; development builds start none.
    let recovery = if options.live && options.screenshot.is_none() {
        match captures_app::updater::preview::recovery::Startup::start(options.health.clone()) {
            Ok(recovery) => recovery,
            Err(error) => {
                eprintln!("Startup recovery unavailable: {error}. Continuing launch.");
                None
            }
        }
    } else {
        None
    };
    if let Some(recovery) = &recovery {
        options.health = Some(recovery.acknowledgement.clone());
    }
    if options.preview_profile {
        match captures_app::preview_channel::startup() {
            Ok(Some(startup)) => {
                options.history_root = Some(startup.history_root);
                options.settings_file = startup.settings_file;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("Captures could not start: {error}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Err(error) = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() {
            eprintln!("COM failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
    }
    let result = match &options.screenshot {
        Some(path) => screenshot::render(&options, path),
        None if options.live => match live::elect(&options) {
            // Another process owns this profile and took the relaunch and
            // any files; that is not a failed launch.
            Ok(None) => {
                if let Some(recovery) = &recovery {
                    recovery.finish();
                }
                return std::process::ExitCode::SUCCESS;
            }
            Ok(Some(instance)) => {
                // Only the elected primary consumes the installer's restart
                // intent, before it decides which windows to show.
                if let Some(health) = &options.health {
                    match health.take_restart_intent() {
                        Ok(restart) => options.restart = restart,
                        Err(error) => {
                            eprintln!("Captures could not start: {error}");
                            return std::process::ExitCode::FAILURE;
                        }
                    }
                }
                host::run(options, Some(instance))
            }
            Err(error) => Err(windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32),
                format!("Could not start the resident app: {error}"),
            )),
        },
        None => host::run(options, None),
    };
    match result {
        Ok(()) => {
            // A normal Quit (including one that hands off to the update
            // helper) is not a failed launch.
            if let Some(recovery) = &recovery {
                recovery.finish();
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("captures-windows runs on Windows only; see apps/native/gtk for Linux.");
    std::process::ExitCode::FAILURE
}

#[cfg(all(test, windows))]
mod tests {
    use super::parse_args;
    use std::path::PathBuf;

    fn parse(preview: bool, args: &[&str]) -> Result<super::Options, String> {
        let args = std::iter::once("captures.exe")
            .chain(args.iter().copied())
            .map(String::from);
        parse_args(args, preview)
    }

    #[test]
    fn preview_builds_open_plain_paths_and_file_uris() {
        let options = parse(
            true,
            &[
                r"C:\Users\me\Pictures\shot.png",
                "file:///C:/Users/me/Videos/take%20one.mp4",
            ],
        )
        .unwrap();
        assert!(options.live);
        assert_eq!(
            options.open,
            [
                PathBuf::from(r"C:\Users\me\Pictures\shot.png"),
                PathBuf::from(r"C:\Users\me\Videos\take one.mp4"),
            ]
        );
        // Flags still parse beside files.
        let quiet = parse(true, &["--captures-autostart", r"D:\a.gif"]).unwrap();
        assert!(quiet.quiet);
        assert_eq!(quiet.open, [PathBuf::from(r"D:\a.gif")]);
        // A bare relaunch opens nothing.
        assert!(parse(true, &[]).unwrap().open.is_empty());
    }

    #[test]
    fn remote_or_malformed_uris_and_unknown_flags_are_refused() {
        // A UNC file URI is a Windows path; an unparseable one is refused.
        assert_eq!(
            parse(true, &["file://server/share/a.png"]).unwrap().open,
            [PathBuf::from(r"\\server\share\a.png")]
        );
        assert!(parse(true, &["file://[::1/a.png"]).is_err());
        assert!(parse(true, &["--unknown"]).is_err());
        assert!(parse(true, &["--open-media", ""]).is_err());
        assert!(parse(true, &["--open-media"]).is_err());
    }

    #[test]
    fn development_builds_open_files_only_explicitly_and_live() {
        // A stray path is still an unknown argument.
        assert!(parse(false, &[r"C:\a.png"]).is_err());
        assert!(parse(false, &["--open-media", r"C:\a.png"]).is_err());
        let options = parse(
            false,
            &[
                "--live",
                "--open-media",
                r"C:\a.png",
                "--",
                "--not-a-flag.mp4",
                "file:///C:/b.gif",
            ],
        )
        .unwrap();
        assert_eq!(
            options.open,
            [
                PathBuf::from(r"C:\a.png"),
                PathBuf::from("--not-a-flag.mp4"),
                PathBuf::from(r"C:\b.gif"),
            ]
        );
        // Offscreen renders never open files.
        assert!(parse(true, &["--screenshot", r"C:\x.png", r"C:\a.png"]).is_err());
    }

    #[test]
    fn update_readiness_flags_go_together_on_a_plain_live_launch() {
        let token = "6f1c3b7a-2d4e-4f8a-9b0c-1d2e3f4a5b6c";
        let ready = r"C:\Temp\captures-preview-x\ready";
        let options = parse(
            true,
            &[
                "--live",
                "--native-update-ready-file",
                ready,
                "--native-update-ready-token",
                token,
            ],
        )
        .unwrap();
        assert!(options.health.is_some() && options.preview_profile);
        assert!(parse(true, &["--native-update-ready-file", ready]).is_err());
        assert!(parse(true, &["--native-update-ready-token", token]).is_err());
        // Not a canonical UUIDv4, a relative file, a duplicate, or with files.
        let bad = [
            vec![
                "--native-update-ready-file",
                ready,
                "--native-update-ready-token",
                "x",
            ],
            vec![
                "--native-update-ready-file",
                "ready",
                "--native-update-ready-token",
                token,
            ],
            vec![
                "--native-update-ready-file",
                ready,
                "--native-update-ready-file",
                ready,
                "--native-update-ready-token",
                token,
            ],
            vec![
                "--native-update-ready-file",
                ready,
                "--native-update-ready-token",
                token,
                r"C:\a.png",
            ],
        ];
        for args in bad {
            assert!(parse(true, &args).is_err(), "{args:?}");
        }
        // A development build needs explicit absolute paths.
        assert!(
            parse(
                false,
                &[
                    "--live",
                    "--native-update-ready-file",
                    ready,
                    "--native-update-ready-token",
                    token,
                ],
            )
            .is_err()
        );
    }
}
