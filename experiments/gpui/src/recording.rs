//! Native GPUI capture, recording, HUD, and recording-editor surfaces.
//!
//! Recording/export uses the repository backends. The editor requires `ffmpeg`
//! and `ffprobe` on PATH; failures are shown and are never replaced by fixtures.
mod editor;
mod model;

use crate::{
    Launch,
    theme::{self, Theme},
};
use anyhow::Context as _;
use captures_capture::{DisplayDescriptor, WindowDescriptor, XcapBackend};
use captures_media::{CancelToken, MediaToolchain};
use captures_recording::{RecordingKind, RecordingOptions, RecordingSegmentInfo, RecordingTarget};
use gpui::{prelude::*, *};
use model::{ActionMode, Lifecycle, Rect, Settings, TargetMode, timestamped};
use std::{path::PathBuf, time::Duration};

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
    displays: Vec<DisplayDescriptor>,
    windows: Vec<WindowDescriptor>,
    display: usize,
    segment: Option<NativeSegment>,
    output: Option<PathBuf>,
    completed: Vec<RecordingSegmentInfo>,
    selection: Option<Rect>,
    drag_start: Option<(f32, f32)>,
    selected_window: Option<String>,
    countdown: Option<u8>,
    busy: bool,
    paused: bool,
    hidden: bool,
    status: String,
    lifecycle: Lifecycle,
    in_hud: bool,
    focus: Option<FocusHandle>,
}
impl Selector {
    fn new(launch: Launch) -> Self {
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
        let windows = if launch.mock || displays.is_empty() {
            Vec::new()
        } else {
            XcapBackend.windows().unwrap_or_default()
        };
        Self {
            launch,
            mode: ActionMode::Screenshot,
            target: TargetMode::Region,
            gif: false,
            settings,
            displays,
            windows,
            display: 0,
            segment: None,
            output: None,
            completed: Vec::new(),
            selection: None,
            drag_start: None,
            selected_window: None,
            countdown: None,
            busy: false,
            paused: false,
            hidden: false,
            status,
            lifecycle: Lifecycle::default(),
            in_hud: false,
            focus: None,
        }
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
        if self.launch.mock {
            return;
        }
        if self.busy || self.countdown.is_some() || self.segment.is_some() {
            return;
        }
        let target = self.target();
        let profile = self.launch.profile.clone();
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
            (|| -> anyhow::Result<PathBuf> {
                if !captures_session::capture_session_available() {
                    anyhow::bail!("desktop session is unavailable")
                };
                let backend = XcapBackend;
                backend.ensure_permission(true)?;
                let image = match target {
                    Some(RecordingTarget::Window { window_id }) => {
                        backend.capture_window(&window_id)?
                    }
                    Some(RecordingTarget::Region { display_id, rect }) => {
                        let frame = backend.capture_display(&display_id)?;
                        let scale = frame
                            .descriptor
                            .overlay_to_buffer_scale(frame.image.width(), frame.image.height());
                        frame
                            .crop(captures_capture::PhysicalRect {
                                x: (f64::from(rect.x) * scale).round() as u32,
                                y: (f64::from(rect.y) * scale).round() as u32,
                                width: (f64::from(rect.width) * scale).round() as u32,
                                height: (f64::from(rect.height) * scale).round() as u32,
                            })
                            .context("selected region is outside display")?
                    }
                    Some(RecordingTarget::Display { display_id }) => {
                        backend.capture_display(&display_id)?.image
                    }
                    None => anyhow::bail!("no capture target"),
                };
                let dir = profile.join("captures");
                std::fs::create_dir_all(&dir)?;
                let path = timestamped(&dir, "png");
                image.save(&path)?;
                Ok(path)
            })()
        });
        cx.spawn(async move |_, cx| {
            let result = task.await;
            if let Err(error) = cx.update(|cx| {
                match result {
                    Ok(path) => {
                        launch.path = Some(path);
                        if let Err(error) = crate::open_view("screenshot-editor", launch, cx) {
                            eprintln!("Capture saved, but editor failed: {error:#}");
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
                let _ = selector.update(cx, |_, window, _| window.remove_window());
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
        if self.segment.is_some() || !self.lifecycle.begin_start(token) {
            return;
        }
        let result = (|| -> anyhow::Result<(NativeSegment, PathBuf)> {
            if !captures_session::capture_session_available() {
                anyhow::bail!("desktop session is unavailable")
            };
            let display = self.chosen_display().context("no capture target")?;
            let kind = if self.gif {
                RecordingKind::Gif
            } else {
                RecordingKind::Video
            };
            let options = self
                .settings
                .options(kind, self.target().context("no capture target")?);
            let drafts = self.launch.profile.join("recording-drafts");
            std::fs::create_dir_all(&drafts)?;
            let destination = self
                .output
                .clone()
                .unwrap_or_else(|| timestamped(&drafts, "mp4"));
            let path = destination.with_file_name(format!(
                "{}.segment-{:03}.mp4",
                destination
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
                self.completed.len() + 1
            ));
            Ok((start_segment(&options, &path, display)?, destination))
        })();
        match result {
            Ok((segment, path)) => {
                self.segment = Some(segment);
                self.lifecycle.started(token);
                self.output = Some(path);
                self.mode = ActionMode::Recording;
                self.paused = false;
                self.status = "Recording".into()
            }
            Err(e) => {
                self.lifecycle.cancel();
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
            let bounds = Bounds::centered(None, size(px(620.), px(110.)), cx);
            // Opening a window immediately renders its root. Defer the whole
            // operation so GPUI does not reborrow this controller mid-update.
            cx.defer(move |cx| {
                match cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Captures Recording controls".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_, _| controller.clone(),
                ) {
                    Ok(handle) => {
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
                        s.countdown = (remaining > 1).then_some(remaining - 1);
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
            let _ = this.update_in(cx, |s, window, cx| {
                s.schedule_recording_start(token, window, cx)
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_segment(&mut self, then_resume: bool, cx: &mut Context<Self>) {
        if !self.lifecycle.begin_finalize() {
            return;
        }
        let Some(segment) = self.segment.take() else {
            self.lifecycle.finalized();
            return;
        };
        self.busy = true;
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
                        s.completed.push(info);
                        if then_resume {
                            s.paused = true;
                            s.status = "Paused".into();
                        } else {
                            s.assemble(cx);
                        }
                    }
                    Err(e) => s.status = format!("Finalizing failed: {e:#}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify()
    }
    fn assemble(&mut self, cx: &mut Context<Self>) {
        let Some(destination) = self.output.clone() else {
            return;
        };
        let segments = self
            .completed
            .iter()
            .map(|s| s.path.clone())
            .collect::<Vec<_>>();
        let assembled = destination.with_file_name(format!(
            "assembled-{}",
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        self.busy = true;
        let task = cx.background_executor().spawn(async move {
            MediaToolchain::from_command_names()
                .concatenate_segments(&segments, &assembled, &CancelToken::default())
                .map(|_| assembled)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(path) => {
                        let mut l = s.launch.clone();
                        l.path = Some(path.clone());
                        s.status = format!("Recording ready: {}", path.display());
                        let _ = crate::open_view("recording-editor", l, cx);
                    }
                    Err(e) => s.status = format!("Assembly failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn stop(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.segment.is_some() {
            self.finish_segment(false, cx)
        } else if self.paused {
            self.paused = false;
            self.assemble(cx)
        }
    }
    fn pause(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_segment(true, cx)
    }
    fn resume(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
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
                            s.finish_segment(false, cx);
                            return false;
                        }
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
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.segment.is_some() || self.busy {
            return;
        }
        if let Some(focus) = &self.focus {
            focus.focus(window);
        }
        let point = (f32::from(event.position.x), f32::from(event.position.y));
        if self.target == TargetMode::Region {
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
        if let Some(start) = self.drag_start {
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
    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag_start = None;
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
            let _ = stop_segment(segment);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn start_segment(
    o: &RecordingOptions,
    p: &std::path::Path,
    d: &DisplayDescriptor,
) -> anyhow::Result<NativeSegment> {
    Ok(NativeSegment::start(o, p, d)?)
}
#[cfg(target_os = "macos")]
fn start_segment(
    o: &RecordingOptions,
    p: &std::path::Path,
    _: &DisplayDescriptor,
) -> anyhow::Result<NativeSegment> {
    Ok(NativeSegment::start(o, p, true)?)
}
fn stop_segment(s: NativeSegment) -> anyhow::Result<captures_recording::RecordingSegmentInfo> {
    Ok(s.stop()?)
}

impl Render for Selector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::new(false);
        let active = self.segment.is_some() || self.paused;
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if self.in_hud {
            return div().track_focus(&focus).on_key_down(cx.listener(Self::key_down))
                .on_mouse_down(MouseButton::Left, cx.listener(|s, _, window, _| {
                    if let Some(focus) = &s.focus { focus.focus(window); }
                }))
                .size_full().font_family(theme::font()).text_size(px(13.)).bg(t.glass).text_color(t.glass_text)
                .p_3().flex().flex_col().gap_2()
                .child(div().flex().items_center().gap_3()
                    .child(div().size(px(8.)).rounded_full().bg(if self.paused { t.glass_muted } else { t.signal }))
                    .child(div().flex_1().child(self.status.clone()))
                    .when(!self.busy, |d| d.child(if self.paused {
                        self.chip("resume-hud", "Resume", t).on_click(cx.listener(Self::resume))
                    } else {
                        self.chip("pause-hud", "Pause", t).on_click(cx.listener(Self::pause))
                    }).child(self.chip("stop-hud", "Stop", t).bg(t.signal).on_click(cx.listener(Self::stop)))))
                .child(div().text_size(px(11.)).text_color(t.glass_muted)
                    .child("Experimental controls may appear in captures. Closing stops the segment; no automatic resume."))
                .into_any_element();
        }
        div()
            .track_focus(&focus)
            .relative()
            .font_family(theme::font())
            .text_size(px(13.))
            .size_full()
            .bg(if self.hidden {
                rgba(0x00000000)
            } else {
                t.glass
            })
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
                root.child(
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
                        .child(self.chip("recording", "Record", t).on_click(cx.listener(
                            |s, _, _, cx| {
                                s.mode = ActionMode::Recording;
                                cx.notify()
                            },
                        )))
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
                .child(div().items_center().child(if let Some(n) = self.countdown {
                    format!("{n}")
                } else if let Some(r) = self.selection {
                    format!("{} × {}", r.width.round(), r.height.round())
                } else {
                    "Drag to select • hold Shift for 1:1 • Enter confirms • Escape cancels".into()
                }))
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
                                        if self.settings.show_cursor {
                                            "Cursor on"
                                        } else {
                                            "Cursor off"
                                        },
                                        t,
                                    )
                                    .on_click(cx.listener(
                                        |s, _, _, cx| {
                                            s.settings.show_cursor = !s.settings.show_cursor;
                                            cx.notify()
                                        },
                                    )),
                                )
                                .child(
                                    self.chip(
                                        "audio",
                                        if self.settings.capture_system_audio {
                                            "Audio on"
                                        } else {
                                            "Audio off"
                                        },
                                        t,
                                    )
                                    .on_click(cx.listener(
                                        |s, _, _, cx| {
                                            s.settings.capture_system_audio =
                                                !s.settings.capture_system_audio;
                                            cx.notify()
                                        },
                                    )),
                                )
                                .child(
                                    self.chip(
                                        "countdown",
                                        format!("{}s", self.settings.countdown_seconds),
                                        t,
                                    )
                                    .on_click(cx.listener(
                                        |s, _, _, cx| {
                                            s.settings.countdown_seconds =
                                                match s.settings.countdown_seconds {
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
                )
            })
            .when(!self.hidden && self.target == TargetMode::Region, |root| {
                root.when_some(self.selection.filter(|rect| rect.valid()), |root, rect| {
                    root.child(
                        div()
                            .absolute()
                            .left(px(rect.x))
                            .top(px(rect.y))
                            .w(px(rect.width))
                            .h(px(rect.height))
                            .border_2()
                            .border_color(t.accent),
                    )
                })
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
            let selector = Selector::new(launch);
            let (width, height) = selector.overlay_size();
            let (x, y) = selector
                .chosen_display()
                .map(DisplayDescriptor::overlay_position)
                .unwrap_or((0., 0.));
            let handle = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Fullscreen(Bounds::new(
                        point(px(x as f32), px(y as f32)),
                        size(px(width), px(height)),
                    ))),
                    titlebar: None,
                    kind: WindowKind::Normal,
                    window_decorations: Some(WindowDecorations::Client),
                    is_movable: false,
                    is_resizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| selector),
            )?;
            handle.update(cx, |_, window, _| {
                if !window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
            })?;
        }
        "recording-hud" => {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| Notice {
                    title: "Recording HUD requires an active recording".into(),
                })
            })?;
        }
        "recording-countdown" => {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| Notice {
                    title: "Countdown requires a capture target".into(),
                })
            })?;
        }
        "screenshot-countdown" => {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| Notice {
                    title: "Countdown requires a capture target".into(),
                })
            })?;
        }
        "recording-region-indicator" => {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| Notice {
                    title: "Recording region".into(),
                })
            })?;
        }
        other => anyhow::bail!("unsupported recording surface: {other}"),
    }
    Ok(())
}
