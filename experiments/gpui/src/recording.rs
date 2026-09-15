//! Native GPUI capture, recording, HUD, and recording-editor surfaces.
//!
//! Recording/export uses the repository backends. The editor requires `ffmpeg`
//! and `ffprobe` on PATH; failures are shown and are never replaced by fixtures.
mod countdown;
mod editor;
mod indicator;
mod model;
pub mod recovery;
mod screenshot;

use crate::{
    Launch,
    theme::{self, Theme},
};
use anyhow::Context as _;
use captures_capture::{
    DisplayDescriptor, DisplayFrame, PointerCursor, WindowDescriptor, XcapBackend,
};
use captures_recording::{
    RecordingKind, RecordingOptions, RecordingSegmentInfo, RecordingState, RecordingTarget,
};
use gpui::{prelude::*, *};
use model::{ActionMode, Lifecycle, Rect, Settings, TargetMode};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

struct RecordingRegistry {
    controller: Entity<Selector>,
    window: WindowHandle<Selector>,
}
impl Global for RecordingRegistry {}

pub struct RecoveryInProgress(pub String);
impl Global for RecoveryInProgress {}

pub fn active_session(cx: &App) -> Option<String> {
    cx.try_global::<RecordingRegistry>()
        .and_then(|registry| registry.controller.read(cx).journal.as_ref())
        .map(|journal| journal.manifest.session_id.clone())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HudConfirmation {
    Restart,
    Delete,
}

struct HudTooltip(&'static str);
impl Render for HudTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::new(false);
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(theme.glass)
            .text_color(theme.glass_text)
            .text_size(px(12.))
            .child(self.0)
    }
}

fn hud_icon(id: &str, muted: bool, theme: Theme) -> Img {
    let body = match id {
        "stop-hud" => {
            r#"<rect x="5" y="5" width="14" height="14" rx="2" fill="currentColor" stroke="none"/>"#
        }
        "pause-hud" => r#"<path d="M8 5v14M16 5v14"/>"#,
        "resume-hud" => r#"<path d="m8 5 11 7-11 7Z"/>"#,
        "restart-hud" => r#"<path d="M4 11a8 8 0 1 1 2 5.3M4 5v6h6"/>"#,
        "screenshot-hud" => {
            r#"<path d="M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2M12 8.5c.4 1.8 1.7 3.1 3.5 3.5-1.8.4-3.1 1.7-3.5 3.5-.4-1.8-1.7-3.1-3.5-3.5 1.8-.4 3.1-1.7 3.5-3.5Z"/>"#
        }
        "microphone-hud" => {
            r#"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M6 11a6 6 0 0 0 11.4 2.6M12 18v3M9 21h6"/>"#
        }
        "delete-hud" => r#"<path d="M4 7h16M9 7V4h6v3m3 0-1 13H7L6 7m4 4v5m4-5v5"/>"#,
        "hide-hud" => {
            r#"<path d="m2 2 20 20M6.7 6.7C4.9 8 3.7 9.7 3 12c1.7 4.1 5 7 9 7 1.8 0 3.5-.6 4.9-1.6M10.7 5.1A10.9 10.9 0 0 1 12 5c4 0 7.3 2.9 9 7-.3.8-.7 1.5-1.2 2.2M14.1 14.1a3 3 0 0 1-4.2-4.2"/>"#
        }
        _ => "",
    };
    let color = theme.glass_text;
    let muted_path = if id == "microphone-hud" && muted {
        r#"<path d="m4 4 16 16"/>"#
    } else {
        ""
    };
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" color="#{:02x}{:02x}{:02x}" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}{muted_path}</svg>"##,
        (color.r * 255.) as u8,
        (color.g * 255.) as u8,
        (color.b * 255.) as u8
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .size(px(16.))
}

