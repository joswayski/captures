use gpui::*;
use std::path::PathBuf;

pub mod editor;
pub mod effects;
pub mod integration;
pub mod notices;
pub mod preferences;
pub mod previews;
pub mod recording;
pub mod theme;

#[derive(Clone, Debug)]
pub struct Launch {
    pub view: String,
    pub path: Option<PathBuf>,
    pub light: bool,
    pub profile: PathBuf,
    /// Explicit visual fixture mode; never confused with real captures.
    pub mock: bool,
}

impl Launch {
    pub fn parse() -> anyhow::Result<Self> {
        let mut launch = Self {
            view: "preferences".into(),
            path: None,
            light: false,
            profile: std::env::var_os("CAPTURES_GPUI_DATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::var_os("XDG_DATA_HOME")
                        .map(PathBuf::from)
                        .or_else(|| {
                            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share"))
                        })
                        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
                        .unwrap_or_else(std::env::temp_dir)
                        .join("captures-gpui-experiment")
                }),
            mock: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--view" => {
                    launch.view = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--view requires a name"))?
                }
                "--open" => {
                    launch.path = Some(
                        args.next()
                            .ok_or_else(|| anyhow::anyhow!("--open requires a path"))?
                            .into(),
                    )
                }
                "--profile" => {
                    launch.profile = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--profile requires a path"))?
                        .into()
                }
                "--light" => launch.light = true,
                "--dark" => launch.light = false,
                "--mock" => launch.mock = true,
                "--preferences" => launch.view = "preferences".into(),
                "--history" => launch.view = "history".into(),
                "--capture" => launch.view = "recording-selector".into(),
                "--help" => {
                    println!(
                        "Captures GPUI experiment\n--view NAME --open FILE --profile DIR --light --dark --mock\nNot a Preview replacement. Profiles are isolated from Tauri."
                    );
                    std::process::exit(0);
                }
                _ => anyhow::bail!("Unknown argument: {arg}"),
            }
        }
        Ok(launch)
    }
}

pub fn open_view(view: &str, mut launch: Launch, cx: &mut App) -> anyhow::Result<()> {
    launch.view = view.to_owned();
    let previous = cx
        .windows()
        .iter()
        .map(AnyWindowHandle::window_id)
        .collect::<Vec<_>>();
    match view {
        "background" => Ok(()),
        "preferences" | "history" | "feedback" | "onboarding" => preferences::open(launch, cx),
        "screenshot-editor" | "viewer" => editor::open(launch, cx),
        "thumbnail" => previews::push(launch, cx),
        "startup"
        | "launch-notice"
        | "recording-ready"
        | "recording-saved"
        | "recording-save-error"
        | "recording-controls-hidden" => notices::open_fixture(launch, cx),
        "overlay"
        | "recording-selector"
        | "recording-hud"
        | "recording-editor"
        | "recording-countdown"
        | "screenshot-countdown"
        | "recording-region-indicator" => recording::open(launch, cx),
        _ => anyhow::bail!("Surface {view:?} is not implemented in the GPUI experiment"),
    }?;
    for handle in cx.windows() {
        if !previous.contains(&handle.window_id()) {
            present_window(handle, cx)?;
        }
    }
    Ok(())
}

pub fn present_window(handle: AnyWindowHandle, cx: &mut App) -> anyhow::Result<()> {
    handle.update(cx, |_, window, _| {
        window.activate_window();
    })?;
    refresh_window(handle, cx)
}

/// Refresh passive surfaces without stealing keyboard focus from the desktop.
pub fn refresh_window(handle: AnyWindowHandle, cx: &mut App) -> anyhow::Result<()> {
    handle.update(cx, |_, window, _| window.refresh())?;
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        && std::env::var_os("CAPTURES_GPUI_X11_RESIZE_WORKAROUND").is_some()
    {
        // GPUI 0.2.2 + Xvfb presents a blank initial surface until a
        // real ConfigureNotify resize. Opt in only in that software-rendered
        // lab. Shrink first: a fullscreen window cannot grow past the screen.
        cx.spawn(async move |cx| {
            Timer::after(std::time::Duration::from_millis(500)).await;
            if let Ok(original) = handle.update(cx, |_, window, _| {
                let original = window.viewport_size();
                window.resize(size(
                    (original.width - px(20.)).max(px(1.)),
                    (original.height - px(20.)).max(px(1.)),
                ));
                original
            }) {
                Timer::after(std::time::Duration::from_millis(100)).await;
                let _ = handle.update(cx, |_, window, _| window.resize(original));
            }
        })
        .detach();
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if std::env::args().skip(1).eq(["--benchmark-effects"]) {
        effects::benchmark();
        return Ok(());
    }
    let launch = Launch::parse()?;
    if !launch.mock
        && let Err(error) = preferences::history::prune_capture_history(&launch.profile)
    {
        eprintln!("Could not prune Capture History: {error:#}");
    }
    if !launch.mock
        && let Err(error) = recording::recovery::prune_gif_sources(&launch.profile)
    {
        eprintln!("Could not prune retained GIF sources: {error:#}");
    }
    Application::new().run(move |cx| {
        let settings = preferences::settings::load(&launch.profile).ok();
        if let Some(settings) = settings.clone() {
            cx.set_global(theme::CurrentSettings(settings));
        }
        if let Err(error) = integration::install(launch.clone(), cx) {
            eprintln!("Could not initialize native integration: {error:#}");
            cx.defer(move |cx| integration::show_native_error(error, cx));
        }
        if let Err(error) = open_view(&launch.view.clone(), launch.clone(), cx) {
            eprintln!("Could not open Captures GPUI: {error:#}");
            cx.quit();
        }
        if launch.view == "background"
            && !launch.mock
            && settings
                .as_ref()
                .is_some_and(|settings| settings.onboarding_completed)
        {
            let shortcut = settings
                .as_ref()
                .map(|settings| notices::shortcut_tokens(&settings.new_capture_shortcut))
                .unwrap_or_default();
            notices::schedule_launch(shortcut, launch.clone(), cx);
        }
        cx.activate(true);
    });
    Ok(())
}
