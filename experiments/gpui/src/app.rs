//! Application lifecycle shared by every GPUI surface.
use crate::{
    capture::{self, Selection, Selector},
    desktop::{self, Action, Desktop, Instance},
    editor, history, preferences, preview, recording,
    settings::Settings,
    ui,
};
use captures_capture::CaptureMode;
use captures_recording::{AudioOptions, GifOptions, RecordingKind, RecordingOptions};
use gpui::{
    AnyWindowHandle, App, Bounds, ClipboardItem, Context, Global, Image, ImageFormat,
    PathPromptOptions, Render, Window, WindowAppearance, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, prelude::*, px, size,
};
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Completion = Box<dyn FnOnce(&mut App)>;

#[derive(Default)]
struct State {
    generation: u64,
    active: bool,
    selector: Option<WindowHandle<Selector>>,
    countdown: Option<AnyWindowHandle>,
    completion: Option<Completion>,
    desktop: Option<Desktop>,
    instance: Option<Instance>,
    _keepalive: Option<AnyWindowHandle>,
}
impl Global for State {}

// Published GPUI 0.2.2 stops X11's event loop on its last window release and
// has no QuitMode API. An unmapped 1×1 window keeps tray/background work alive.
struct KeepAlive;
impl Render for KeepAlive {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub fn run(instance: Instance, args: Vec<String>, cx: &mut App) {
    let settings = match Settings::load() {
        Ok(settings) => settings,
        Err(error) => {
            ui::error(format!("Cannot load settings: {error}"), cx);
            Settings::default()
        }
    };
    cx.set_global(settings);
    cx.set_global(State {
        instance: Some(instance),
        ..Default::default()
    });
    match cx.open_window(
        WindowOptions {
            show: false,
            focus: false,
            titlebar: None,
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                gpui::point(px(0.), px(0.)),
                size(px(1.), px(1.)),
            ))),
            ..Default::default()
        },
        |window, cx| {
            window
                .observe_window_appearance(|_, cx| {
                    if cx.global::<Settings>().appearance == "system" {
                        settings_changed(cx);
                    }
                })
                .detach();
            cx.new(|_| KeepAlive)
        },
    ) {
        Ok(window) => cx.global_mut::<State>()._keepalive = Some(window.into()),
        Err(error) => {
            ui::error(format!("Cannot initialize tray lifecycle: {error}"), cx);
            return;
        }
    }
    match Desktop::new() {
        Ok(desktop) => cx.global_mut::<State>().desktop = Some(desktop),
        Err(error) => ui::error(format!("Desktop integration could not start: {error}"), cx),
    }
    settings_changed(cx);
    command(&args, cx);
    if let Err(error) = recording::recover(cx) {
        ui::error(error.to_string(), cx);
    }
    cx.spawn(async move |cx| {
        let mut last_safety = Instant::now();
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
            let _ = cx.update(|cx| {
                let mut events = cx
                    .global::<State>()
                    .desktop
                    .as_ref()
                    .map(Desktop::events)
                    .unwrap_or_default();
                if let Some(instance) = &cx.global::<State>().instance {
                    events.extend(instance.commands());
                }
                for event in events {
                    action(event, cx);
                }
            });
            if last_safety.elapsed() >= Duration::from_millis(400) {
                let safe = cx
                    .background_executor()
                    .spawn(async { captures_session::capture_session_available() })
                    .await;
                let _ = cx.update(|cx| {
                    if !safe {
                        if cx.global::<State>().active {
                            cancel_capture(cx);
                        }
                        preview::hide(cx);
                    }
                });
                last_safety = Instant::now();
            }
        }
    })
    .detach();
}

