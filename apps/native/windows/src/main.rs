//! Captures' Windows host: Win32 windows, DirectComposition, Direct2D,
//! DirectWrite and UI Automation, with Captures' own token-drawn surfaces and
//! controls. Runs beside the wgpu workbench until it passes the feature,
//! screenshot, performance and accessibility gates in `docs/native-rewrite.md`.
//! Release builds are GUI-subsystem; debug builds keep a console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod appearance;
#[cfg(windows)]
mod dialog;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod history;
#[cfg(windows)]
mod history_uia;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod keys;
#[cfg(windows)]
mod scene;
#[cfg(windows)]
mod screenshot;
// The editing model builds everywhere so its tests run on any OS.
mod textfield;
#[cfg(windows)]
mod tokens;
#[cfg(windows)]
mod uia;
#[cfg(windows)]
mod uia_text;

#[cfg(windows)]
#[derive(Clone)]
pub struct Options {
    /// `--scene history`: open Capture History instead of Preferences.
    history: bool,
    appearance: Option<String>,
    settings_file: std::path::PathBuf,
    history_root: Option<std::path::PathBuf>,
    screenshot: Option<std::path::PathBuf>,
    size: Option<(u32, u32)>,
    /// `--find`: render with Find open and this query typed.
    find: Option<String>,
    /// `--profile DIR` or `CAPTURES_NATIVE_PROFILE`: a disposable profile for
    /// tests on a real PC. Settings, history and saves stay under DIR, and the
    /// launch-at-login registry entry is never touched.
    profile: bool,
}

#[cfg(windows)]
fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        history: false,
        appearance: None,
        settings_file: captures_settings::default_native_settings_path(),
        history_root: None,
        screenshot: None,
        size: None,
        find: None,
        profile: false,
    };
    let mut profile = std::env::var_os("CAPTURES_NATIVE_PROFILE").map(std::path::PathBuf::from);
    let mut explicit = false;
    let mut args = args.skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--scene" => {
                options.history = match value()?.as_str() {
                    "preferences" => false,
                    "history" => true,
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
            "--screenshot" => options.screenshot = Some(value()?.into()),
            "--find" => options.find = Some(value()?),
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
        None => host::run(options),
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
