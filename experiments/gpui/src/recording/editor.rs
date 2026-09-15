#[path = "playback.rs"]
mod playback;

use self::playback::VideoDecoder;
use super::model::{EditorState, timestamped};
use crate::{
    Launch,
    theme::{self, Theme},
};
use captures_media::{CancelToken, ExportFormat, MediaToolchain, QualityPreset};
use gpui::{prelude::*, *};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineDrag {
    Playhead,
    TrimStart,
    TrimEnd,
}

fn timeline_value(x: f32, viewport_width: f32, duration_ms: u64) -> u64 {
    let usable = (viewport_width - 48.).max(1.);
    (((x - 24.) / usable).clamp(0., 1.) * duration_ms as f32).round() as u64
}

pub struct RecordingEditor {
    launch: Launch,
    focus: Option<FocusHandle>,
    source: PathBuf,
    poster: Option<Arc<Path>>,
    frame: Option<Arc<RenderImage>>,
    decoder: VideoDecoder,
    generation: u64,
    state: EditorState,
    has_audio: bool,
    playing: bool,
    playhead_ms: u64,
    clock_started: Option<(Instant, u64)>,
    tick_generation: u64,
    pending_frame: bool,
    drag: Option<TimelineDrag>,
    exporting: bool,
    export_cancel: Option<CancelToken>,
    export_progress: Arc<AtomicU64>,
    status: String,
}