#[derive(Clone, Copy, Debug)]
struct RecordingClock {
    recorded: Duration,
    running_since: Option<Instant>,
}
impl Default for RecordingClock {
    fn default() -> Self {
        Self {
            recorded: Duration::ZERO,
            running_since: None,
        }
    }
}
impl RecordingClock {
    fn start(&mut self, now: Instant) {
        self.running_since = Some(now);
    }
    fn pause(&mut self, now: Instant) {
        if let Some(start) = self.running_since.take() {
            self.recorded += now.saturating_duration_since(start);
        }
    }
    fn reset(&mut self, now: Instant) {
        self.recorded = Duration::ZERO;
        self.running_since = Some(now);
    }
    fn elapsed(&self, now: Instant) -> Duration {
        self.recorded
            + self
                .running_since
                .map(|start| now.saturating_duration_since(start))
                .unwrap_or_default()
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
type NativeSegment = captures_recording_xcap::XcapRecordingSegment;
#[cfg(target_os = "macos")]
type NativeSegment = captures_recording_macos::MacRecordingSegment;

struct Selector {
    launch: Launch,
    mode: ActionMode,
    target: TargetMode,
    gif: bool,
    settings: Settings,
    preferences: crate::preferences::settings::Settings,
    frozen: Option<Arc<DisplayFrame>>,
    backdrop: Option<Arc<RenderImage>>,
    cursor: Option<PointerCursor>,
    displays: Vec<DisplayDescriptor>,
    windows: Vec<WindowDescriptor>,
    display: usize,
    segment: Option<NativeSegment>,
    journal: Option<recovery::Journal>,
    completed: Vec<RecordingSegmentInfo>,
    selection: Option<Rect>,
    drag_start: Option<(f32, f32)>,
    selection_edit: Option<(Rect, usize, (f32, f32))>,
    selected_window: Option<String>,
    countdown: Option<u8>,
    countdown_started: Option<Instant>,
    countdown_exiting: bool,
    busy: bool,
    paused: bool,
    hidden: bool,
    status: String,
    lifecycle: Lifecycle,
    in_hud: bool,
    hud: Option<WindowHandle<Selector>>,
    indicator: Option<WindowHandle<indicator::Indicator>>,
    controls_excluded: bool,
    focus: Option<FocusHandle>,
    clock: RecordingClock,
    microphone_muted: bool,
    confirmation: Option<HudConfirmation>,
}
impl Selector {
    fn new(launch: Launch) -> anyhow::Result<Self> {
        let preferences = crate::preferences::settings::load(&launch.profile)?;
        let settings = Settings::load(&launch.profile);
        let displays = if launch.mock || !captures_session::capture_session_available() {
            Vec::new()
        } else {
            XcapBackend.displays().unwrap_or_default()
        };
        let status = if launch.mock {
            "Visual fixture — capture actions disabled".into()
        } else if !captures_session::capture_session_available() {
            "Capture is unavailable while the desktop session is locked or inactive".into()
        } else if displays.is_empty() {
            "No capturable display is available".into()
        } else {
            String::new()
        };
        let cursor = crate::integration::pointer_cursor();
        let display = cursor
            .as_ref()
            .and_then(|cursor| {
                displays.iter().position(|d| {
                    cursor.position.0 >= d.x
                        && cursor.position.1 >= d.y
                        && i64::from(cursor.position.0) < i64::from(d.x) + i64::from(d.width)
                        && i64::from(cursor.position.1) < i64::from(d.y) + i64::from(d.height)
                })
            })
            .or_else(|| displays.iter().position(|d| d.is_primary))
            .unwrap_or(0);
        let frozen = if !launch.mock && preferences.freeze_screen && !displays.is_empty() {
            XcapBackend.ensure_permission(true)?;
            Some(Arc::new(
                XcapBackend.capture_display(&displays[display].id)?,
            ))
        } else {
            None
        };
        let backdrop = frozen.as_ref().map(|frame| {
            let mut image = frame.image.clone();
            for p in image.pixels_mut() {
                p.0.swap(0, 2);
            }
            Arc::new(RenderImage::new([image::Frame::new(image)]))
        });
        let windows = if launch.mock || displays.is_empty() {
            Vec::new()
        } else {
            XcapBackend.windows().unwrap_or_default()
        };
        Ok(Self {
            launch,
            mode: ActionMode::Screenshot,
            target: TargetMode::Region,
            gif: false,
            settings,
            preferences,
            frozen,
            backdrop,
            cursor,
            displays,
            windows,
            display,
            segment: None,
            journal: None,
            completed: Vec::new(),
            selection: None,
            drag_start: None,
            selection_edit: None,
            selected_window: None,
            countdown: None,
            countdown_started: None,
            countdown_exiting: false,
            busy: false,
            paused: false,
            hidden: false,
            status,
            lifecycle: Lifecycle::default(),
            in_hud: false,
            hud: None,
            indicator: None,
            controls_excluded: false,
            focus: None,
            clock: RecordingClock::default(),
            microphone_muted: false,
            confirmation: None,
        })
    }
    fn chosen_display(&self) -> Option<&DisplayDescriptor> {
        self.displays.get(self.display)
    }
    fn target(&self) -> Option<RecordingTarget> {
        let d = self.chosen_display()?;
        Some(match self.target {
            TargetMode::Display => RecordingTarget::Display {
                display_id: d.id.clone(),
            },
            TargetMode::Region => {
                let rect = self.selection.filter(|rect| rect.valid())?;
                RecordingTarget::Region {
                    display_id: d.id.clone(),
                    rect: captures_recording::CaptureRect {
                        x: rect.x.round() as i32,
                        y: rect.y.round() as i32,
                        width: rect.width.round() as u32,
                        height: rect.height.round() as u32,
                    },
                }
            }
            TargetMode::Window => RecordingTarget::Window {
                window_id: self.selected_window.clone()?,
            },
        })
    }
    fn capture(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.launch.mock || self.busy || self.countdown.is_some() || self.segment.is_some() {
            return;
        }
        if self.target().is_none() {
            self.status = "Choose a capture target first".into();
            cx.notify();
            return;
        }
        let seconds = self.preferences.screenshot_countdown_seconds;
        if seconds == 0 {
            self.capture_now(window, cx);
            return;
        }
        let Some(token) = self.lifecycle.begin_countdown() else {
            return;
        };
        self.busy = true;
        self.countdown = Some(seconds);
        self.countdown_started = Some(Instant::now());
        self.countdown_exiting = false;
        // A delayed shot must sample the future desktop, not the frozen frame
        // from when the selector opened.
        self.frozen = None;
        self.backdrop = None;
        cx.spawn_in(window, async move |this, cx| {
            for remaining in (1..=seconds).rev() {
                Timer::after(Duration::from_secs(1)).await;
                let keep_going = this
                    .update(cx, |s, cx| {
                        if !s.lifecycle.current(token)
                            || !captures_session::capture_session_available()
                        {
                            s.busy = false;
                            s.countdown = None;
                            s.status = "Capture cancelled".into();
                            cx.notify();
                            return false;
                        }
                        s.countdown = Some(remaining.saturating_sub(1).max(1));
                        s.countdown_exiting = remaining == 1;
                        if s.countdown_exiting {
                            s.countdown_started = Some(Instant::now());
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
            Timer::after(Duration::from_millis(if theme::reduced_motion() {
                0
            } else {
                140
            }))
            .await;
            let _ = this.update_in(cx, |s, window, cx| {
                if !s.lifecycle.current(token) {
                    return;
                }
                s.countdown = None;
                s.lifecycle.cancel();
                s.capture_now(window, cx);
            });
        })
        .detach();
        cx.notify();
    }
    fn capture_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.target() else {
            return;
        };
        let Some(display) = self.chosen_display() else {
            return;
        };
        let display_id = display.id.clone();
        let profile = self.launch.profile.clone();
        let frozen = self.frozen.clone();
        let windows = self.windows.clone();
        let cursor = if self.preferences.show_cursor_in_screenshots {
            if frozen.is_some() {
                self.cursor.clone()
            } else {
                crate::integration::pointer_cursor()
            }
        } else {
            None
        };
        let copy = self.preferences.auto_copy_to_clipboard;
        let show_preview = self.preferences.show_mini_previews;
        self.busy = true;
        self.hidden = true;
        self.status = "Capturing…".into();
        cx.notify();
        // Keep a managed window alive until the result opens: closing the last
        // window can exit GPUI while the capture task is still running.
        // App::hide is a no-op on Linux, whereas minimize unmaps this window.
        let mut launch = self.launch.clone();
        let selector = window.window_handle();
        window.minimize_window();
        let task = cx.background_executor().spawn(async move {
            // Allow the window manager to unmap the selector before Xcap.
            std::thread::sleep(Duration::from_millis(120));
            screenshot::capture(
                &profile,
                target,
                &display_id,
                frozen,
                &windows,
                cursor.as_ref(),
            )
        });
        cx.spawn(async move |_, cx| {
            let result = task.await;
            if let Err(error) = cx.update(|cx| {
                match result {
                    Ok(capture) => {
                        if copy {
                            cx.write_to_clipboard(ClipboardItem::new_image(
                                &gpui::Image::from_bytes(gpui::ImageFormat::Png, capture.png),
                            ));
                        }
                        launch.path = Some(capture.path);
                        if show_preview
                            && let Err(error) = crate::open_view("thumbnail", launch, cx)
                        {
                            eprintln!("Capture saved, but preview failed: {error:#}");
                        }
                    }
                    Err(e) => {
                        let message = format!("Capture failed: {e:#}");
                        if let Ok(handle) = cx.open_window(WindowOptions::default(), |_, cx| {
                            cx.new(|_| Notice { title: message })
                        }) {
                            let _ = crate::present_window(handle.into(), cx);
                        }
                    }
                }
                // Keep clipboard ownership while no-preview mode has no other
                // windows. The app integration layer supplies the tray lifetime.
                if cx.windows().len() > 1 {
                    let _ = selector.update(cx, |_, window, _| window.remove_window());
                }
            }) {
                eprintln!("Could not show capture result: {error}");
            }
        })
        .detach();
    }
    fn begin_recording(&mut self, token: u64, cx: &mut Context<Self>) {
        if self.launch.mock {
            return;
        }
        if cx.has_global::<RecoveryInProgress>() {
            self.lifecycle.cancel();
            self.status = "Finish recording recovery first.".into();
            cx.notify();
            return;
        }
        if self.segment.is_some() || !self.lifecycle.begin_start(token) {
            return;
        }
        let result = (|| -> anyhow::Result<NativeSegment> {
            if !captures_session::capture_session_available() {
                anyhow::bail!("desktop session is unavailable")
            };
            let display = self.chosen_display().context("no capture target")?.clone();
            let kind = if self.gif {
                RecordingKind::Gif
            } else {
                RecordingKind::Video
            };
            let mut options = self
                .settings
                .options(kind, self.target().context("no capture target")?);
            options.audio.microphone_muted = self.microphone_muted;
            if self.journal.is_none() {
                self.journal = Some(recovery::Journal::create(
                    &self.launch.profile,
                    options.clone(),
                )?);
            }
            let path = self
                .journal
                .as_mut()
                .context("no recording journal")?
                .begin_segment(&options)?;
            start_segment(
                &options,
                &path,
                &display,
                !self.preferences.include_recording_controls_in_captures,
            )
        })();
        match result {
            Ok(segment) => {
                self.segment = Some(segment);
                self.lifecycle.started(token);
                self.mode = ActionMode::Recording;
                self.paused = false;
                self.clock.start(Instant::now());
                self.status = "Recording".into();
                self.show_region_indicator(cx);
            }
            Err(e) => {
                self.lifecycle.cancel();
                if let Some(journal) = &mut self.journal {
                    journal.fail(&e);
                }
                self.status = format!("Recording failed: {e:#}")
            }
        }
        if self.segment.is_some() {
            self.watch_session(cx);
        }
        cx.notify()
    }
    fn schedule_recording_start(
        &mut self,
        token: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.launch.mock {
            self.lifecycle.cancel();
            return;
        }
        if !self.in_hud {
            let controller = cx.entity();
            let selector = window.window_handle();
            self.in_hud = true;
            let bounds = self.chosen_display().map_or_else(
                || Bounds::centered(None, size(px(430.), px(102.)), cx),
                |display| {
                    let (x, y) = display.overlay_position();
                    let scale = display.scale_factor.max(1.) as f32;
                    hud_bounds(
                        x as f32,
                        y as f32,
                        display.width as f32 / scale,
                        display.height as f32 / scale,
                    )
                },
            );
            // Opening a window immediately renders its root. Defer the whole
            // operation so GPUI does not reborrow this controller mid-update.
            cx.defer(move |cx| {
                match cx.open_window(
                    WindowOptions {
                        app_id: Some("captures-gpui-recording-hud".into()),
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: None,
                        window_decorations: Some(WindowDecorations::Client),
                        window_background: WindowBackgroundAppearance::Transparent,
                        kind: if cfg!(target_os = "linux") {
                            WindowKind::Normal
                        } else {
                            WindowKind::PopUp
                        },
                        is_resizable: false,
                        ..Default::default()
                    },
                    |window, cx| {
                        // Closing floating controls hides them; the recording
                        // remains reachable through its tray/shortcut action.
                        let close_controller = controller.clone();
                        window.on_window_should_close(cx, move |window, cx| {
                            let handle = window.window_handle();
                            let controller = close_controller.clone();
                            cx.spawn(async move |cx| {
                                Timer::after(Duration::from_millis(16)).await;
                                let _ = cx.update(|cx| {
                                    let _ =
                                        handle.update(cx, |_, window, _| window.minimize_window());
                                    let selector = controller.read(cx);
                                    let launch = selector.launch.clone();
                                    let shortcut =
                                        selector.preferences.new_capture_shortcut.clone();
                                    if let Err(error) = crate::notices::show_controls_hidden(
                                        crate::notices::shortcut_tokens(&shortcut),
                                        if cfg!(target_os = "macos") {
                                            "menu bar"
                                        } else {
                                            "system tray"
                                        },
                                        launch,
                                        cx,
                                    ) {
                                        eprintln!(
                                            "Could not show controls-hidden notice: {error:#}"
                                        );
                                    }
                                });
                            })
                            .detach();
                            false
                        });
                        controller.update(cx, |s, _| {
                            s.controls_excluded =
                                match crate::integration::set_window_capture_excluded(
                                    window,
                                    !s.preferences.include_recording_controls_in_captures,
                                ) {
                                    Ok(excluded) => excluded,
                                    Err(error) => {
                                        s.status = format!("Could not exclude controls: {error}");
                                        false
                                    }
                                };
                        });
                        controller.clone()
                    },
                ) {
                    Ok(handle) => {
                        #[cfg(target_os = "linux")]
                        if let Err(error) = crate::integration::configure_x11_floating(
                            "captures-gpui-recording-hud",
                            bounds,
                        ) {
                            eprintln!("Could not configure recording controls: {error:#}");
                        }
                        #[cfg(target_os = "linux")]
                        if std::env::var_os("WAYLAND_DISPLAY").is_none()
                            && let Err(error) = crate::integration::keep_x11_window_above(
                                "captures-gpui-recording-hud",
                            )
                        {
                            eprintln!(
                                "Could not keep recording controls above the guide: {error:#}"
                            );
                        }
                        controller.update(cx, |s, _| {
                            s.hud = Some(handle);
                            s.frozen = None;
                            s.backdrop = None;
                        });
                        cx.set_global(RecordingRegistry {
                            controller: controller.clone(),
                            window: handle,
                        });
                        let _ = selector.update(cx, |_, window, _| window.remove_window());
                        let _ = crate::present_window(handle.into(), cx);
                    }
                    Err(error) => {
                        controller.update(cx, |s, cx| {
                            s.in_hud = false;
                            s.lifecycle.cancel();
                            s.status = format!("Cannot open recording controls: {error}");
                            cx.notify();
                        });
                    }
                }
            });
        }
        self.hidden = false;
        self.status = "Starting…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let _ = this.update(cx, |s, cx| s.begin_recording(token, cx));
        })
        .detach();
    }
    fn record(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.launch.mock || self.busy {
            return;
        }
        if self.target().is_none() {
            self.status = match self.target {
                TargetMode::Region => "Drag a region before recording",
                TargetMode::Window => "Point at and click a window before recording",
                TargetMode::Display => "No display selected",
            }
            .into();
            cx.notify();
            return;
        }
        let Some(token) = self.lifecycle.begin_countdown() else {
            return;
        };
        let seconds = self.settings.countdown_seconds;
        if seconds == 0 {
            self.schedule_recording_start(token, window, cx);
            return;
        }
        self.countdown = Some(seconds);
        self.countdown_started = Some(Instant::now());
        self.countdown_exiting = false;
        self.frozen = None;
        self.backdrop = None;
        self.status = format!("Recording in {seconds}");
        cx.spawn_in(window, async move |this, cx| {
            for remaining in (1..=seconds).rev() {
                let timer = cx.background_executor().timer(Duration::from_secs(1));
                timer.await;
                let keep_going = this
                    .update(cx, |s, cx| {
                        if s.countdown.is_none() || !s.lifecycle.current(token) {
                            return false;
                        }
                        if !captures_session::capture_session_available() {
                            s.countdown = None;
                            s.lifecycle.cancel();
                            s.status =
                                "Recording cancelled: desktop session locked or inactive".into();
                            cx.notify();
                            return false;
                        }
                        s.countdown = Some(remaining.saturating_sub(1).max(1));
                        s.countdown_exiting = remaining == 1;
                        if s.countdown_exiting {
                            s.countdown_started = Some(Instant::now());
                        }
                        s.status = if remaining > 1 {
                            format!("Recording in {}", remaining - 1)
                        } else {
                            "Starting…".into()
                        };
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
            Timer::after(Duration::from_millis(if theme::reduced_motion() {
                0
            } else {
                140
            }))
            .await;
            let _ = this.update_in(cx, |s, window, cx| {
                if !s.lifecycle.current(token) {
                    return;
                }
                s.countdown = None;
                s.schedule_recording_start(token, window, cx)
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_segment(
        &mut self,
        then_resume: bool,
        resume_immediately: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.lifecycle.begin_finalize() {
            return;
        }
        if !then_resume {
            self.close_region_indicator(cx);
        }
        let Some(segment) = self.segment.take() else {
            self.lifecycle.finalized();
            return;
        };
        if let Some(journal) = &mut self.journal
            && let Err(error) = journal.state(RecordingState::Finalizing)
        {
            // Still stop the native writer; never leave it running because a
            // journal write failed. The pending entry remains recoverable.
            eprintln!("Could not journal recording stop: {error:#}");
        }
        self.busy = true;
        self.clock.pause(Instant::now());
        self.status = if then_resume {
            "Pausing…"
        } else {
            "Finalizing…"
        }
        .into();
        let task = cx
            .background_executor()
            .spawn(async move { stop_segment(segment) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                s.lifecycle.finalized();
                match result {
                    Ok(info) => {
                        let persisted =
                            s.journal.as_mut().context("no recording journal").and_then(
                                |journal| {
                                    journal.complete_segment(&info)?;
                                    journal.state(if then_resume {
                                        RecordingState::Paused
                                    } else {
                                        RecordingState::Finalizing
                                    })
                                },
                            );
                        s.completed.push(info);
                        if let Err(error) = persisted {
                            if let Some(journal) = &mut s.journal {
                                journal.fail(&error);
                            }
                            s.paused = true;
                            s.status =
                                format!("Media retained, but could not journal it: {error:#}");
                            cx.notify();
                            return;
                        }
                        if then_resume {
                            s.paused = true;
                            s.status = "Paused".into();
                            if resume_immediately && let Some(token) = s.lifecycle.begin_countdown()
                            {
                                s.begin_recording(token, cx);
                            }
                        } else {
                            s.assemble(cx);
                        }
                    }
                    Err(e) => {
                        if let Some(journal) = &mut s.journal {
                            journal.fail(&e);
                        }
                        s.status = format!("Finalizing failed: {e:#}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify()
    }
    fn assemble(&mut self, cx: &mut Context<Self>) {
        let Some(journal) = &self.journal else {
            return;
        };
        let id = journal.manifest.session_id.clone();
        let profile = self.launch.profile.clone();
        self.busy = true;
        let task = cx
            .background_executor()
            .spawn(async move { recovery::recover(&profile, &id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(path) => {
                        s.journal = None;
                        let mut l = s.launch.clone();
                        l.path = Some(path.clone());
                        s.status = format!("Recording ready: {}", path.display());
                        let result = if s.preferences.recording.open_editor_after_recording {
                            crate::open_view("recording-editor", l, cx)
                        } else if s.preferences.show_mini_previews {
                            crate::open_view("thumbnail", l, cx)
                        } else {
                            show_ready_notice(l, path, false, cx)
                        };
                        match result {
                            Ok(()) => {
                                s.completed.clear();
                                if let Some(hud) = s.hud.take() {
                                    cx.defer(move |cx| {
                                        if cx.has_global::<RecordingRegistry>() {
                                            cx.remove_global::<RecordingRegistry>();
                                        }
                                        let _ =
                                            hud.update(cx, |_, window, _| window.remove_window());
                                    });
                                }
                            }
                            Err(error) => {
                                s.status = format!(
                                    "Saved to history, but could not show recording: {error:#}"
                                )
                            }
                        }
                    }
                    Err(e) => s.status = format!("Assembly failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn stop(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.segment.is_some() {
            self.finish_segment(false, false, cx)
        } else if self.paused {
            self.paused = false;
            self.assemble(cx)
        }
    }
    fn pause(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.finish_segment(true, false, cx)
    }
    fn resume(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.paused {
            return;
        }
        if !captures_session::capture_session_available() {
            self.status = "Cannot resume while session is locked".into();
            cx.notify();
            return;
        }
        let Some(token) = self.lifecycle.begin_countdown() else {
            return;
        };
        self.schedule_recording_start(token, window, cx)
    }

    fn request_restart(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.confirmation = Some(HudConfirmation::Restart);
        cx.notify();
    }
    fn request_delete(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.confirmation = Some(HudConfirmation::Delete);
        cx.notify();
    }
    fn cancel_confirmation(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.confirmation = None;
        cx.notify();
    }
    fn remove_completed_drafts(&mut self) {
        for info in self.completed.drain(..) {
            for path in [
                Some(info.path),
                info.system_audio_path,
                info.microphone_path,
            ]
            .into_iter()
            .flatten()
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    fn confirm_destructive(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(action) = self.confirmation.take() else {
            return;
        };
        self.lifecycle.cancel();
        self.close_region_indicator(cx);
        if let Some(segment) = self.segment.take()
            && let Err(error) = segment.discard()
        {
            self.status = format!("Could not discard draft: {error}");
            cx.notify();
            return;
        }
        self.remove_completed_drafts();
        if let Some(journal) = &self.journal
            && let Err(error) = journal.discard()
        {
            self.status = format!("Could not discard recording journal: {error:#}");
            cx.notify();
            return;
        }
        self.journal = None;
        self.paused = false;
        self.clock.reset(Instant::now());
        self.clock.pause(Instant::now());
        match action {
            HudConfirmation::Restart => {
                self.status = "Restarting…".into();
                let Some(token) = self.lifecycle.begin_countdown() else {
                    return;
                };
                let controller = cx.entity();
                let seconds = self.settings.countdown_seconds;
                let display = self.chosen_display().cloned();
                if seconds == 0 {
                    self.schedule_recording_start(token, window, cx);
                    return;
                }
                if let Some(display) = display {
                    let (x, y) = display.overlay_position();
                    let bounds = Bounds::new(
                        point(px(x as f32), px(y as f32)),
                        size(
                            px(display.width as f32 / display.scale_factor.max(1.) as f32),
                            px(display.height as f32 / display.scale_factor.max(1.) as f32),
                        ),
                    );
                    cx.defer(move |cx| {
                        let callback_controller = controller.clone();
                        if let Err(error) =
                            countdown::open(bounds, seconds, cx, move |complete, cx| {
                                callback_controller.update(cx, |s, cx| {
                                    if !s.lifecycle.current(token) {
                                        return;
                                    }
                                    if complete {
                                        s.countdown = None;
                                        s.begin_recording(token, cx);
                                    } else {
                                        s.lifecycle.cancel();
                                        s.status = "Restart cancelled".into();
                                        s.countdown = None;
                                        let hud = s.hud.take();
                                        let owner = cx.entity_id();
                                        cx.defer(move |cx| {
                                            if let Some(hud) = hud {
                                                let _ = hud.update(cx, |_, w, _| w.remove_window());
                                            }
                                            if cx
                                                .try_global::<RecordingRegistry>()
                                                .is_some_and(|r| r.controller.entity_id() == owner)
                                            {
                                                cx.remove_global::<RecordingRegistry>();
                                            }
                                        });
                                    }
                                });
                            })
                        {
                            controller.update(cx, |s, cx| {
                                s.lifecycle.cancel();
                                s.status = format!("Cannot show restart countdown: {error:#}");
                                cx.notify();
                            });
                        }
                    });
                }
            }
            HudConfirmation::Delete => {
                self.status = "Draft deleted".into();
                // The registry intentionally keeps the controller alive while
                // recording, but no ownership is needed after confirmed delete.
                window.remove_window();
                cx.defer(|cx| {
                    if cx.has_global::<RecordingRegistry>() {
                        cx.remove_global::<RecordingRegistry>();
                    }
                });
            }
        }
    }
    fn toggle_microphone(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings.microphone_device_id.is_none() || self.busy {
            return;
        }
        self.microphone_muted = !self.microphone_muted;
        if self.segment.is_some() {
            self.finish_segment(true, true, cx);
        } else {
            cx.notify();
        }
    }
    fn screenshot_while_recording(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let launch = self.launch.clone();
        cx.defer(move |cx| {
            if let Err(error) = open_target(launch, "screenshot", "region", cx) {
                crate::integration::show_native_error(error, cx);
            }
        });
    }
    fn hide_hud(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Minimize rather than destroy: tray/menu capture actions restore this
        // exact controller through RecordingRegistry.
        window.minimize_window();
        let launch = self.launch.clone();
        let shortcut = self.preferences.new_capture_shortcut.clone();
        cx.defer(move |cx| {
            if let Err(error) = crate::notices::show_controls_hidden(
                crate::notices::shortcut_tokens(&shortcut),
                if cfg!(target_os = "macos") {
                    "menu bar"
                } else {
                    "system tray"
                },
                launch,
                cx,
            ) {
                eprintln!("Could not show controls-hidden notice: {error:#}");
            }
        });
    }

    fn watch_session(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let keep_watching = this
                    .update(cx, |s, cx| {
                        if s.segment.is_none() {
                            return false;
                        }
                        if !captures_session::capture_session_available() {
                            s.status =
                                "Recording stopped: desktop session locked or inactive".into();
                            s.finish_segment(false, false, cx);
                            return false;
                        }
                        if let Some(segment) = s.segment.as_ref()
                            && let Some(warning) = segment.warning()
                        {
                            s.status = warning;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_watching {
                    break;
                }
            }
        })
        .detach();
    }

    fn show_region_indicator(&mut self, cx: &mut Context<Self>) {
        if self.target != TargetMode::Region || self.indicator.is_some() {
            return;
        }
        let (Some(hole), Some(display)) = (self.selection, self.chosen_display().cloned()) else {
            return;
        };
        let controller = cx.entity();
        let scale = display.scale_factor.max(1.) as f32;
        let (x, y) = display.overlay_position();
        let bounds = Bounds::new(
            point(px(x as f32), px(y as f32)),
            size(
                px(display.width as f32 / scale),
                px(display.height as f32 / scale),
            ),
        );
        cx.defer(move |cx| {
            // Stop/pause can win before this deferred native window creation.
            if controller.read(cx).segment.is_none() || controller.read(cx).indicator.is_some() {
                return;
            }
            match indicator::open(hole, bounds, cx) {
                Ok(handle) => {
                    let hud = controller.update(cx, |s, _| {
                        s.indicator = Some(handle);
                        s.hud.filter(|_| !s.hidden)
                    });
                    if let Some(hud) = hud {
                        let _ = crate::present_window(hud.into(), cx);
                    }
                }
                Err(error) => eprintln!("Could not show passive recording region guide: {error:#}"),
            }
        });
    }

    fn close_region_indicator(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.indicator.take() {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.segment.is_some() || self.busy {
            return;
        }
        if let Some(focus) = &self.focus {
            focus.focus(window);
        }
        let point = (f32::from(event.position.x), f32::from(event.position.y));
        if self.target == TargetMode::Region {
            if let Some(rect) = self.selection.filter(|rect| rect.valid()) {
                let handle = rect
                    .handles()
                    .iter()
                    .position(|p| (point.0 - p.0).abs() <= 7. && (point.1 - p.1).abs() <= 7.)
                    .or_else(|| {
                        (point.0 >= rect.x
                            && point.0 <= rect.x + rect.width
                            && point.1 >= rect.y
                            && point.1 <= rect.y + rect.height)
                            .then_some(8)
                    });
                if let Some(handle) = handle {
                    self.selection_edit = Some((rect, handle, point));
                    return;
                }
            }
            self.drag_start = Some(point);
            self.selection = Some(Rect::from_drag(
                point,
                point,
                self.overlay_size(),
                event.modifiers.shift,
            ));
        } else if self.target == TargetMode::Window {
            self.selected_window = self.window_at(point).map(|w| w.id.clone());
        }
        cx.notify();
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let point = (f32::from(event.position.x), f32::from(event.position.y));
        if let Some((original, handle, start)) = self.selection_edit {
            self.selection = Some(original.adjusted(
                handle,
                (point.0 - start.0, point.1 - start.1),
                self.overlay_size(),
            ));
        } else if let Some(start) = self.drag_start {
            self.selection = Some(Rect::from_drag(
                start,
                point,
                self.overlay_size(),
                event.modifiers.shift,
            ));
        } else if self.target == TargetMode::Window {
            self.selected_window = self.window_at(point).map(|w| w.id.clone());
        }
        cx.notify();
    }
    fn mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag_start = None;
        let edited = self.selection_edit.take().is_some();
        if !edited && self.preferences.auto_start_on_selection && self.target().is_some() {
            if self.mode == ActionMode::Screenshot {
                self.capture(&ClickEvent::default(), window, cx);
            } else {
                self.record(&ClickEvent::default(), window, cx);
            }
        }
        cx.notify();
    }
    fn overlay_size(&self) -> (f32, f32) {
        self.chosen_display()
            .map(|d| {
                let (w, h) = d.overlay_size();
                (w as f32, h as f32)
            })
            .unwrap_or((1., 1.))
    }
    fn window_at(&self, point: (f32, f32)) -> Option<&WindowDescriptor> {
        let d = self.chosen_display()?;
        let scale = if DisplayDescriptor::reports_physical_geometry() {
            d.scale_factor.max(1.)
        } else {
            1.
        };
        self.windows
            .iter()
            .filter(|w| w.display_id == d.id)
            .filter(|w| {
                let x = (f64::from(w.x - d.x) / scale) as f32;
                let y = (f64::from(w.y - d.y) / scale) as f32;
                point.0 >= x
                    && point.1 >= y
                    && point.0 < x + w.width as f32 / scale as f32
                    && point.1 < y + w.height as f32 / scale as f32
            })
            .min_by_key(|w| w.z_order)
    }

    fn selection_rect(&self) -> Option<Rect> {
        let d = self.chosen_display()?;
        match self.target {
            TargetMode::Region => self.selection.filter(|r| r.valid()),
            TargetMode::Display => {
                let (width, height) = self.overlay_size();
                Some(Rect {
                    x: 0.,
                    y: 0.,
                    width,
                    height,
                })
            }
            TargetMode::Window => {
                let w = self
                    .windows
                    .iter()
                    .find(|w| Some(&w.id) == self.selected_window.as_ref())?;
                let scale = if DisplayDescriptor::reports_physical_geometry() {
                    d.scale_factor.max(1.) as f32
                } else {
                    1.
                };
                let (width, height) = self.overlay_size();
                Some(Rect::from_drag(
                    ((w.x - d.x) as f32 / scale, (w.y - d.y) as f32 / scale),
                    (
                        (w.x - d.x) as f32 / scale + w.width as f32 / scale,
                        (w.y - d.y) as f32 / scale + w.height as f32 / scale,
                    ),
                    (width, height),
                    false,
                ))
            }
        }
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                self.countdown = None;
                self.lifecycle.cancel();
                if self.segment.is_none() {
                    window.remove_window()
                }
            }
            "enter" if self.segment.is_none() => {
                if self.mode == ActionMode::Screenshot {
                    self.capture(&ClickEvent::default(), window, cx)
                } else {
                    self.record(&ClickEvent::default(), window, cx)
                }
            }
            "h" if self.segment.is_some() || self.paused => {
                self.hidden = !self.hidden;
                cx.notify();
            }
            _ => {}
        }
    }
    fn chip(&self, id: &'static str, label: impl IntoElement, t: Theme) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded_md()
            .bg(t.hover)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .child(label)
    }
}

impl Drop for Selector {
    fn drop(&mut self) {
        self.lifecycle.cancel();
        if let Some(segment) = self.segment.take() {
            // Native segment Drop is not the recording contract: explicitly
            // stop so writers and audio sidecars are finalized on window close.
            let result = stop_segment(segment).and_then(|info| {
                if let Some(journal) = &mut self.journal {
                    journal.complete_segment(&info)?;
                    journal.state(RecordingState::Paused)?;
                }
                Ok(())
            });
            if let Err(error) = result
                && let Some(journal) = &mut self.journal
            {
                journal.fail(&error);
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn start_segment(
    o: &RecordingOptions,
    p: &std::path::Path,
    d: &DisplayDescriptor,
    _: bool,
) -> anyhow::Result<NativeSegment> {
    Ok(NativeSegment::start(o, p, d)?)
}
#[cfg(target_os = "macos")]
fn start_segment(
    o: &RecordingOptions,
    p: &std::path::Path,
    _: &DisplayDescriptor,
    exclude_controls: bool,
) -> anyhow::Result<NativeSegment> {
    Ok(NativeSegment::start(o, p, exclude_controls)?)
}
fn stop_segment(s: NativeSegment) -> anyhow::Result<captures_recording::RecordingSegmentInfo> {
    Ok(s.stop()?)
}

impl Render for Selector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::configured(
            false,
            &self.preferences.theme,
            &self.preferences.custom_theme.accent,
            &self.preferences.custom_theme.signal,
        );
        let selection = self.selection_rect();
        let window_target = self.target == TargetMode::Window;
        let active = self.segment.is_some() || self.paused;
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if let Some(remaining) = self.countdown
            && !self.in_hud
        {
            let elapsed = self
                .countdown_started
                .map_or(0., |start| start.elapsed().as_secs_f32());
            let reduced_motion = theme::reduced_motion();
            let (opacity, scale) = countdown_pose(elapsed, self.countdown_exiting, reduced_motion);
            if !reduced_motion && elapsed < if self.countdown_exiting { 0.14 } else { 0.28 } {
                window.request_animation_frame();
            }
            let width = f32::from(window.viewport_size().width);
            return div()
                .size_full()
                .track_focus(&focus)
                .font_family(theme::font())
                .on_key_down(cx.listener(Self::key_down))
                .opacity(opacity)
                .bg(rgba(0x06070a80))
                .text_color(t.glass_text)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .text_color(t.glass_muted)
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(px((width * 0.016).clamp(14., 20.)))
                                .child(if self.mode == ActionMode::Screenshot {
                                    "SCREENSHOT IN"
                                } else {
                                    "RECORDING STARTS IN"
                                }),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_size(px((width * 0.26).clamp(150., 340.) * scale))
                                .line_height(relative(0.95))
                                .my(px(14.))
                                .child(remaining.to_string()),
                        )
                        .child(
                            div()
                                .mt(px(28.))
                                .text_size(px(14.))
                                .text_color(t.glass_muted)
                                .child("Press Esc to cancel"),
                        ),
                )
                .into_any_element();
        }
        if self.in_hud {
            let elapsed = self.clock.elapsed(Instant::now()).as_secs();
            let time = format!("{:02}:{:02}", elapsed / 60, elapsed % 60);
            let has_microphone = self.settings.microphone_device_id.is_some();
            let level = self
                .segment
                .as_ref()
                .map(NativeSegment::microphone_level)
                .unwrap_or(0.)
                .clamp(0., 1.);
            let privacy = if self.preferences.include_recording_controls_in_captures {
                "Controls are included in this recording"
            } else if self.controls_excluded || cfg!(target_os = "macos") {
                "Controls are hidden from this recording"
            } else {
                "Controls may appear in this recording"
            };
            let action = |id: &'static str, label: &'static str, t: Theme| {
                self.chip(id, hud_icon(id, self.microphone_muted, t), t)
                    .w(px(34.))
                    .h(px(34.))
                    .bg(gpui::transparent_black())
                    .hover(move |style| style.bg(t.hover))
                    .px_0()
                    .py_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(if self.busy { 0.32 } else { 1. })
                    .tooltip(move |_, cx| cx.new(|_| HudTooltip(label)).into())
            };
            return div()
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::key_down))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|s, _, window, _| {
                        if let Some(focus) = &s.focus {
                            focus.focus(window);
                        }
                        window.start_window_move();
                    }),
                )
                .size_full()
                .font_family(theme::font())
                .text_size(px(12.))
                .bg(rgba(0x00000000))
                .text_color(t.glass_text)
                .p(px(6.))
                .child(
                    div()
                        .w_full()
                        .h(px(70.))
                        .rounded(px(18.))
                        .border_1()
                        .border_color(t.glass_border)
                        .bg(t.glass)
                        .px(px(8.))
                        .pt(px(6.))
                        .pb(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(t.glass_muted)
                                .text_center()
                                .child(privacy),
                        )
                        .child(if let Some(confirm) = self.confirmation {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().child(match confirm {
                                    HudConfirmation::Restart => "Delete this draft and restart?",
                                    HudConfirmation::Delete => "Permanently delete this draft?",
                                }))
                                .child(
                                    self.chip("confirm-cancel", "Cancel", t)
                                        .on_click(cx.listener(Self::cancel_confirmation)),
                                )
                                .child(
                                    self.chip(
                                        "confirm-action",
                                        match confirm {
                                            HudConfirmation::Restart => "Restart",
                                            HudConfirmation::Delete => "Delete",
                                        },
                                        t,
                                    )
                                    .bg(t.signal)
                                    .on_click(cx.listener(Self::confirm_destructive)),
                                )
                        } else {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .w(px(96.))
                                        .flex_shrink_0()
                                        .child(
                                            div().size(px(10.)).rounded_full().bg(if self.paused {
                                                t.accent
                                            } else {
                                                t.signal
                                            }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_size(px(16.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(time),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(9.))
                                                        .text_color(t.glass_muted)
                                                        .child(if self.busy {
                                                            "SAVING…"
                                                        } else if self.countdown.is_some() {
                                                            "STARTING…"
                                                        } else if self.paused {
                                                            "PAUSED"
                                                        } else if self.segment.is_some()
                                                            || self.launch.mock
                                                        {
                                                            "RECORDING"
                                                        } else {
                                                            "STOPPED"
                                                        }),
                                                ),
                                        ),
                                )
                                .child(
                                    action("stop-hud", "Stop and save", t)
                                        .bg(t.signal)
                                        .on_click(cx.listener(Self::stop)),
                                )
                                .child(if self.paused {
                                    action("resume-hud", "Resume recording", t)
                                        .on_click(cx.listener(Self::resume))
                                } else {
                                    action("pause-hud", "Pause recording", t)
                                        .on_click(cx.listener(Self::pause))
                                })
                                .child(
                                    action("restart-hud", "Restart recording", t)
                                        .on_click(cx.listener(Self::request_restart)),
                                )
                                .child(
                                    action("screenshot-hud", "Take a region screenshot", t)
                                        .on_click(cx.listener(Self::screenshot_while_recording)),
                                )
                                .when(has_microphone, |row| {
                                    row.child(
                                        div()
                                            .w(px(32.))
                                            .h(px(4.))
                                            .rounded_full()
                                            .bg(rgba(0x00000066))
                                            .child(
                                                div()
                                                    .h_full()
                                                    .w(relative(level))
                                                    .rounded_full()
                                                    .bg(t.positive),
                                            ),
                                    )
                                })
                                .child(
                                    action(
                                        "microphone-hud",
                                        if self.microphone_muted {
                                            "Unmute microphone"
                                        } else {
                                            "Mute microphone"
                                        },
                                        t,
                                    )
                                    .opacity(if has_microphone { 1. } else { 0.3 })
                                    .on_click(cx.listener(Self::toggle_microphone)),
                                )
                                .child(
                                    action("delete-hud", "Delete recording", t)
                                        .on_click(cx.listener(Self::request_delete)),
                                )
                                .child(
                                    action("hide-hud", "Hide controls", t)
                                        .on_click(cx.listener(Self::hide_hud)),
                                )
                        }),
                )
                .into_any_element();
        }
        div()
            .track_focus(&focus)
            .relative()
            .font_family(theme::font())
            .text_size(px(13.))
            .size_full()
            .bg(rgba(0x00000000))
            .text_color(t.glass_text)
            .p_5()
            .flex()
            .flex_col()
            .justify_between()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_key_down(cx.listener(Self::key_down))
            .when(!self.hidden, |root| {
                root.when_some(self.backdrop.clone(), |root, image| {
                    root.child(div().absolute().inset_0().child(img(image).size_full()))
                })
            })
            .when(!self.hidden, |root| root.child(canvas(|_,_,_| {}, move |bounds,_,window,_| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                let rects = match selection {
                    Some(r) => vec![Rect{x:0.,y:0.,width:w,height:r.y}, Rect{x:0.,y:r.y+r.height,width:w,height:(h-r.y-r.height).max(0.)}, Rect{x:0.,y:r.y,width:r.x,height:r.height}, Rect{x:r.x+r.width,y:r.y,width:(w-r.x-r.width).max(0.),height:r.height}],
                    None => vec![Rect{x:0.,y:0.,width:w,height:h}],
                };
                for r in rects {
                    window.paint_quad(fill(Bounds::new(bounds.origin+point(px(r.x),px(r.y)),size(px(r.width),px(r.height))),rgba(if window_target {0x0609106b} else {0x06060a33})));
                }
                if let Some(r) = selection {
                    window.paint_quad(outline(Bounds::new(bounds.origin+point(px(r.x),px(r.y)),size(px(r.width),px(r.height))),t.accent,BorderStyle::Solid));
                    if !window_target {
                        for (x,y) in r.handles() {
                            window.paint_quad(fill(Bounds::new(bounds.origin+point(px(x-4.),px(y-4.)),size(px(8.),px(8.))),t.accent));
                        }
                    }
                }
            }).absolute().inset_0().size_full()))
            .when(!self.hidden, |root| {
                root.child(
                    div().absolute().bottom(px(26.)).left(px(20.)).right(px(20.)).flex().justify_center().child(
                    div().bg(t.glass).border_1().border_color(t.glass_border).rounded(px(18.)).p_4().flex().flex_col().gap_3().child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            self.chip("screenshot", "Screenshot", t)
                                .on_click(cx.listener(|s, _, _, cx| {
                                    s.mode = ActionMode::Screenshot;
                                    cx.notify()
                                })),
                        )
                        .child(self.chip("recording", "Video", t).on_click(cx.listener(
                            |s, _, _, cx| {
                                s.mode = ActionMode::Recording;
                                s.gif = false;
                                cx.notify()
                            },
                        )))
                        .child(self.chip("gif", "GIF", t).on_click(cx.listener(|s,_,_,cx| {
                            s.mode = ActionMode::Recording;
                            s.gif = true;
                            cx.notify();
                        })))
                        .child(div().w(px(1.)).h(px(24.)).my_auto().bg(t.glass_border))
                        .child(self.chip("region", "Region", t).on_click(cx.listener(
                            |s, _, _, cx| {
                                s.target = TargetMode::Region;
                                cx.notify()
                            },
                        )))
                        .child(self.chip("window", "Window", t).on_click(cx.listener(
                            |s, _, _, cx| {
                                s.target = TargetMode::Window;
                                s.selected_window = None;
                                cx.notify()
                            },
                        )))
                        .child(self.chip("display", "Display", t).on_click(cx.listener(
                            |s, _, _, cx| {
                                s.target = TargetMode::Display;
                                cx.notify()
                            },
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(self.status.clone())
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    self.chip(
                                        "cursor",
                                        if if self.mode == ActionMode::Screenshot {
                                            self.preferences.show_cursor_in_screenshots
                                        } else {
                                            self.settings.show_cursor
                                        } {
                                            "Cursor on"
                                        } else {
                                            "Cursor off"
                                        },
                                        t,
                                    )
                                    .on_click(cx.listener(
                                        |s, _, _, cx| {
                                            if s.mode == ActionMode::Screenshot {
                                                s.preferences.show_cursor_in_screenshots =
                                                    !s.preferences.show_cursor_in_screenshots;
                                            } else {
                                                s.settings.show_cursor = !s.settings.show_cursor;
                                            }
                                            cx.notify()
                                        },
                                    )),
                                )
                                .when(self.mode == ActionMode::Recording, |row| {
                                    row.child(
                                        self.chip(
                                            "audio",
                                            if self.settings.capture_system_audio {
                                                "Audio on"
                                            } else {
                                                "Audio off"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.settings.capture_system_audio =
                                                    !s.settings.capture_system_audio;
                                                cx.notify()
                                            }),
                                        ),
                                    )
                                })
                                .child(
                                    self.chip(
                                        "countdown",
                                        format!(
                                            "{}s",
                                            if self.mode == ActionMode::Screenshot {
                                                self.preferences.screenshot_countdown_seconds
                                            } else {
                                                self.settings.countdown_seconds
                                            }
                                        ),
                                        t,
                                    )
                                    .on_click(cx.listener(
                                        |s, _, _, cx| {
                                            let seconds = if s.mode == ActionMode::Screenshot {
                                                &mut s.preferences.screenshot_countdown_seconds
                                            } else {
                                                &mut s.settings.countdown_seconds
                                            };
                                            *seconds = match *seconds {
                                                0 => 3,
                                                3 => 5,
                                                _ => 0,
                                            };
                                            cx.notify()
                                        },
                                    )),
                                )
                                .when(!active, |d| {
                                    d.child(if self.mode == ActionMode::Screenshot {
                                        self.chip("capture", "Capture", t)
                                            .bg(t.accent)
                                            .text_color(gpui::black())
                                            .on_click(cx.listener(Self::capture))
                                    } else {
                                        self.chip("start", "Start recording", t)
                                            .bg(t.signal)
                                            .on_click(cx.listener(Self::record))
                                    })
                                })
                                .child(self.chip("cancel", "✕", t).on_click(cx.listener(|s,_,window,_| {
                                    s.lifecycle.cancel();
                                    window.remove_window();
                                })))
                                .when(active, |d| {
                                    d.child(if self.paused {
                                        self.chip("resume", "Resume", t)
                                            .on_click(cx.listener(Self::resume))
                                    } else {
                                        self.chip("pause", "Pause", t)
                                            .on_click(cx.listener(Self::pause))
                                    })
                                    .child(self.chip("hide", "Hide (restore: H)", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            s.hidden = true;
                                            cx.notify()
                                        }),
                                    ))
                                    .child(
                                        self.chip("stop", "Stop", t)
                                            .bg(t.signal)
                                            .on_click(cx.listener(Self::stop)),
                                    )
                                }),
                        ),
                )))
                .child(div().absolute().top(relative(0.16)).left_0().w_full().flex().justify_center().child(
                    div().px_6().py_4().rounded(px(16.)).bg(t.glass).border_1().border_color(t.glass_border).flex().flex_col().items_center().gap_1()
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(if self.countdown.is_some() {64.} else {16.})).child(if let Some(n)=self.countdown {format!("{n}")} else if let Some(r)=selection {format!("{} × {}",r.width.round(),r.height.round())} else {"Select an area to capture".into()}))
                        .child(div().text_size(px(12.)).text_color(t.glass_muted).child(if self.countdown.is_some() {"Escape to cancel"} else {"Drag to select · Shift for square · Enter to capture · Escape to cancel"}))
                ))
            })
            .into_any_element()
    }
}

struct Notice {
    title: String,
}
impl Render for Notice {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::new(false);
        div()
            .font_family(theme::font())
            .size_full()
            .bg(t.glass)
            .text_color(t.glass_text)
            .items_center()
            .justify_center()
            .text_size(px(42.))
            .child(self.title.clone())
    }
}

pub fn open(launch: Launch, cx: &mut App) -> anyhow::Result<()> {
    match launch.view.as_str() {
        "recording-editor" => {
            let editor = editor::RecordingEditor::new(launch)?;
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Captures Recording editor".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| editor),
            )?;
        }
        "recording-selector" | "overlay" => {
            let selector = Selector::new(launch)?;
            show_selector(selector, cx)?;
        }
        "recording-hud" => {
            if launch.mock {
                let state = launch
                    .path
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .unwrap_or("default")
                    .to_owned();
                let mut selector = Selector::new(launch)?;
                selector.in_hud = true;
                selector.paused = state.contains("paused");
                selector.confirmation = state
                    .contains("restart")
                    .then_some(HudConfirmation::Restart)
                    .or_else(|| state.contains("delete").then_some(HudConfirmation::Delete));
                selector.settings.microphone_device_id = Some("mock microphone".into());
                selector.clock.recorded = Duration::from_secs(83);
                if !selector.paused {
                    selector.clock.running_since = Some(Instant::now());
                }
                let bounds = Bounds::centered(None, size(px(620.), px(82.)), cx);
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Captures Recording controls fixture".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| selector),
                )?;
            } else if !restore_hud(cx) {
                cx.open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| Notice {
                        title: "Recording HUD requires an active recording".into(),
                    })
                })?;
            }
        }
        "recording-countdown" | "screenshot-countdown" => {
            anyhow::ensure!(
                launch.mock,
                "countdown is driven by an active capture; use --mock for a fixture"
            );
            let recording = launch.view == "recording-countdown";
            let mut selector = Selector::new(launch)?;
            selector.mode = if recording {
                ActionMode::Recording
            } else {
                ActionMode::Screenshot
            };
            selector.countdown = Some(3);
            selector.countdown_started = Some(Instant::now());
            show_selector(selector, cx)?;
        }
        "recording-region-indicator" => {
            anyhow::ensure!(
                launch.mock,
                "region indicator is owned by an active recording"
            );
            let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
            indicator::open(
                Rect {
                    x: 173.,
                    y: 91.,
                    width: 541.,
                    height: 337.,
                },
                bounds,
                cx,
            )?;
        }
        other => anyhow::bail!("unsupported recording surface: {other}"),
    }
    Ok(())
}

fn show_selector(mut selector: Selector, cx: &mut App) -> anyhow::Result<WindowHandle<Selector>> {
    let (width, height) = selector.overlay_size();
    let (x, y) = selector
        .chosen_display()
        .map(DisplayDescriptor::overlay_position)
        .unwrap_or((0., 0.));
    let bounds = Bounds::new(
        point(px(x as f32), px(y as f32)),
        size(px(width), px(height)),
    );
    let class = format!("captures-gpui-selector-{}", uuid::Uuid::new_v4());
    let handle = cx.open_window(
        WindowOptions {
            app_id: Some(class.clone()),
            window_bounds: Some(WindowBounds::Fullscreen(bounds)),
            titlebar: None,
            kind: WindowKind::Normal,
            window_decorations: Some(WindowDecorations::Client),
            is_movable: false,
            is_resizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            selector.focus = Some(focus);
            cx.new(|_| selector)
        },
    )?;
    #[cfg(target_os = "linux")]
    crate::integration::configure_x11_floating(&class, bounds)?;
    Ok(handle)
}

/// Entry point shared by tray/menu/global shortcuts. Display shortcuts commit
/// directly; region and window shortcuts open their matching selectors.
pub fn open_target(launch: Launch, action: &str, target: &str, cx: &mut App) -> anyhow::Result<()> {
    if matches!(action, "video" | "gif") && restore_hud(cx) {
        return Ok(());
    }
    let mut selector = Selector::new(launch)?;
    (selector.mode, selector.gif) = match action {
        "screenshot" => (ActionMode::Screenshot, false),
        "video" => (ActionMode::Recording, false),
        "gif" => (ActionMode::Recording, true),
        _ => anyhow::bail!("unknown capture action: {action}"),
    };
    selector.target = match target {
        "region" => TargetMode::Region,
        "window" => TargetMode::Window,
        "display" => TargetMode::Display,
        _ => anyhow::bail!("unknown capture target: {target}"),
    };
    let commit = selector.target == TargetMode::Display;
    let handle = show_selector(selector, cx)?;
    crate::present_window(handle.into(), cx)?;
    if commit {
        cx.defer(move |cx| {
            let _ = handle.update(cx, |selector, window, cx| {
                if selector.mode == ActionMode::Screenshot {
                    selector.capture(&ClickEvent::default(), window, cx);
                } else {
                    selector.record(&ClickEvent::default(), window, cx);
                }
            });
        });
    }
    Ok(())
}

/// Restore the one active recording HUD, if any. The recording registry owns
/// the controller strongly so hiding its last window cannot end a recording.
pub fn restore_hud(cx: &mut App) -> bool {
    let Some((controller, window)) = cx
        .try_global::<RecordingRegistry>()
        .map(|registry| (registry.controller.clone(), registry.window))
    else {
        return false;
    };
    let _ = controller.read(cx);
    crate::present_window(window.into(), cx).is_ok()
}

fn hud_bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    // Match the shipping logical bounds and bottom margin on the selected display.
    Bounds::new(
        point(px(x + (width - 430.) / 2.), px(y + height - 102. - 20.)),
        size(px(430.), px(102.)),
    )
}

