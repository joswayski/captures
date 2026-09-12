use anyhow::Result;
use captures_media::{
    CancelToken, ExportFormat, ExportProgress, ProbeResult, QualityPreset, TimelineSpriteSpec,
};
use gpui::{
    App, AppContext, Bounds, Context, Hsla, Image, ImageFormat, ImageSource, IntoElement,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, RenderImage, SharedString, Timer,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, img, prelude::*, px, rgb, rgba,
    size,
};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::model::{CropHandle, EditorSettings, Resolution, crop_after_drag, format_time};
use crate::ui::{button, metric, render_image, root, theme};

struct LoadedMedia {
    probe: ProbeResult,
    has_system_audio: bool,
    has_microphone_audio: bool,
    poster: Arc<Image>,
    timeline: Arc<Image>,
    scratch: PathBuf,
}

#[derive(Clone, Copy)]
enum TrimHandle {
    Start,
    End,
}

#[derive(Clone, Copy)]
struct TrimDrag {
    handle: TrimHandle,
    pointer: Point<Pixels>,
    start_ms: u64,
    end_ms: u64,
}

#[derive(Clone, Copy)]
struct CropDrag {
    handle: CropHandle,
    pointer: Point<Pixels>,
    initial: captures_media::CropRect,
}

enum PlaybackMessage {
    Frame(u64, Arc<RenderImage>),
    Error(String),
}

struct PlaybackDecode {
    source: PathBuf,
    start_ms: u64,
    end_ms: u64,
    width: u32,
    height: u32,
    audio: Vec<(usize, bool, u16)>,
    cancel: Arc<AtomicBool>,
}

pub(super) struct RecordingEditor {
    source: PathBuf,
    media: Option<LoadedMedia>,
    settings: Option<EditorSettings>,
    frame: Option<ImageSource>,
    loading: bool,
    playing: bool,
    looping: bool,
    fit: bool,
    playhead_ms: u64,
    exporting: bool,
    export_progress: Option<ExportProgress>,
    cancel: Option<CancelToken>,
    save_source: bool,
    compare: Option<(Arc<Image>, Arc<Image>)>,
    playback_generation: u64,
    playback_cancel: Option<Arc<AtomicBool>>,
    trim_drag: Option<TrimDrag>,
    crop_drag: Option<CropDrag>,
    error: Option<String>,
    notice: Option<String>,
}