pub fn settings_changed(cx: &mut App) {
    let settings = cx.global::<Settings>().clone();
    let dark = match settings.appearance.as_str() {
        "light" => false,
        "dark" => true,
        _ => matches!(
            cx.window_appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let mut theme = ui::Theme {
        dark,
        ..Default::default()
    };
    theme.accent = ui::accent(&settings.theme, &settings.custom_accent);
    theme.signal = ui::signal(&settings.theme, &settings.custom_signal);
    cx.set_global(theme);
    if let Some(desktop) = cx.global_mut::<State>().desktop.as_mut()
        && let Err(error) = desktop.replace_shortcuts(&settings)
    {
        ui::error(error, cx);
    }
    if let Err(error) = desktop::login(settings.launch_at_login) {
        ui::error(error, cx);
    }
    cx.refresh_windows();
}

fn action(action: Action, cx: &mut App) {
    match action {
        Action::Capture(mode, kind) => begin(mode, kind, None, cx),
        Action::Cancel => cancel_capture(cx),
        Action::Preferences => {
            if let Err(e) = preferences::open(cx) {
                ui::error(e.to_string(), cx);
            }
        }
        Action::History => {
            if let Err(e) = history::open(cx) {
                ui::error(e.to_string(), cx);
            }
        }
        Action::Open => choose_file(cx),
        Action::Previews => {
            if let Err(e) = preview::show(cx) {
                ui::error(e.to_string(), cx);
            }
        }
        Action::RestoreControls => recording::restore_controls(cx),
        Action::Arguments(args) => command(&args, cx),
        Action::Quit => recording::request_quit(cx),
    }
}

pub fn command(args: &[String], cx: &mut App) {
    let mut args = args.to_vec();
    if let Some(index) = args.iter().position(|a| a == "--appearance")
        && index + 1 < args.len()
    {
        cx.global_mut::<Settings>().appearance = args[index + 1].clone();
        args.drain(index..index + 2);
        settings_changed(cx);
    }
    match args.first().map(String::as_str) {
        Some("--background") => {}
        Some("--capture") => capture(cx),
        Some("--record") => begin(CaptureMode::Region, 1, None, cx),
        Some("--gif") => begin(CaptureMode::Region, 2, None, cx),
        Some("--window") => begin(CaptureMode::Window, 0, None, cx),
        Some("--display") => begin(CaptureMode::Display, 0, None, cx),
        Some("--history") => action(Action::History, cx),
        Some("--open") => {
            for path in args.iter().skip(1) {
                open(path.into(), cx);
            }
        }
        Some("--previews") => {
            for path in args.iter().skip(1) {
                if let Err(e) = preview::add(path.into(), cx) {
                    ui::error(e.to_string(), cx);
                }
            }
        }
        Some("--canvas") => {
            if let Err(e) = editor::canvas(1280, 720, cx) {
                ui::error(e.to_string(), cx);
            }
        }
        Some(path) if !path.starts_with('-') => {
            for path in args {
                open(path.into(), cx);
            }
        }
        _ => action(Action::Preferences, cx),
    }
}

pub fn open(path: PathBuf, cx: &mut App) {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let result = if matches!(extension.as_str(), "mp4" | "webm" | "gif" | "mov" | "mkv") {
        recording::open(path, cx)
    } else {
        editor::open(path, cx)
    };
    if let Err(error) = result {
        ui::error(error.to_string(), cx);
    }
}

pub fn restore_preview(path: PathBuf, cx: &mut App) {
    if let Err(error) = preview::add(path, cx) {
        ui::error(error.to_string(), cx);
    }
}

fn choose_file(cx: &mut App) {
    let request = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some("Open capture".into()),
    });
    cx.spawn(async move |cx| match request.await {
        Ok(Ok(Some(paths))) => {
            let _ = cx.update(|cx| {
                for path in paths {
                    open(path, cx);
                }
            });
        }
        Ok(Err(error)) => {
            let _ = cx.update(|cx| ui::error(error.to_string(), cx));
        }
        _ => {}
    })
    .detach();
}

pub fn saved(path: PathBuf, cx: &mut App) {
    let for_preview = path.clone();
    ui::job(
        cx,
        move || history::add(&path),
        move |result, cx| {
            if let Err(error) = result {
                ui::error(error, cx);
            }
            if cx.global::<Settings>().show_mini_previews
                && let Err(error) = preview::add(for_preview, cx)
            {
                ui::error(error.to_string(), cx);
            }
        },
    );
}

