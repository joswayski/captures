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
    /// `--profile DIR` or `CAPTURES_NATIVE_PROFILE`: a disposable profile for
    /// tests on a real PC. Settings, history and saves stay under DIR, and the
    /// launch-at-login registry entry is never touched.
    profile: bool,
    /// `--live`: the resident app with its notification-area icon.
    live: bool,
    /// `--captures-autostart`: a quiet resident launch that opens no window.
    quiet: bool,
}

#[cfg(windows)]
fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
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
        profile: false,
        live: false,
        quiet: false,
    };
    let mut profile = std::env::var_os("CAPTURES_NATIVE_PROFILE").map(std::path::PathBuf::from);
    let mut explicit = false;
    let mut args = args.skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--scene" => {
                options.scene = match value()?.as_str() {
                    "preferences" => Scene::Preferences,
                    "history" => Scene::History,
                    "editor" => Scene::Editor,
                    "recording" => Scene::Recording,
                    "capture" => Scene::Capture,
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
    if matches!(options.scene, Scene::Editor | Scene::Recording) && options.artifact.is_none() {
        return Err("--scene editor/recording needs --artifact ID".into());
    }
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
    let options = match parse(std::env::args()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::from(2);
        }
    };
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
            // Another process owns this profile and took the relaunch.
            Ok(None) => return std::process::ExitCode::SUCCESS,
            Ok(Some(instance)) => host::run(options, Some(instance)),
            Err(error) => Err(windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32),
                format!("Could not start the resident app: {error}"),
            )),
        },
        None => host::run(options, None),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
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