impl RecordingEditor {
    pub fn new(launch: Launch) -> anyhow::Result<Self> {
        let source = launch
            .path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("recording-editor requires --open FILE"))?;
        let tools = MediaToolchain::from_command_names();
        tools
            .verify()
            .map_err(|e| anyhow::anyhow!("Recording editor requires FFmpeg and ffprobe: {e}"))?;
        let probe = tools.probe(&source)?;
        let duration = probe.metadata.duration_ms.unwrap_or(1).max(1);
        std::fs::create_dir_all(&launch.profile)?;
        let poster_path = launch.profile.join("recording-editor-poster.png");
        let poster = tools
            .create_poster(&source, &poster_path, &CancelToken::default())
            .ok()
            .map(|_| Arc::<Path>::from(poster_path));
        let format = if source
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
        {
            ExportFormat::Gif
        } else {
            ExportFormat::Mp4
        };
        Ok(Self {
            decoder: VideoDecoder::new(
                &source,
                probe.metadata.width,
                probe.metadata.height,
                duration,
            ),
            launch,
            focus: None,
            source,
            poster,
            frame: None,
            generation: 0,
            state: EditorState::new(duration, format),
            has_audio: probe.has_audio,
            playing: false,
            playhead_ms: 0,
            clock_started: None,
            tick_generation: 0,
            pending_frame: false,
            drag: None,
            exporting: false,
            export_cancel: None,
            export_progress: Arc::new(AtomicU64::new(0)),
            status: String::new(),
        })
    }

    fn seek(&mut self, timestamp_ms: u64, cx: &mut Context<Self>) {
        self.playhead_ms = timestamp_ms.min(self.state.duration_ms.saturating_sub(1));
        self.pending_frame = true;
        self.clock_started = self.playing.then(|| (Instant::now(), self.playhead_ms));
        match self.decoder.restart(self.playhead_ms, self.playing) {
            Ok(generation) => {
                self.generation = generation;
                self.start_ticks(cx);
            }
            Err(error) => {
                self.playing = false;
                self.pending_frame = false;
                self.status = format!("Preview failed: {error}");
            }
        }
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.playing = false;
            self.clock_started = None;
            self.tick_generation += 1;
            self.decoder.stop();
        } else {
            if self.playhead_ms >= self.state.trim_end_ms.saturating_sub(1) {
                self.playhead_ms = self.state.trim_start_ms;
            }
            self.playing = true;
            self.seek(self.playhead_ms.max(self.state.trim_start_ms), cx);
        }
        cx.notify();
    }

    fn start_ticks(&mut self, cx: &mut Context<Self>) {
        self.tick_generation += 1;
        let tick = self.tick_generation;
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(16)).await;
                let keep_going = this
                    .update(cx, |s, cx| {
                        if s.tick_generation != tick {
                            return false;
                        }
                        let target = s
                            .clock_started
                            .map(|(at, start)| {
                                start.saturating_add(at.elapsed().as_millis() as u64)
                            })
                            .unwrap_or(s.playhead_ms);
                        if let Some((at, image)) =
                            s.decoder.latest_at_or_before(s.generation, target)
                        {
                            if s.pending_frame || at != s.playhead_ms || s.frame.is_none() {
                                s.playhead_ms = at;
                                s.frame = Some(image);
                                cx.notify();
                            }
                            s.pending_frame = false;
                            if !s.playing {
                                s.decoder.stop();
                            }
                        }
                        if s.pending_frame && s.decoder.finished() {
                            s.pending_frame = false;
                            s.playing = false;
                            s.status =
                                "FFmpeg ended without a preview frame at this position".into();
                            cx.notify();
                        }
                        if s.playing && target >= s.state.trim_end_ms {
                            s.playing = false;
                            s.clock_started = None;
                            s.decoder.stop();
                            cx.notify();
                            return false;
                        }
                        s.playing || s.pending_frame
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    fn export(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let ext = if self.state.format == ExportFormat::Gif {
            "gif"
        } else {
            "mp4"
        };
        let dir = self.launch.profile.join("captures");
        let destination = timestamped(&dir, ext);
        if destination == self.source {
            self.status = "Export destination cannot overwrite the source".into();
            return;
        }
        let source = self.source.clone();
        let edit = self.state.edit(self.has_audio);
        let export = self.state.export();
        let cancel = CancelToken::default();
        self.export_cancel = Some(cancel.clone());
        self.exporting = true;
        self.export_progress.store(0, Ordering::Release);
        self.status = "Preparing export…".into();
        let progress = self.export_progress.clone();
        let task = cx.background_executor().spawn(async move {
            std::fs::create_dir_all(&dir).map_err(anyhow::Error::from)?;
            MediaToolchain::from_command_names()
                .export(&source, &destination, &edit, &export, &cancel, |p| {
                    progress.store(p.completed_per_mille as u64, Ordering::Release);
                })
                .map_err(anyhow::Error::from)?;
            Ok::<_, anyhow::Error>(destination)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.exporting = false;
                s.export_cancel = None;
                s.status = match result {
                    Ok(path) => format!("Saved {}", path.display()),
                    Err(e) => format!("Export failed: {e}"),
                };
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                let active = this
                    .update(cx, |s, cx| {
                        if s.exporting {
                            cx.notify();
                        }
                        s.exporting
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn timeline_update(&mut self, event_x: f32, width: f32, cx: &mut Context<Self>) {
        let value = timeline_value(event_x, width, self.state.duration_ms);
        match self.drag.unwrap_or(TimelineDrag::Playhead) {
            TimelineDrag::Playhead => self.seek(value, cx),
            TimelineDrag::TrimStart => {
                self.state.trim_start_ms = value.min(self.state.trim_end_ms.saturating_sub(1));
                self.seek(self.state.trim_start_ms, cx);
            }
            TimelineDrag::TrimEnd => {
                self.state.trim_end_ms = value
                    .max(self.state.trim_start_ms + 1)
                    .min(self.state.duration_ms);
                self.seek(self.state.trim_end_ms.saturating_sub(1), cx);
            }
        }
    }

    fn timeline_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let width = f32::from(window.viewport_size().width);
        let x = f32::from(e.position.x);
        let value = timeline_value(x, width, self.state.duration_ms);
        let tolerance = self.state.duration_ms / 40 + 1;
        self.drag = Some(if value.abs_diff(self.state.trim_start_ms) <= tolerance {
            TimelineDrag::TrimStart
        } else if value.abs_diff(self.state.trim_end_ms) <= tolerance {
            TimelineDrag::TrimEnd
        } else {
            TimelineDrag::Playhead
        });
        self.timeline_update(x, width, cx);
    }
    fn timeline_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            self.timeline_update(
                f32::from(e.position.x),
                f32::from(window.viewport_size().width),
                cx,
            );
        }
    }
    fn key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "space" => self.toggle_play(cx),
            "left" => self.seek(
                self.playhead_ms
                    .saturating_sub(if e.keystroke.modifiers.shift {
                        1000
                    } else {
                        100
                    }),
                cx,
            ),
            "right" => self.seek(
                self.playhead_ms
                    .saturating_add(if e.keystroke.modifiers.shift {
                        1000
                    } else {
                        100
                    })
                    .min(self.state.duration_ms.saturating_sub(1)),
                cx,
            ),
            _ => {}
        }
    }
    fn button(&self, id: &'static str, label: impl IntoElement, t: Theme) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded_md()
            .bg(t.hover)
            .cursor_pointer()
            .child(label)
    }
}

impl Render for RecordingEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::new(self.launch.light);
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let poster = self.poster.clone();
        let frame = self.frame.clone();
        let duration = self.state.duration_ms as f32;
        let start = self.state.trim_start_ms as f32 / duration;
        let end = self.state.trim_end_ms as f32 / duration;
        let playhead = self.playhead_ms as f32 / duration;
        let progress = self.export_progress.load(Ordering::Acquire) as f32 / 10.;
        div()
            .track_focus(&focus)
            .on_mouse_down(MouseButton::Left, move |_, window, _| focus.focus(window))
            .on_key_down(cx.listener(Self::key_down))
            .font_family(theme::font())
            .text_size(px(13.))
            .size_full()
            .bg(t.canvas)
            .text_color(t.text)
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_size(px(24.)).child(
                        if self.state.format == ExportFormat::Gif {
                            "Edit GIF"
                        } else {
                            "Edit recording"
                        },
                    ))
                    .child(
                        div()
                            .text_color(t.muted)
                            .child("Preview · audio playback unavailable"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(240.))
                    .rounded_lg()
                    .bg(t.field)
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(frame, |d, image| {
                        d.child(img(image).size_full().object_fit(ObjectFit::Contain))
                    })
                    .when(self.frame.is_none(), |d| {
                        d.when_some(poster, |d, p| {
                            d.child(img(p).size_full().object_fit(ObjectFit::Contain))
                        })
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(format!(
                        "{:.2}s – {:.2}s",
                        self.state.trim_start_ms as f64 / 1000.,
                        self.state.trim_end_ms as f64 / 1000.
                    ))
                    .child(format!(
                        "{:.2}s selected",
                        (self.state.trim_end_ms - self.state.trim_start_ms) as f64 / 1000.
                    )),
            )
            .child(
                div()
                    .id("timeline")
                    .relative()
                    .h(px(44.))
                    .rounded_md()
                    .bg(t.field)
                    .cursor_pointer()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(start))
                            .right(relative(1. - end))
                            .bg(t.accent)
                            .opacity(0.25),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(start))
                            .w(px(5.))
                            .bg(t.accent),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(end))
                            .w(px(5.))
                            .bg(t.accent),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(playhead))
                            .w(px(2.))
                            .bg(t.text),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::timeline_down))
                    .on_mouse_move(cx.listener(Self::timeline_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|s, _, _, _| s.drag = None))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|s, _, _, _| s.drag = None)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        self.button("play", if self.playing { "Pause" } else { "Play" }, t)
                            .on_click(cx.listener(|s, _, _, cx| s.toggle_play(cx))),
                    )
                    .child(
                        self.button(
                            "format",
                            if self.state.format == ExportFormat::Gif {
                                "GIF"
                            } else {
                                "MP4"
                            },
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.state.format = if s.state.format == ExportFormat::Gif {
                                ExportFormat::Mp4
                            } else {
                                ExportFormat::Gif
                            };
                            cx.notify();
                        })),
                    )
                    .child(
                        self.button(
                            "quality",
                            if self.state.quality == QualityPreset::Preserve {
                                "Original quality"
                            } else {
                                "High quality"
                            },
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.state.quality = if s.state.quality == QualityPreset::Preserve {
                                QualityPreset::High
                            } else {
                                QualityPreset::Preserve
                            };
                            cx.notify();
                        })),
                    )
                    .when(self.has_audio, |d| {
                        d.child(
                            self.button(
                                "audio",
                                if self.state.mute_audio {
                                    "Audio muted"
                                } else {
                                    "Include audio"
                                },
                                t,
                            )
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.state.mute_audio = !s.state.mute_audio;
                                cx.notify()
                            })),
                        )
                    })
                    .child(div().flex_1())
                    .when(self.exporting, |d| {
                        d.child(self.button("cancel", "Cancel", t).on_click(cx.listener(
                            |s, _, _, cx| {
                                if let Some(c) = &s.export_cancel {
                                    c.cancel()
                                }
                                s.status = "Cancelling export…".into();
                                cx.notify()
                            },
                        )))
                    })
                    .child(
                        self.button(
                            "export",
                            if self.exporting {
                                format!("Exporting {progress:.0}%")
                            } else {
                                "Export".into()
                            },
                            t,
                        )
                        .bg(t.accent)
                        .text_color(gpui::black())
                        .on_click(cx.listener(Self::export)),
                    ),
            )
            .child(div().text_color(t.muted).child(self.status.clone()))
    }
}

impl Drop for RecordingEditor {
    fn drop(&mut self) {
        if let Some(cancel) = &self.export_cancel {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn timeline_boundaries_are_asymmetric_and_clamped() {
        assert_eq!(timeline_value(24., 1048., 10_000), 0);
        assert_eq!(timeline_value(1048., 1048., 10_000), 10_000);
        assert_eq!(timeline_value(280., 1048., 10_000), 2_560);
        assert_eq!(timeline_value(-50., 1048., 10_000), 0);
    }
}