impl RecordingEditor {
    fn new(source: PathBuf) -> Self {
        Self {
            source,
            media: None,
            settings: None,
            frame: None,
            loading: true,
            playing: false,
            looping: true,
            fit: true,
            playhead_ms: 0,
            exporting: false,
            export_progress: None,
            cancel: None,
            save_source: false,
            compare: None,
            playback_generation: 0,
            playback_cancel: None,
            trim_drag: None,
            crop_drag: None,
            error: None,
            notice: None,
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        let task = cx.background_spawn(async move {
            let media = crate::media::toolchain();
            media.verify().map_err(|error| error.to_string())?;
            let probe = media.probe(&source).map_err(|error| error.to_string())?;
            let (has_system_audio, has_microphone_audio) = audio_layout(&source, &probe);
            let scratch = private_scratch("editor").map_err(|error| error.to_string())?;
            let poster_path = scratch.join("poster.png");
            let timeline_path = scratch.join("timeline.png");
            let cancel = CancelToken::default();
            media
                .create_poster(&source, &poster_path, &cancel)
                .map_err(|error| error.to_string())?;
            media
                .create_timeline_sprite(
                    &source,
                    &timeline_path,
                    TimelineSpriteSpec {
                        duration_ms: probe.metadata.duration_ms.unwrap_or(1).max(1),
                        frame_count: 12,
                        frame_width: 100,
                        frame_height: 68,
                    },
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
            let poster = Arc::new(Image::from_bytes(
                ImageFormat::Png,
                fs::read(poster_path).map_err(|error| error.to_string())?,
            ));
            let timeline = Arc::new(Image::from_bytes(
                ImageFormat::Png,
                fs::read(timeline_path).map_err(|error| error.to_string())?,
            ));
            Ok::<_, String>(LoadedMedia {
                probe,
                has_system_audio,
                has_microphone_audio,
                poster,
                timeline,
                scratch,
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(media) => {
                        let metadata = &media.probe.metadata;
                        let mut settings = EditorSettings::new(
                            metadata.width,
                            metadata.height,
                            metadata.duration_ms.unwrap_or(1),
                            media.has_system_audio,
                            media.has_microphone_audio,
                        );
                        settings.clamp(
                            metadata.width,
                            metadata.height,
                            metadata.duration_ms.unwrap_or(1),
                        );
                        if this
                            .source
                            .extension()
                            .and_then(|value| value.to_str())
                            .is_some_and(|value| value.eq_ignore_ascii_case("gif"))
                        {
                            settings.format = ExportFormat::Gif;
                            settings.quality = QualityPreset::Standard;
                        }
                        this.set_frame(media.poster.clone().into(), cx);
                        this.settings = Some(settings);
                        this.media = Some(media);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn duration(&self) -> u64 {
        self.media
            .as_ref()
            .and_then(|media| media.probe.metadata.duration_ms)
            .unwrap_or(1)
            .max(1)
    }

    fn set_frame(&mut self, frame: ImageSource, cx: &mut Context<Self>) {
        if let Some(ImageSource::Render(previous)) = self.frame.replace(frame) {
            cx.drop_image(previous, None);
        }
    }

    fn seek(&mut self, milliseconds: u64, cx: &mut Context<Self>) {
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        self.playhead_ms = milliseconds.clamp(settings.trim_start_ms, settings.trim_end_ms);
        if self.playing {
            self.start_playback(cx);
            return;
        }
        let source = self.source.clone();
        let at = self.playhead_ms;
        let Some(scratch) = self.media.as_ref().map(|media| media.scratch.clone()) else {
            return;
        };
        let task = cx.background_spawn(async move {
            let path = scratch.join(format!("frame-{at}.png"));
            crate::media::toolchain()
                .extract_frame(&source, at, &path, &CancelToken::default())
                .map_err(|error| error.to_string())?;
            fs::read(path)
                .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            if let Ok(frame) = task.await {
                this.update(cx, |this, cx| {
                    if this.playhead_ms == at {
                        this.set_frame(frame.into(), cx);
                        cx.notify();
                    }
                })?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.pause_playback();
        } else {
            self.playing = true;
            self.start_playback(cx);
        }
        cx.notify();
    }

    fn pause_playback(&mut self) {
        self.playing = false;
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
    }

    fn audio_settings_changed(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.start_playback(cx);
        }
        cx.notify();
    }

    fn start_playback(&mut self, cx: &mut Context<Self>) {
        let (Some(media), Some(settings)) = (self.media.as_ref(), self.settings.as_ref()) else {
            self.playing = false;
            return;
        };
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        if self.playhead_ms >= settings.trim_end_ms {
            self.playhead_ms = settings.trim_start_ms;
        }
        self.playback_generation = self.playback_generation.wrapping_add(1);
        let generation = self.playback_generation;
        let source = self.source.clone();
        let start_ms = self.playhead_ms;
        let end_ms = settings.trim_end_ms;
        let looping = self.looping;
        let mut audio = Vec::with_capacity(2);
        if media.has_system_audio {
            audio.push((0, settings.mute_system, settings.system_volume));
        }
        if media.has_microphone_audio {
            audio.push((
                usize::from(media.has_system_audio),
                settings.mute_microphone,
                settings.microphone_volume,
            ));
        }
        let (width, height) =
            playback_dimensions(media.probe.metadata.width, media.probe.metadata.height);
        let cancel = Arc::new(AtomicBool::new(false));
        self.playback_cancel = Some(cancel.clone());
        let (sender, receiver) = mpsc::sync_channel(2);
        cx.background_spawn(async move {
            decode_playback(
                PlaybackDecode {
                    source,
                    start_ms,
                    end_ms,
                    width,
                    height,
                    audio,
                    cancel,
                },
                sender,
            );
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(16)).await;
                let mut disconnected = false;
                let mut latest = None;
                let mut playback_error = None;
                loop {
                    match receiver.try_recv() {
                        Ok(PlaybackMessage::Frame(at, frame)) => latest = Some((at, frame)),
                        Ok(PlaybackMessage::Error(error)) => playback_error = Some(error),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                let keep = this.update(cx, |this, cx| {
                    if this.playback_generation != generation || !this.playing {
                        return false;
                    }
                    if let Some((at, frame)) = latest {
                        this.playhead_ms = at.min(end_ms);
                        this.set_frame(frame.into(), cx);
                        cx.notify();
                    }
                    if let Some(error) = playback_error {
                        this.error = Some(error);
                        this.pause_playback();
                        cx.notify();
                        return false;
                    }
                    if disconnected {
                        if looping {
                            this.playhead_ms = this
                                .settings
                                .as_ref()
                                .map_or(0, |settings| settings.trim_start_ms);
                            this.start_playback(cx);
                        } else {
                            this.pause_playback();
                        }
                        return false;
                    }
                    true
                })?;
                if !keep {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn trim_start(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(settings) = self.settings.as_mut() else {
            return;
        };
        settings.trim_start_ms =
            add_signed(settings.trim_start_ms, delta).min(settings.trim_end_ms.saturating_sub(1));
        self.playhead_ms = settings.trim_start_ms;
        self.seek(self.playhead_ms, cx);
    }

    fn trim_end(&mut self, delta: i64, cx: &mut Context<Self>) {
        let duration = self.duration();
        let Some(settings) = self.settings.as_mut() else {
            return;
        };
        settings.trim_end_ms =
            add_signed(settings.trim_end_ms, delta).clamp(settings.trim_start_ms + 1, duration);
        self.playhead_ms = settings.trim_end_ms;
        self.seek(self.playhead_ms, cx);
    }

    fn begin_trim_drag(&mut self, handle: TrimHandle, event: &MouseDownEvent) {
        let Some((start_ms, end_ms)) = self
            .settings
            .as_ref()
            .map(|settings| (settings.trim_start_ms, settings.trim_end_ms))
        else {
            return;
        };
        self.pause_playback();
        self.trim_drag = Some(TrimDrag {
            handle,
            pointer: event.position,
            start_ms,
            end_ms,
        });
    }

    fn update_trim_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let duration = self.duration();
        let (Some(drag), Some(settings)) = (self.trim_drag, self.settings.as_mut()) else {
            return;
        };
        if !event.dragging() {
            self.trim_drag = None;
            return;
        }
        let track_width = (window.viewport_size().width - px(112.)).max(px(1.));
        let delta =
            ((event.position.x - drag.pointer.x) / track_width * duration as f32).round() as i64;
        match drag.handle {
            TrimHandle::Start => {
                settings.trim_start_ms =
                    add_signed(drag.start_ms, delta).min(settings.trim_end_ms.saturating_sub(1));
                self.playhead_ms = settings.trim_start_ms;
            }
            TrimHandle::End => {
                settings.trim_end_ms =
                    add_signed(drag.end_ms, delta).clamp(settings.trim_start_ms + 1, duration);
                self.playhead_ms = settings.trim_end_ms.saturating_sub(1);
            }
        }
        cx.notify();
    }

    fn begin_crop_drag(&mut self, handle: CropHandle, event: &MouseDownEvent) {
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        self.crop_drag = Some(CropDrag {
            handle,
            pointer: event.position,
            initial: settings.crop,
        });
    }

    fn update_crop_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(drag), Some(settings), Some(media)) =
            (self.crop_drag, self.settings.as_mut(), self.media.as_ref())
        else {
            return;
        };
        if !event.dragging() {
            self.crop_drag = None;
            return;
        }
        let width = media.probe.metadata.width;
        let height = media.probe.metadata.height;
        let canvas_width = ((window.viewport_size().width - px(136.)) / 2.).max(px(1.));
        let delta_x =
            ((event.position.x - drag.pointer.x) / canvas_width * width as f32).round() as i32;
        let delta_y =
            ((event.position.y - drag.pointer.y) / px(180.) * height as f32).round() as i32;
        settings.crop = crop_after_drag(
            drag.initial,
            drag.handle,
            delta_x,
            delta_y,
            width,
            height,
            settings.aspect_locked,
        );
        settings.crop_enabled = true;
        cx.notify();
    }

    fn compare(&mut self, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let Some(media) = self.media.as_ref() else {
            return;
        };
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        if settings.format == ExportFormat::WebM {
            self.error = Some(
                "WebM export is not available in the shared media backend. Choose MP4 or GIF."
                    .into(),
            );
            cx.notify();
            return;
        }
        let source = self.source.clone();
        let before = media.scratch.join("compare-before.png");
        let after = media.scratch.join("compare-after.png");
        let encoded = media.scratch.join(match settings.format {
            ExportFormat::Gif => "compare.gif",
            _ => "compare.mp4",
        });
        let at = self.playhead_ms.clamp(
            settings.trim_start_ms,
            settings.trim_end_ms.saturating_sub(1),
        );
        let (edit, spec) = settings.specs(
            media.probe.metadata.width,
            media.probe.metadata.height,
            media.has_system_audio,
            media.has_microphone_audio,
        );
        self.notice = Some("Preparing before / after comparison…".into());
        let task = cx.background_spawn(async move {
            let tools = crate::media::toolchain();
            let cancel = CancelToken::default();
            tools
                .extract_frame(&source, at, &before, &cancel)
                .map_err(|error| error.to_string())?;
            tools
                .export(&source, &encoded, &edit, &spec, &cancel, |_| {})
                .map_err(|error| error.to_string())?;
            tools
                .extract_frame(
                    &encoded,
                    at.saturating_sub(edit.trim_start_ms),
                    &after,
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((
                Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    fs::read(before).map_err(|error| error.to_string())?,
                )),
                Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    fs::read(after).map_err(|error| error.to_string())?,
                )),
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.notice = None;
                match result {
                    Ok(images) => this.compare = Some(images),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn request_export(&mut self, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        if settings.format == ExportFormat::WebM {
            self.error = Some(
                "WebM export is not available in the shared media backend. Choose MP4 or GIF."
                    .into(),
            );
            cx.notify();
            return;
        }
        if self.save_source {
            self.export_to(self.source.clone(), true, cx);
            return;
        }
        let directory = self
            .source
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let stem = self
            .source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Recording");
        let extension = extension(settings.format);
        let suggested = format!("{stem} edited.{extension}");
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn(async move |this, cx| {
            match path.await {
                Ok(Ok(Some(path))) => {
                    this.update(cx, |this, cx| this.export_to(path, false, cx))?;
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        this.error = Some(format!("Could not choose where to save: {error}"));
                        cx.notify();
                    })?;
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.error = Some(format!("Save dialog did not complete: {error}"));
                        cx.notify();
                    })?;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn export_to(&mut self, destination: PathBuf, replace_source: bool, cx: &mut Context<Self>) {
        let Some(media) = self.media.as_ref() else {
            return;
        };
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        let source_extension = self
            .source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if replace_source && !source_extension.eq_ignore_ascii_case(extension(settings.format)) {
            self.error =
                Some("Saving over the source requires keeping its original format.".into());
            cx.notify();
            return;
        }
        let source = self.source.clone();
        let (edit, spec) = settings.specs(
            media.probe.metadata.width,
            media.probe.metadata.height,
            media.has_system_audio,
            media.has_microphone_audio,
        );
        let temporary = if replace_source {
            destination.with_file_name(format!(
                ".{}.captures-edit.tmp.{}",
                destination
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or("recording"),
                extension(settings.format)
            ))
        } else {
            destination.clone()
        };
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.exporting = true;
        self.export_progress = Some(ExportProgress {
            stage: captures_media::ExportStage::Preparing,
            completed_per_mille: 0,
            attempt: 1,
            message: None,
        });
        self.error = None;
        let (progress_tx, progress_rx) = mpsc::channel();
        let task = cx.background_spawn(async move {
            let result = crate::media::toolchain()
                .export(&source, &temporary, &edit, &spec, &cancel, |progress| {
                    let _ = progress_tx.send(progress);
                })
                .map_err(|error| error.to_string());
            match result {
                Ok(outcome) if replace_source => {
                    fs::rename(&outcome.path, &destination).map_err(|error| error.to_string())?;
                    Ok(destination)
                }
                Ok(outcome) => Ok(outcome.path),
                Err(error) => {
                    let _ = fs::remove_file(&temporary);
                    Err(error)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                let keep_polling = this.update(cx, |this, cx| {
                    while let Ok(progress) = progress_rx.try_recv() {
                        this.export_progress = Some(progress);
                    }
                    cx.notify();
                    this.exporting
                })?;
                if !keep_polling {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.exporting = false;
                this.cancel = None;
                match result {
                    Ok(path) => {
                        this.notice = Some(format!("Saved {}", path.display()));
                        crate::app::saved(path, cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
        })
        .detach();
        cx.notify();
    }

    fn editor_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        button(id, label, theme(cx))
    }

    fn card(title: &'static str, cx: &Context<Self>) -> gpui::Div {
        let colors = theme(cx);
        div()
            .flex()
            .flex_col()
            .gap(metric("--s-5"))
            .p(metric("--s-6"))
            .rounded(metric("--r-lg"))
            .border_1()
            .border_color(colors.border())
            .bg(colors.raised())
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            )
    }
}

impl Drop for RecordingEditor {
    fn drop(&mut self) {
        self.pause_playback();
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        if let Some(media) = self.media.take() {
            let _ = fs::remove_dir_all(media.scratch);
        }
    }
}

impl Render for RecordingEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme(cx);
        let mut page = root(colors)
            .id("recording-editor-page")
            .overflow_y_scroll()
            .overflow_x_hidden()
            .p(metric("--s-8"))
            .gap(metric("--s-6"));
        page = page.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(24.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(
                            if self
                                .source
                                .extension()
                                .and_then(|v| v.to_str())
                                .is_some_and(|v| v.eq_ignore_ascii_case("gif"))
                            {
                                "Edit GIF"
                            } else {
                                "Edit recording"
                            },
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted())
                        .child(self.source.display().to_string()),
                ),
        );
        if self.loading {
            return page.child(
                div()
                    .p(metric("--s-8"))
                    .child("Preparing preview and timeline…"),
            );
        }
        if self.media.is_none() || self.settings.is_none() {
            return page.child(
                div().text_color(rgb(0xd54b55)).child(
                    self.error
                        .clone()
                        .unwrap_or_else(|| "The recording could not be opened.".into()),
                ),
            );
        }
        let media = self.media.as_ref().unwrap();
        let settings = self.settings.as_ref().unwrap().clone();
        let metadata = &media.probe.metadata;
        let duration = self.duration();
        let selected = settings.trim_end_ms.saturating_sub(settings.trim_start_ms);
        let estimate = estimated_size(
            metadata.size_bytes,
            duration,
            selected,
            settings.quality,
            settings.format,
        );
        let preview = self.compare.as_ref().map_or_else(
            || {
                self.frame
                    .clone()
                    .unwrap_or_else(|| media.poster.clone().into())
            },
            |(_, after)| after.clone().into(),
        );
        let preview_card = div()
            .flex()
            .flex_col()
            .rounded(metric("--r-lg"))
            .border_1()
            .border_color(colors.border())
            .bg(colors.raised())
            .child(
                div()
                    .h(px(52.))
                    .px(metric("--s-6"))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Preview"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(metric("--s-4"))
                            .child(
                                self.editor_button(
                                    "preview-loop",
                                    if self.looping {
                                        "↻ Loop on"
                                    } else {
                                        "↻ Loop off"
                                    },
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.looping = !this.looping;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.editor_button(
                                    "preview-size",
                                    if self.fit { "Fit" } else { "100%" },
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.fit = !this.fit;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                div()
                    .relative()
                    .h(px(400.))
                    .overflow_hidden()
                    .bg(rgb(0x090b0e))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(preview.clone())
                            .when(self.fit, |image| image.max_w_full().max_h_full())
                            .when(!self.fit, |image| {
                                image
                                    .w(px(metadata.width as f32))
                                    .h(px(metadata.height as f32))
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id("preview-play")
                                    .size(px(58.))
                                    .rounded_full()
                                    .bg(rgba(0x11151bc7))
                                    .text_color(rgb(0xffffff))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(22.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx)))
                                    .child(if self.playing { "Ⅱ" } else { "▶" }),
                            ),
                    ),
            )
            .when_some(self.compare.as_ref(), |card, _| {
                card.child(
                    div()
                        .px(metric("--s-6"))
                        .py(metric("--s-4"))
                        .text_size(px(12.))
                        .text_color(colors.muted())
                        .child("Before / after comparison · showing encoded result"),
                )
            });
        page = page.child(preview_card);

        let start_percent = settings.trim_start_ms as f32 / duration as f32 * 100.;
        let end_percent = settings.trim_end_ms as f32 / duration as f32 * 100.;
        let selected_percent = selected as f32 / duration as f32 * 100.;
        let playhead_percent = self.playhead_ms as f32 / duration as f32 * 100.;
        page = page.child(
            Self::card("Timeline", cx)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(format!(
                            "{} – {}",
                            format_time(settings.trim_start_ms, true),
                            format_time(settings.trim_end_ms, true)
                        ))
                        .child(format!("{} selected", format_time(selected, true))),
                )
                .child(
                    div()
                        .id("recording-timeline-drag-area")
                        .relative()
                        .h(px(68.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .cursor_col_resize()
                        .on_mouse_move(cx.listener(Self::update_trim_drag))
                        .on_mouse_up(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.trim_drag = None;
                                this.seek(this.playhead_ms, cx);
                            }),
                        )
                        .child(img(media.timeline.clone()).size_full())
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(gpui::relative(start_percent / 100.))
                                .bg(rgba(0x00000094)),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(gpui::relative(start_percent / 100.))
                                .w(gpui::relative(selected_percent / 100.))
                                .border_2()
                                .border_color(colors.accent),
                        )
                        .child(
                            div()
                                .id("trim-start-handle")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(gpui::relative(start_percent / 100.))
                                .ml(px(-6.))
                                .w(px(12.))
                                .rounded(px(4.))
                                .bg(colors.accent)
                                .cursor_col_resize()
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|this, event, _, _| {
                                        this.begin_trim_drag(TrimHandle::Start, event)
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(gpui::relative(playhead_percent / 100.))
                                .w(px(2.))
                                .bg(rgb(0xffffff)),
                        )
                        .child(
                            div()
                                .id("trim-end-handle")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(gpui::relative(end_percent / 100.))
                                .ml(px(-6.))
                                .w(px(12.))
                                .rounded(px(4.))
                                .bg(colors.accent)
                                .cursor_col_resize()
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|this, event, _, _| {
                                        this.begin_trim_drag(TrimHandle::End, event)
                                    }),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-4"))
                        .child(
                            self.editor_button("trim-start-back", "Start −0.5s", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.trim_start(-500, cx))),
                        )
                        .child(
                            self.editor_button("trim-start-forward", "Start +0.5s", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.trim_start(500, cx))),
                        )
                        .child(
                            self.editor_button("trim-end-back", "End −0.5s", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.trim_end(-500, cx))),
                        )
                        .child(
                            self.editor_button("trim-end-forward", "End +0.5s", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.trim_end(500, cx))),
                        ),
                ),
        );

        let crop = settings.crop;
        let picture = Self::card("Crop & size", cx)
            .child(
                self.editor_button(
                    "crop-toggle",
                    if settings.crop_enabled {
                        "✓ Crop recording"
                    } else {
                        "Crop recording"
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(settings) = this.settings.as_mut() {
                        settings.crop_enabled = !settings.crop_enabled;
                    }
                    cx.notify();
                })),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.muted())
                    .child(format!(
                        "X {} · Y {} · {} × {}",
                        crop.x, crop.y, crop.width, crop.height
                    )),
            )
            .child(
                div()
                    .id("crop-drag-area")
                    .relative()
                    .h(px(180.))
                    .overflow_hidden()
                    .rounded(px(8.))
                    .bg(rgb(0x090b0e))
                    .on_mouse_move(cx.listener(Self::update_crop_drag))
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.crop_drag = None;
                            cx.notify();
                        }),
                    )
                    .child(img(preview.clone()).size_full().opacity(0.72))
                    .child(
                        div()
                            .id("crop-selection")
                            .absolute()
                            .left(gpui::relative(crop.x as f32 / metadata.width.max(1) as f32))
                            .top(gpui::relative(
                                crop.y as f32 / metadata.height.max(1) as f32,
                            ))
                            .w(gpui::relative(
                                crop.width as f32 / metadata.width.max(1) as f32,
                            ))
                            .h(gpui::relative(
                                crop.height as f32 / metadata.height.max(1) as f32,
                            ))
                            .border_2()
                            .border_color(colors.accent)
                            .cursor_move()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, event, _, _| {
                                    this.begin_crop_drag(CropHandle::Move, event)
                                }),
                            )
                            .child(crop_handle(
                                "crop-north-west",
                                0.,
                                0.,
                                CropHandle::NorthWest,
                                colors.accent,
                                cx,
                            ))
                            .child(crop_handle(
                                "crop-north-east",
                                1.,
                                0.,
                                CropHandle::NorthEast,
                                colors.accent,
                                cx,
                            ))
                            .child(crop_handle(
                                "crop-south-west",
                                0.,
                                1.,
                                CropHandle::SouthWest,
                                colors.accent,
                                cx,
                            ))
                            .child(crop_handle(
                                "crop-south-east",
                                1.,
                                1.,
                                CropHandle::SouthEast,
                                colors.accent,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(metric("--s-4"))
                    .child(
                        self.editor_button("crop-left", "←", cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(s) = this.settings.as_mut() {
                                    s.crop.x = s.crop.x.saturating_sub(8);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        self.editor_button("crop-right", "→", cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let (Some(s), Some(m)) =
                                    (this.settings.as_mut(), this.media.as_ref())
                                {
                                    s.crop.x = (s.crop.x + 8)
                                        .min(m.probe.metadata.width.saturating_sub(s.crop.width));
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        self.editor_button("crop-narrow", "Width −", cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(s) = this.settings.as_mut() {
                                    s.crop.width = s.crop.width.saturating_sub(16).max(2);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        self.editor_button("crop-wide", "Width +", cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let (Some(s), Some(m)) =
                                    (this.settings.as_mut(), this.media.as_ref())
                                {
                                    s.crop.width =
                                        (s.crop.width + 16).min(m.probe.metadata.width - s.crop.x);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                self.editor_button(
                    "aspect-toggle",
                    if settings.aspect_locked {
                        "✓ Lock aspect ratio"
                    } else {
                        "Lock aspect ratio"
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(s) = this.settings.as_mut() {
                        s.aspect_locked = !s.aspect_locked;
                    }
                    cx.notify();
                })),
            )
            .child(
                self.editor_button(
                    "resolution-cycle",
                    format!("Output: {}", resolution_name(settings.resolution)),
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(s) = this.settings.as_mut() {
                        s.resolution = next_resolution(s.resolution);
                    }
                    cx.notify();
                })),
            );

        let quality = Self::card("Save quality", cx)
            .child(self.editor_button("format-cycle", format!("Format: {}", format_name(settings.format)), cx).on_click(cx.listener(|this, _, _, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.format = match s.format { ExportFormat::Mp4 => ExportFormat::Gif, ExportFormat::Gif => ExportFormat::WebM, ExportFormat::WebM => ExportFormat::Mp4 };
                    if s.format == ExportFormat::Gif && s.quality == QualityPreset::Preserve { s.quality = QualityPreset::Standard; }
                    if s.format == ExportFormat::WebM { this.error = Some("WebM export is not available in the shared media backend. Choose MP4 or GIF.".into()); }
                }
                cx.notify();
            })))
            .child(self.editor_button("quality-cycle", format!("Quality: {}", quality_name(settings.quality)), cx).on_click(cx.listener(|this, _, _, cx| { if let Some(s) = this.settings.as_mut() { s.quality = next_quality(s.quality, s.format); } cx.notify(); })))
            .child(self.editor_button("maximum-size", settings.maximum_size_bytes.map_or_else(|| "Maximum size: Off".to_owned(), |bytes| format!("Maximum size: {} MB", bytes / 1_000_000)), cx).on_click(cx.listener(|this, _, _, cx| { if let Some(s) = this.settings.as_mut() { s.maximum_size_bytes = if s.maximum_size_bytes.is_some() { None } else { Some(10_000_000) }; } cx.notify(); })))
            .child(div().text_size(px(12.)).text_color(colors.muted()).child(format!("Estimated saved size {}", human_bytes(estimate))))
            .child(self.editor_button("compare", "Compare before / after", cx).on_click(cx.listener(|this, _, _, cx| this.compare(cx))));

        let audio = Self::card("Audio", cx)
            .when(!media.probe.has_audio, |card| {
                card.child(
                    div()
                        .text_color(colors.muted())
                        .child("No audio tracks in this source"),
                )
            })
            .when(media.has_system_audio, |card| {
                card.child(
                    self.editor_button(
                        "system-volume",
                        format!("System audio {}%", settings.system_volume),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.settings.as_mut() {
                            s.system_volume = if s.system_volume >= 200 {
                                0
                            } else {
                                s.system_volume + 25
                            };
                        }
                        this.audio_settings_changed(cx);
                    })),
                )
                .child(
                    self.editor_button(
                        "mute-system",
                        if settings.mute_system {
                            "System audio muted"
                        } else {
                            "Mute system audio"
                        },
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.settings.as_mut() {
                            s.mute_system = !s.mute_system;
                        }
                        this.audio_settings_changed(cx);
                    })),
                )
            })
            .when(media.has_microphone_audio, |card| {
                card.child(
                    self.editor_button(
                        "microphone-volume",
                        format!("Microphone {}%", settings.microphone_volume),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.settings.as_mut() {
                            s.microphone_volume = if s.microphone_volume >= 200 {
                                0
                            } else {
                                s.microphone_volume + 25
                            };
                        }
                        this.audio_settings_changed(cx);
                    })),
                )
                .child(
                    self.editor_button(
                        "mute-microphone",
                        if settings.mute_microphone {
                            "Microphone muted"
                        } else {
                            "Mute microphone"
                        },
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.settings.as_mut() {
                            s.mute_microphone = !s.mute_microphone;
                        }
                        this.audio_settings_changed(cx);
                    })),
                )
            })
            .when(media.probe.has_audio, |card| {
                card.child(
                    self.editor_button(
                        "mono",
                        if settings.mono {
                            "✓ Mix output to mono"
                        } else {
                            "Mix output to mono"
                        },
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.settings.as_mut() {
                            s.mono = !s.mono;
                        }
                        this.audio_settings_changed(cx);
                    })),
                )
            });

        page = page.child(
            div()
                .grid()
                .grid_cols(2)
                .gap(metric("--s-6"))
                .child(picture)
                .child(quality)
                .child(audio),
        );
        if let Some(error) = self.error.clone() {
            page = page.child(
                div()
                    .p(metric("--s-5"))
                    .rounded(metric("--r-sm"))
                    .bg(rgb(0x5a1d24))
                    .text_color(rgb(0xffd7da))
                    .child(error),
            );
        }
        if let Some(notice) = self.notice.clone() {
            page = page.child(div().text_color(colors.muted()).child(notice));
        }
        page.child(
            div()
                .bottom_0()
                .p(metric("--s-5"))
                .rounded(metric("--r-lg"))
                .bg(colors.raised())
                .border_1()
                .border_color(colors.border())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    self.editor_button(
                        "save-source-toggle",
                        if self.save_source {
                            "Save: Replace source"
                        } else {
                            "Save: New file"
                        },
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_source = !this.save_source;
                        if this.save_source
                            && let Some(settings) = this.settings.as_mut()
                        {
                            settings.format = if this
                                .source
                                .extension()
                                .and_then(|v| v.to_str())
                                .is_some_and(|v| v.eq_ignore_ascii_case("gif"))
                            {
                                ExportFormat::Gif
                            } else {
                                ExportFormat::Mp4
                            };
                        }
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .flex()
                        .gap(metric("--s-4"))
                        .when(self.exporting, |row| {
                            row.child(
                                self.editor_button("cancel-export", "Cancel export", cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(cancel) = &this.cancel {
                                            cancel.cancel();
                                        }
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            self.editor_button(
                                "save-export",
                                if self.exporting {
                                    SharedString::from(format!(
                                        "Saving… {}%",
                                        self.export_progress
                                            .as_ref()
                                            .map_or(0, |progress| progress.completed_per_mille
                                                / 10)
                                    ))
                                } else {
                                    SharedString::from("Save")
                                },
                                cx,
                            )
                            .when(!self.exporting, |button| {
                                button
                                    .bg(colors.accent)
                                    .border_color(colors.accent)
                                    .text_color(rgb(0xffffff))
                                    .on_click(cx.listener(|this, _, _, cx| this.request_export(cx)))
                            }),
                        ),
                ),
        )
    }
}

pub(super) fn open(path: PathBuf, cx: &mut App) -> Result<()> {
    if !path.is_file() {
        anyhow::bail!("recording does not exist: {}", path.display());
    }
    let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Captures GPUI Recording".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(920.), px(620.))),
            app_id: Some("captures-gpui-recording-editor".into()),
            ..Default::default()
        },
        move |_, cx| cx.new(|_| RecordingEditor::new(path)),
    )?;
    handle.update(cx, |editor, _, cx| {
        editor.load(cx);
    })?;
    Ok(())
}

fn private_scratch(label: &str) -> std::io::Result<PathBuf> {
    let root = crate::settings::data_dir().join("media-work");
    let path = root.join(format!(
        "{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    crate::desktop::private_directory(&path)?;
    Ok(path)
}

fn add_signed(value: u64, delta: i64) -> u64 {
    if delta < 0 {
        value.saturating_sub(delta.unsigned_abs())
    } else {
        value.saturating_add(delta as u64)
    }
}
fn extension(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Mp4 => "mp4",
        ExportFormat::Gif => "gif",
        ExportFormat::WebM => "webm",
    }
}
fn format_name(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Mp4 => "MP4",
        ExportFormat::Gif => "GIF",
        ExportFormat::WebM => "WebM unavailable",
    }
}
fn resolution_name(value: Resolution) -> &'static str {
    match value {
        Resolution::Original => "Original",
        Resolution::P1080 => "1080p maximum",
        Resolution::P720 => "720p maximum",
        Resolution::Half => "Half size",
    }
}
fn next_resolution(value: Resolution) -> Resolution {
    match value {
        Resolution::Original => Resolution::P1080,
        Resolution::P1080 => Resolution::P720,
        Resolution::P720 => Resolution::Half,
        Resolution::Half => Resolution::Original,
    }
}
fn quality_name(value: QualityPreset) -> &'static str {
    match value {
        QualityPreset::Preserve => "Preserve",
        QualityPreset::Highest => "Highest",
        QualityPreset::High => "High",
        QualityPreset::Standard => "Balanced",
        QualityPreset::Small => "Smaller",
        QualityPreset::Tiny => "Tiny",
    }
}
fn next_quality(value: QualityPreset, format: ExportFormat) -> QualityPreset {
    match value {
        QualityPreset::Preserve => QualityPreset::Highest,
        QualityPreset::Highest => QualityPreset::High,
        QualityPreset::High => QualityPreset::Standard,
        QualityPreset::Standard => QualityPreset::Small,
        QualityPreset::Small => QualityPreset::Tiny,
        QualityPreset::Tiny => {
            if format == ExportFormat::Mp4 {
                QualityPreset::Preserve
            } else {
                QualityPreset::Highest
            }
        }
    }
}
fn estimated_size(
    source: u64,
    duration: u64,
    selected: u64,
    quality: QualityPreset,
    format: ExportFormat,
) -> u64 {
    let trim = source.saturating_mul(selected) / duration.max(1);
    let factor = match (format, quality) {
        (ExportFormat::Gif, QualityPreset::Tiny) => 25,
        (ExportFormat::Gif, _) => 65,
        (_, QualityPreset::Preserve) => 100,
        (_, QualityPreset::Highest) => 82,
        (_, QualityPreset::High) => 66,
        (_, QualityPreset::Standard) => 48,
        (_, QualityPreset::Small) => 32,
        (_, QualityPreset::Tiny) => 20,
    };
    trim.saturating_mul(factor) / 100
}
fn human_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{} KB", bytes / 1_000)
    }
}