pub fn capture(cx: &mut App) {
    begin(CaptureMode::Region, 0, None, cx);
}

pub fn capture_with_completion(done: Completion, cx: &mut App) {
    cancel_capture(cx);
    cx.global_mut::<State>().completion = Some(done);
    begin(CaptureMode::Region, 0, None, cx);
}

pub fn select_display(mode: CaptureMode, kind: u32, id: String, cx: &mut App) {
    cx.global_mut::<State>().selector = None;
    begin(mode, kind, Some(id), cx);
}

fn begin(mode: CaptureMode, kind: u32, display: Option<String>, cx: &mut App) {
    if display.is_none()
        && let Some(selector) = cx.global::<State>().selector
        && selector
            .update(cx, |view, _, cx| view.switch(mode, kind, cx))
            .is_ok()
    {
        return;
    }
    let settings = cx.global::<Settings>().clone();
    if !settings.include_mini_previews_in_captures {
        preview::hide(cx);
    }
    let state = cx.global_mut::<State>();
    state.generation = state.generation.wrapping_add(1).max(1);
    state.active = true;
    if let Some(desktop) = state.desktop.as_mut() {
        desktop.escape(true);
    }
    let generation = state.generation;
    ui::job(
        cx,
        move || {
            std::thread::sleep(Duration::from_millis(80));
            capture::prepare(display, desktop::pointer())
        },
        move |result, cx| {
            if !current(generation, cx) {
                return;
            }
            match result {
                Ok(prepared) => match capture::open(prepared, settings, mode, kind, cx) {
                    Ok(window) => cx.global_mut::<State>().selector = Some(window),
                    Err(error) => {
                        cancel_capture(cx);
                        ui::error(error.to_string(), cx);
                    }
                },
                Err(error) => {
                    cancel_capture(cx);
                    ui::error(error, cx);
                }
            }
        },
    );
}

fn current(generation: u64, cx: &App) -> bool {
    let state = cx.global::<State>();
    state.active && state.generation == generation
}

pub fn cancel_capture(cx: &mut App) {
    let state = cx.global_mut::<State>();
    state.generation = state.generation.wrapping_add(1);
    state.active = false;
    let selector = state.selector.take();
    let countdown = state.countdown.take();
    let completion = state.completion.take();
    if let Some(desktop) = state.desktop.as_mut() {
        desktop.escape(false);
    }
    if let Some(window) = selector {
        let _ = window.update(cx, |_, window, _| window.remove_window());
    }
    if let Some(window) = countdown {
        let _ = window.update(cx, |_, window, _| window.remove_window());
    }
    let _ = preview::show(cx);
    if let Some(completion) = completion {
        completion(cx);
    }
}

pub fn selected(selection: Selection, cx: &mut App) {
    cx.global_mut::<State>().selector = None;
    if selection.kind != 0 {
        let settings = &selection.settings.recording;
        let gif = selection.kind == 2;
        let options = RecordingOptions {
            kind: if gif {
                RecordingKind::Gif
            } else {
                RecordingKind::Video
            },
            target: selection.target,
            frames_per_second: if gif {
                settings.gif_fps
            } else {
                settings.video_fps
            },
            max_resolution: settings.video_max_resolution,
            countdown_seconds: settings.countdown_seconds,
            show_cursor: settings.show_cursor,
            highlight_clicks: settings.highlight_clicks,
            show_keystrokes: settings.show_keystrokes,
            audio: if gif {
                AudioOptions::default()
            } else {
                AudioOptions {
                    capture_system_audio: settings.capture_system_audio,
                    microphone_device_id: settings.microphone_device_id.clone(),
                    mono_output: settings.mono_audio,
                    ..Default::default()
                }
            },
            gif: GifOptions {
                max_width: settings.gif_max_width,
                max_colors: settings.gif_max_colors,
                optimize: true,
            },
        };
        // End selection before starting the recorder; it owns its countdown/lock state.
        cancel_capture(cx);
        if let Err(error) = recording::start(
            options,
            selection.display,
            selection.settings.output_directory,
            cx,
        ) {
            ui::error(error.to_string(), cx);
        }
        return;
    }
    let generation = cx.global::<State>().generation;
    let seconds = selection.settings.screenshot_countdown_seconds;
    if seconds == 0 {
        take_screenshot(selection, generation, cx);
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
    let bounds = Bounds::centered(None, size(px(220.), px(180.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::PopUp,
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        window.set_window_title("Captures GPUI Countdown");
        cx.new(|_| Countdown { deadline })
    }) {
        Ok(window) => cx.global_mut::<State>().countdown = Some(window.into()),
        Err(error) => {
            cancel_capture(cx);
            ui::error(error.to_string(), cx);
            return;
        }
    }
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_secs(u64::from(seconds)))
            .await;
        let _ = cx.update(|cx| {
            if !current(generation, cx) {
                return;
            }
            if let Some(window) = cx.global_mut::<State>().countdown.take() {
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
            take_screenshot(selection, generation, cx);
        });
    })
    .detach();
}