fn countdown_pose(elapsed: f32, exiting: bool, reduced: bool) -> (f32, f32) {
    if reduced {
        return (1., 1.);
    }
    if exiting {
        let progress = (elapsed / 0.14).clamp(0., 1.);
        (1. - progress, 1. + 0.035 * progress)
    } else {
        (
            theme::ease_out(elapsed / 0.2),
            0.94 + 0.06 * theme::ease_out(elapsed / 0.28),
        )
    }
}

fn show_ready_notice(
    launch: Launch,
    path: PathBuf,
    permanently_saved: bool,
    cx: &mut App,
) -> anyhow::Result<()> {
    use crate::{notices::RecordingReady, previews::media};
    let profile = launch.profile.clone();
    let save_executor = cx.background_executor().clone();
    let reveal_executor = save_executor.clone();
    crate::notices::show_recording_ready(
        RecordingReady {
            path,
            permanently_saved,
            save: Arc::new(move |source| {
                let profile = profile.clone();
                Box::pin(save_executor.spawn(async move {
                    let settings = crate::preferences::settings::load(&profile)?;
                    let media = media::PreviewMedia::load(source, &profile)?;
                    let path = media::save(&media, &settings)?;
                    if let Err(error) =
                        crate::preferences::history::link_saved(&profile, &media.source, &path)
                    {
                        eprintln!("Recording saved, but history link failed: {error}");
                    }
                    Ok(path)
                }))
            }),
            reveal: Arc::new(move |path| {
                Box::pin(reveal_executor.spawn(async move { media::reveal(&path) }))
            }),
        },
        launch,
        cx,
    )
}

