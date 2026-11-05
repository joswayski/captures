//! Captures' Windows host: Win32 windows, DirectComposition, Direct2D,
//! DirectWrite and UI Automation, with Captures' own token-drawn surfaces and
//! controls. Runs beside the wgpu workbench until it passes the feature,
//! screenshot, performance and accessibility gates in `docs/native-rewrite.md`.
//! Release builds are GUI-subsystem; debug builds keep a console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod appearance;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod scene;
#[cfg(windows)]
mod screenshot;
#[cfg(windows)]
mod tokens;
#[cfg(windows)]
mod uia;

#[cfg(windows)]
pub struct Options {
    appearance: Option<String>,
    settings_file: std::path::PathBuf,
    history_root: Option<std::path::PathBuf>,
    screenshot: Option<std::path::PathBuf>,
    size: Option<(u32, u32)>,
}

#[cfg(windows)]
fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        appearance: None,
        settings_file: captures_settings::default_native_settings_path(),
        history_root: None,
        screenshot: None,
        size: None,
    };
    let mut args = args.skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--scene" => {
                let scene = value()?;
                if scene != "preferences" {
                    return Err(format!("unsupported scene {scene}"));
                }
            }
            "--appearance" => {
                let appearance = value()?;
                if appearance != "light" && appearance != "dark" {
                    return Err("--appearance is light or dark".into());
                }
                options.appearance = Some(appearance);
            }
            "--settings-file" => options.settings_file = value()?.into(),
            "--history-root" => options.history_root = Some(value()?.into()),
            "--screenshot" => options.screenshot = Some(value()?.into()),
            "--size" => {
                let size = value()?;
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                let parse = |v: &str| v.parse::<u32>().ok().filter(|v| (1..=16384).contains(v));
                options.size = Some((parse(w).ok_or("bad width")?, parse(h).ok_or("bad height")?));
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    Ok(options)
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