fn playback_dimensions(width: u32, height: u32) -> (u32, u32) {
    let scale = (960.0 / f64::from(width.max(1)))
        .min(540.0 / f64::from(height.max(1)))
        .min(1.0);
    (
        (f64::from(width) * scale).round().max(2.0) as u32 & !1,
        (f64::from(height) * scale).round().max(2.0) as u32 & !1,
    )
}

fn audio_layout(source: &Path, probe: &ProbeResult) -> (bool, bool) {
    if probe.audio_stream_count == 0 {
        return (false, false);
    }
    if probe.audio_stream_count >= 2 {
        return (true, true);
    }
    let output = Command::new(crate::media::tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream_tags=title",
            "-of",
            "json",
        ])
        .arg(source)
        .output();
    let microphone_only = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .and_then(|value| {
            value
                .get("streams")
                .and_then(|streams| streams.as_array())
                .cloned()
        })
        .is_some_and(|streams| {
            streams.iter().any(|stream| {
                stream
                    .pointer("/tags/title")
                    .and_then(|title| title.as_str())
                    .is_some_and(|title| title.to_ascii_lowercase().contains("microphone"))
            })
        });
    (!microphone_only, microphone_only)
}

fn crop_handle(
    id: &'static str,
    x: f32,
    y: f32,
    handle: CropHandle,
    color: Hsla,
    cx: &Context<RecordingEditor>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .absolute()
        .left(gpui::relative(x))
        .top(gpui::relative(y))
        .ml(px(-6.))
        .mt(px(-6.))
        .size(px(12.))
        .rounded_full()
        .border_2()
        .border_color(rgb(0xffffff))
        .bg(color)
        .cursor_pointer()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |this, event, _, cx| {
                cx.stop_propagation();
                this.begin_crop_drag(handle, event);
            }),
        )
}