#[cfg(test)]
mod recording_clock_tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn hud_uses_selected_display_origin_and_bottom_margin() {
        let bounds = hud_bounds(-1440., 85., 1440., 900.);
        assert_eq!(bounds.origin, point(px(-935.), px(863.)));
        assert_eq!(bounds.size, size(px(430.), px(102.)));
    }

    #[test]
    fn countdown_animation_uses_shared_durations_and_reduced_motion() {
        assert_eq!(countdown_pose(0., false, false), (0., 0.94));
        assert_eq!(countdown_pose(0.28, false, false), (1., 1.));
        assert_eq!(countdown_pose(0.14, true, false), (0., 1.035));
        assert_eq!(countdown_pose(0.1, true, true), (1., 1.));
        let (opacity, scale) = countdown_pose(0.07, true, false);
        assert!((opacity - 0.5).abs() < 0.00001 && (scale - 1.0175).abs() < 0.00001);
    }

    #[test]
    fn elapsed_excludes_each_independent_pause_boundary() {
        let base = Instant::now();
        let mut clock = RecordingClock::default();
        clock.start(base);
        clock.pause(base + Duration::from_millis(1250));
        assert_eq!(
            clock.elapsed(base + Duration::from_secs(9)),
            Duration::from_millis(1250)
        );
        clock.start(base + Duration::from_secs(10));
        clock.pause(base + Duration::from_millis(12_750));
        assert_eq!(
            clock.elapsed(base + Duration::from_secs(30)),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn reset_drops_prior_segments_and_starts_at_its_own_boundary() {
        let base = Instant::now();
        let mut clock = RecordingClock::default();
        clock.start(base);
        clock.pause(base + Duration::from_secs(7));
        clock.reset(base + Duration::from_secs(20));
        assert_eq!(
            clock.elapsed(base + Duration::from_millis(22_500)),
            Duration::from_millis(2500)
        );
    }
}