fn take_screenshot(selection: Selection, generation: u64, cx: &mut App) {
    let copy = selection.settings.auto_copy_to_clipboard;
    ui::job(
        cx,
        move || {
            std::thread::sleep(Duration::from_millis(100));
            let image = capture::pixels(&selection)?;
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut png, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            Ok((
                png.into_inner(),
                crate::settings::data_dir().join("unsaved"),
            ))
        },
        move |result, cx| {
            if !current(generation, cx) {
                return;
            }
            match result {
                Ok((png, directory)) => {
                    let clipboard = copy.then(|| Image::from_bytes(ImageFormat::Png, png.clone()));
                    ui::job(
                        cx,
                        move || publish_capture(&directory, &png, "png"),
                        move |result, cx| {
                            // Publication was committed after a current-generation check above.
                            // Cancellation after that check may close UI, but never deletes a completed file.
                            if let Some(image) = clipboard {
                                cx.write_to_clipboard(ClipboardItem::new_image(&image));
                            }
                            match result {
                                Ok(path) => saved(path, cx),
                                Err(error) => ui::error(error, cx),
                            }
                            if current(generation, cx) {
                                cancel_capture(cx);
                            }
                        },
                    );
                }
                Err(error) => {
                    cancel_capture(cx);
                    ui::error(error, cx);
                }
            }
        },
    );
}

pub(crate) fn publish_capture(
    directory: &std::path::Path,
    bytes: &[u8],
    extension: &str,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    temp.write_all(bytes).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    for index in 0..1000 {
        let path = directory.join(format!("Capture-{timestamp}-{index}.{extension}"));
        match temp.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temp = error.file
            }
            Err(error) => return Err(error.error.to_string()),
        }
    }
    Err("Could not choose a new screenshot filename".into())
}

struct Countdown {
    deadline: Instant,
}
impl Render for Countdown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = ui::theme(cx);
        let remaining = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f32();
        if remaining > 0. {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(ui::metric("--s-6"))
            .rounded(ui::metric("--r-2xl"))
            .bg(p.glass())
            .text_color(p.glass_text())
            .child(
                div()
                    .text_size(ui::metric("--text-3xl"))
                    .child(format!("{}", remaining.ceil() as u32)),
            )
            .child(
                ui::button("cancel-countdown", "Cancel · Esc", p).on_click(|_, window, cx| {
                    window.remove_window();
                    cancel_capture(cx);
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screenshot_publication_never_overwrites_and_preserves_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"asymmetric screenshot fixture";
        let a = publish_capture(dir.path(), bytes, "png").unwrap();
        let b = publish_capture(dir.path(), b"second", "webp").unwrap();
        assert_eq!(b.extension().unwrap(), "webp");
        assert_ne!(a, b);
        assert_eq!(std::fs::read(a).unwrap(), bytes);
        assert_eq!(std::fs::read(b).unwrap(), b"second");
    }
}