fn decode_playback(request: PlaybackDecode, sender: mpsc::SyncSender<PlaybackMessage>) {
    let PlaybackDecode {
        source,
        start_ms,
        end_ms,
        width,
        height,
        audio,
        cancel,
    } = request;
    let filter = format!("fps=30,scale={width}:{height}");
    let mut child = match Command::new(crate::media::tool("ffmpeg"))
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{:.3}", start_ms as f64 / 1_000.0))
        .arg("-i")
        .arg(&source)
        .args(["-an", "-t"])
        .arg(format!(
            "{:.3}",
            end_ms.saturating_sub(start_ms) as f64 / 1_000.0
        ))
        .args([
            "-vf", &filter, "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(PlaybackMessage::Error(format!(
                "Could not start video preview: {error}"
            )));
            return;
        }
    };
    let mut audio_children = audio
        .into_iter()
        .filter(|(_, muted, volume)| !muted && *volume > 0)
        .map(|(stream, _, volume)| {
            Command::new(crate::media::tool("ffplay"))
                .args(["-nodisp", "-autoexit", "-loglevel", "error", "-ss"])
                .arg(format!("{:.3}", start_ms as f64 / 1_000.0))
                .arg("-t")
                .arg(format!(
                    "{:.3}",
                    end_ms.saturating_sub(start_ms) as f64 / 1_000.0
                ))
                .arg("-volume")
                .arg(volume.min(200).to_string())
                .arg("-ast")
                .arg(stream.to_string())
                .arg(&source)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        })
        .collect::<Vec<_>>();
    let mut stdout = child.stdout.take().expect("piped FFmpeg stdout");
    let mut bytes = vec![0; width as usize * height as usize * 4];
    let mut index = 0_u64;
    let clock = Instant::now();
    while !cancel.load(Ordering::Acquire) && stdout.read_exact(&mut bytes).is_ok() {
        let Some(image) = image::RgbaImage::from_raw(width, height, bytes.clone()) else {
            break;
        };
        let at = start_ms.saturating_add(index.saturating_mul(1_000) / 30);
        let target = Duration::from_millis(index.saturating_mul(1_000) / 30);
        if let Some(wait) = target.checked_sub(clock.elapsed()) {
            std::thread::sleep(wait);
        }
        if sender
            .send(PlaybackMessage::Frame(at, render_image(image)))
            .is_err()
        {
            break;
        }
        index = index.saturating_add(1);
    }
    let _ = child.kill();
    let _ = child.wait();
    for child in audio_children.iter_mut().flatten() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn estimates_distinguish_trim_and_quality() {
        assert_eq!(
            estimated_size(
                100_000_000,
                10_000,
                2_500,
                QualityPreset::Preserve,
                ExportFormat::Mp4
            ),
            25_000_000
        );
        assert_eq!(
            estimated_size(
                100_000_000,
                10_000,
                2_500,
                QualityPreset::Tiny,
                ExportFormat::Mp4
            ),
            5_000_000
        );
    }
    #[test]
    fn negative_timeline_steps_saturate() {
        assert_eq!(add_signed(200, -500), 0);
        assert_eq!(add_signed(200, 500), 700);
    }

    #[test]
    fn editor_specs_produce_real_cropped_mp4_and_gif_outputs() {
        let directory = private_scratch("export-test").unwrap();
        let source = directory.join("source.mp4");
        let generated = Command::new(crate::media::tool("ffmpeg"))
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=30:duration=2",
                "-c:v",
                "mpeg4",
            ])
            .arg(&source)
            .status()
            .expect("run FFmpeg fixture generator");
        assert!(generated.success());

        let media = crate::media::toolchain();
        let mut settings = EditorSettings::new(640, 360, 2_000, false, false);
        settings.trim_start_ms = 250;
        settings.trim_end_ms = 1_250;
        settings.crop_enabled = true;
        settings.crop = captures_media::CropRect {
            x: 40,
            y: 20,
            width: 400,
            height: 200,
        };
        settings.aspect_locked = false;
        settings.quality = QualityPreset::Standard;

        for (format, name) in [
            (ExportFormat::Mp4, "edited.mp4"),
            (ExportFormat::Gif, "edited.gif"),
        ] {
            settings.format = format;
            let destination = directory.join(name);
            let (edit, export) = settings.specs(640, 360, false, false);
            media
                .export(
                    &source,
                    &destination,
                    &edit,
                    &export,
                    &CancelToken::default(),
                    |_| {},
                )
                .unwrap();
            let result = media.probe(&destination).unwrap();
            assert_eq!((result.metadata.width, result.metadata.height), (400, 200));
            assert!(destination.metadata().unwrap().len() > 1_000);
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
