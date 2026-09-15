use gpui::*;
use std::path::PathBuf;

pub mod editor;
pub mod effects;
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
        "preferences" | "history" | "feedback" | "onboarding" => preferences::open(launch, cx),
        "screenshot-editor" | "viewer" => editor::open(launch, cx),
        "thumbnail" => previews::open(launch, cx),
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
        window.refresh();
    })?;
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        // GPUI 0.2.2 + Xvfb presents a blank initial surface until a
        // real ConfigureNotify resize. Restore the requested size after
        // one nudge; refresh alone does not initialize this renderer.
        cx.spawn(async move |cx| {
            Timer::after(std::time::Duration::from_millis(200)).await;
            if let Ok(original) = handle.update(cx, |_, window, _| {
                let original = window.viewport_size();
                window.resize(size(original.width + px(1.), original.height));
                original
            }) {
                Timer::after(std::time::Duration::from_millis(50)).await;
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
    Application::new().run(move |cx| {
        if let Err(error) = open_view(&launch.view.clone(), launch, cx) {
            eprintln!("Could not open Captures GPUI: {error:#}");
            cx.quit();
        }
        cx.activate(true);
    });
    Ok(())
}
