//! Frame-based recording editor. Media work stays on one serialized worker;
//! the UI never substitutes a poster or unaccepted edit for the decoded frame.
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequest, RecordingEditorSession,
    RecordingSaveRequest, RecordingTimelineThumbnails, SavedRecording,
};
use captures_app::recording_timeline::{TimelineTrimDrag, TimelineTrimEdge, timeline_ratio};
use captures_media::{
    AudioEdit, CancelToken, CropDragHandle, CropRect, CropResizeAxis, EditSpec, ExportEstimate,
    ExportFormat, ExportProgress, ExportSpec, MediaMetadata, MediaToolchain, QualityPreset,
};
use captures_recording::MaxResolution;
use eframe::egui;
use image::RgbaImage;

use crate::tokens::Tokens;

struct Presented {
    source: MediaMetadata,
    edit: EditSpec,
    export: ExportSpec,
    position_ms: u64,
    frame: Arc<RgbaImage>,
}

impl Presented {
    fn from_session(session: &RecordingEditorSession) -> Self {
        let snapshot = session.snapshot();
        Self {
            source: snapshot.source.clone(),
            edit: snapshot.edit.clone(),
            export: snapshot.preview_export.clone(),
            position_ms: snapshot.position_ms,
            frame: session.frame(),
        }
    }
}

enum Job {
    Apply(RecordingEditorRequest),
    Save(RecordingSaveRequest, CancelToken),
    Estimate(CancelToken),
    Thumbnails(CancelToken),
    SourceFrame(CancelToken),
    Play(u64, bool, CancelToken),
    Shutdown,
}

struct PlaybackFrame {
    position_ms: u64,
    pixels: Arc<RgbaImage>,
}

enum PlaybackEnd {
    Paused,
    Ended,
}

enum Event {
    Presented(Result<Presented, String>),
    Progress(ExportProgress),
    Saved(Result<SavedRecording, String>),
    Estimated(Result<ExportEstimate, String>),
    Thumbnails(Result<RecordingTimelineThumbnails, String>),
    SourceFrame(Result<Arc<RgbaImage>, String>),
    PlaybackStarted { audio_enabled: bool },
    PlaybackFinished(Result<PlaybackEnd, String>),
    Destination(Option<PathBuf>),
}

struct TrimGesture {
    edge: TimelineTrimEdge,
    drag: TimelineTrimDrag,
    track: egui::Rect,
}

struct CropGesture {
    initial: CropRect,
    handle: CropDragHandle,
    origin: egui::Pos2,
    image: egui::Rect,
    locked: bool,
}

#[derive(Default)]
struct View {
    presented: Option<Presented>,
    texture: Option<egui::TextureHandle>,
    source_texture: Option<egui::TextureHandle>,
    preview_actual_size: bool,
    adjusting_crop: bool,
    crop_gesture: Option<CropGesture>,
    loading_source: bool,
    thumbnails: Option<egui::TextureHandle>,
    loading_thumbnails: bool,
    thumbnail_error: Option<String>,
    playing: bool,
    preview_loop: Arc<AtomicBool>,
    preview_sound: bool,
    playback_audio_enabled: bool,
    playback_position_ms: Option<u64>,
    playback_ended: bool,
    close_after_playback: bool,
    busy: bool,
    cancel: Option<CancelToken>,
    estimating: bool,
    estimate: Option<ExportEstimate>,
    picker: bool,
    closed: bool,
    confirm_close: bool,
    history_changed: bool,
    saved_edit: EditSpec,
    saved_export: Option<ExportSpec>,
    start_ms: u64,
    end_ms: u64,
    trim_gesture: Option<TrimGesture>,
    crop: Option<CropRect>,
    crop_aspect_unlocked: bool,
    output_size: Option<(u32, u32)>,
    max_resolution: MaxResolution,
    audio: AudioEdit,
    position_ms: u64,
    destination: String,
    gif: bool,
    gif_frames_per_second: Option<u16>,
    quality: QualityPreset,
    progress: Option<ExportProgress>,
    status: Option<String>,
    error: Option<String>,
}

impl View {
    fn export_spec(&self) -> ExportSpec {
        ExportSpec {
            format: if self.gif {
                ExportFormat::Gif
            } else {
                ExportFormat::Mp4
            },
            quality: self.quality,
            max_size_bytes: None,
            frames_per_second: if self.gif {
                self.gif_frames_per_second
            } else {
                None
            },
            gif_max_colors: None,
        }
    }

    fn output_dimensions(&self, source_size: (u32, u32)) -> Option<(u32, u32)> {
        self.output_size.or_else(|| match self.max_resolution {
            MaxResolution::Original => None,
            preset => {
                let (width, height) = self
                    .crop
                    .map_or(source_size, |crop| (crop.width, crop.height));
                Some(preset.constrain(width, height))
            }
        })
    }

    fn staged_edit(&self, p: &Presented) -> EditSpec {
        let output_size = self.output_dimensions((p.source.width, p.source.height));
        EditSpec {
            trim_start_ms: self.start_ms,
            trim_end_ms: (self.end_ms != p.source.duration_ms.unwrap_or(0)).then_some(self.end_ms),
            crop: self.crop,
            output_width: output_size.map(|size| size.0),
            output_height: output_size.map(|size| size.1),
            audio: self.audio.clone(),
        }
    }

    fn unapplied(&self) -> bool {
        self.presented
            .as_ref()
            .is_some_and(|p| self.staged_edit(p) != p.edit || self.export_spec() != p.export)
    }

    fn dirty(&self) -> bool {
        self.unapplied()
            || self.presented.as_ref().is_some_and(|p| {
                p.edit != self.saved_edit || Some(&p.export) != self.saved_export.as_ref()
            })
    }

    fn estimate_label(&self) -> String {
        if self.estimating {
            "Estimating size…".into()
        } else if self.unapplied() {
            "Apply edits to estimate size".into()
        } else if let Some(estimate) = &self.estimate {
            format!(
                "{}{} bytes{}",
                if estimate.exact { "" } else { "≈ " },
                estimate.size_bytes,
                if estimate.exact { " (exact)" } else { "" }
            )
        } else {
            "Size not estimated".into()
        }
    }

    fn request_estimate(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.presented.is_none()
            || self.unapplied()
        {
            return;
        }
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Estimate(cancel));
    }

    fn request_thumbnails(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.closed
            || self.presented.is_none()
            || self.thumbnails.is_some()
        {
            return;
        }
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Thumbnails(cancel));
    }

    fn request_crop_view(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.closed
            || self.presented.is_none()
            || self.crop.is_none()
        {
            return;
        }
        if self.source_texture.is_some() {
            self.adjusting_crop = true;
        } else {
            let cancel = CancelToken::default();
            self.cancel = Some(cancel.clone());
            self.send(tx, Job::SourceFrame(cancel));
        }
    }

    fn request_playback(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.closed
            || self.unapplied()
            || self.adjusting_crop
        {
            return;
        }
        let Some(p) = &self.presented else { return };
        let position = if self.playback_ended {
            p.edit.trim_start_ms
        } else {
            self.playback_position_ms.unwrap_or(p.position_ms)
        };
        self.position_ms = self.playback_position_ms.unwrap_or(p.position_ms);
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Play(position, self.preview_sound, cancel));
    }

    fn pause_playback(&self) {
        if self.playing
            && let Some(cancel) = &self.cancel
        {
            cancel.cancel();
        }
    }

    fn receive_playback_frame(&mut self, ctx: &egui::Context, frame: PlaybackFrame) {
        // Pause freezes the last frame actually presented, not a pending worker
        // frame that happened to race the click. There is only one active job.
        if !self.playing || self.cancel.as_ref().is_none_or(CancelToken::is_cancelled) {
            return;
        }
        self.playback_position_ms = Some(frame.position_ms);
        self.position_ms = frame.position_ms;
        self.set_frame(ctx, &frame.pixels);
    }

    fn set_frame(&mut self, ctx: &egui::Context, pixels: &RgbaImage) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [pixels.width() as usize, pixels.height() as usize],
            pixels.as_raw(),
        );
        if let Some(texture) = &mut self.texture {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.texture = Some(ctx.load_texture(
                "recording-editor-frame",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }

    fn send(&mut self, tx: &Sender<Job>, job: Job) {
        if self.busy || self.picker {
            return;
        }
        self.trim_gesture = None;
        self.crop_gesture = None;
        let estimating = matches!(job, Job::Estimate(_));
        let loading_thumbnails = matches!(job, Job::Thumbnails(_));
        let loading_source = matches!(job, Job::SourceFrame(_));
        let playing = matches!(job, Job::Play(..));
        match tx.send(job) {
            Ok(()) => {
                self.busy = true;
                self.estimating = estimating;
                self.loading_thumbnails = loading_thumbnails;
                self.loading_source = loading_source;
                self.playing = playing;
                if playing {
                    self.playback_ended = false;
                    self.playback_audio_enabled = false;
                }
                if loading_thumbnails {
                    self.thumbnail_error = None;
                }
                if estimating {
                    self.estimate = None;
                }
                self.error = None;
                self.status = loading_source.then(|| "Loading uncropped source frame…".into());
            }
            Err(_) => {
                self.error = Some("Recording editor worker stopped.".into());
                self.cancel = None;
            }
        }
    }

    fn receive(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::Presented(result) => {
                self.busy = false;
                match result {
                    Ok(p) => {
                        self.adjusting_crop = false;
                        if self
                            .presented
                            .as_ref()
                            .is_none_or(|old| old.position_ms != p.position_ms)
                        {
                            self.source_texture = None;
                        }
                        if self
                            .presented
                            .as_ref()
                            .is_none_or(|old| old.edit != p.edit || old.export != p.export)
                        {
                            self.estimate = None;
                        }
                        self.set_frame(ctx, &p.frame);
                        self.playback_position_ms = None;
                        self.playback_ended = false;
                        self.start_ms = p.edit.trim_start_ms;
                        self.end_ms = p
                            .edit
                            .trim_end_ms
                            .unwrap_or(p.source.duration_ms.unwrap_or(0));
                        self.crop = p.edit.crop;
                        // A preset's resolved pixels are not a custom size:
                        // retain the preset so later crop changes recompute it.
                        if self.presented.is_none() || self.output_size.is_some() {
                            self.output_size = p.edit.output_width.zip(p.edit.output_height);
                        }
                        self.audio = p.edit.audio.clone();
                        self.position_ms = p.position_ms;
                        self.gif = p.export.format == ExportFormat::Gif;
                        if self.gif {
                            self.gif_frames_per_second = p.export.frames_per_second;
                        }
                        self.quality = p.export.quality;
                        // The initial edit includes trusted audio flags.
                        if self.presented.is_none() {
                            self.saved_edit = p.edit.clone();
                            self.saved_export = Some(p.export.clone());
                        }
                        self.presented = Some(p);
                    }
                    Err(error) => {
                        // A failed command restores the accepted still, rather
                        // than labelling transient playback pixels as accepted.
                        if let Some(p) = &self.presented {
                            self.position_ms = p.position_ms;
                            let frame = p.frame.clone();
                            self.set_frame(ctx, &frame);
                        }
                        self.playback_position_ms = None;
                        self.playback_ended = false;
                        self.error = Some(error);
                    }
                }
            }
            Event::Progress(progress) => self.progress = Some(progress),
            Event::Saved(result) => {
                self.busy = false;
                self.cancel = None;
                self.progress = None;
                match result {
                    Ok(saved) => {
                        let (path, warning) = match saved {
                            SavedRecording::Saved { path, .. } => {
                                self.history_changed = true;
                                (path, None)
                            }
                            SavedRecording::SavedWithoutHistory { path, warning } => {
                                (path, Some(warning))
                            }
                        };
                        if let Some(p) = &self.presented {
                            self.saved_edit = p.edit.clone();
                            self.saved_export = Some(p.export.clone());
                        }
                        self.status = Some(format!("Saved new copy: {}", path.display()));
                        self.error = warning;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Estimated(result) => {
                self.busy = false;
                self.estimating = false;
                self.cancel = None;
                match result {
                    Ok(estimate) => {
                        self.estimate = Some(estimate);
                        self.error = None;
                    }
                    Err(error) => {
                        self.estimate = None;
                        self.error = Some(error);
                    }
                }
            }
            Event::Thumbnails(result) => {
                self.busy = false;
                self.loading_thumbnails = false;
                self.cancel = None;
                self.error = None;
                match result {
                    Ok(thumbnails) => {
                        let pixels = thumbnails.pixels();
                        self.thumbnails = Some(ctx.load_texture(
                            "recording-source-thumbnails",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [pixels.width() as usize, pixels.height() as usize],
                                pixels.as_raw(),
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                        self.thumbnail_error = None;
                    }
                    Err(error) => self.thumbnail_error = Some(error),
                }
            }
            Event::SourceFrame(result) => {
                self.busy = false;
                self.loading_source = false;
                self.status = None;
                let result = if self.cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
                    Err("Source preview cancelled.".into())
                } else {
                    result
                };
                self.cancel = None;
                match result {
                    Ok(pixels) => {
                        self.source_texture = Some(ctx.load_texture(
                            "recording-crop-source",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [pixels.width() as usize, pixels.height() as usize],
                                pixels.as_raw(),
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                        self.adjusting_crop = true;
                        self.error = None;
                    }
                    Err(error) => {
                        self.adjusting_crop = false;
                        self.error = Some(format!("Source preview failed: {error}"));
                    }
                }
            }
            Event::PlaybackStarted { audio_enabled } => {
                if self.playing
                    && self
                        .cancel
                        .as_ref()
                        .is_some_and(|cancel| !cancel.is_cancelled())
                {
                    self.playback_audio_enabled = audio_enabled;
                    self.status = Some(
                        if audio_enabled {
                            "Playing accepted audio on the default output device."
                        } else if self.preview_sound {
                            "Playing silently: the accepted format or audio mix has no sound."
                        } else {
                            "Playing silently."
                        }
                        .into(),
                    );
                }
            }
            Event::PlaybackFinished(result) => {
                self.busy = false;
                self.playing = false;
                self.cancel = None;
                match result {
                    Ok(end) => {
                        self.error = None;
                        self.playback_ended = matches!(end, PlaybackEnd::Ended);
                        self.status = Some(
                            if self.playback_ended {
                                "Playback ended. Play restarts the accepted trim."
                            } else {
                                "Playback paused."
                            }
                            .into(),
                        );
                    }
                    Err(error) => {
                        self.playback_position_ms = None;
                        self.playback_ended = false;
                        if let Some(p) = &self.presented {
                            self.position_ms = p.position_ms;
                            let frame = p.frame.clone();
                            self.set_frame(ctx, &frame);
                        }
                        self.status = None;
                        self.error = Some(format!("Playback failed: {error}"));
                    }
                }
                self.playback_audio_enabled = false;
                if std::mem::take(&mut self.close_after_playback) {
                    self.request_close();
                }
            }
            Event::Destination(path) => {
                self.picker = false;
                if let Some(path) = path {
                    self.destination = path.to_string_lossy().into_owned();
                }
            }
        }
    }

    fn request_close(&mut self) {
        self.trim_gesture = None;
        self.crop_gesture = None;
        if self.playing {
            self.close_after_playback = true;
            self.pause_playback();
        } else if self.busy || self.picker {
            self.error =
                Some("Wait for the current operation, or cancel it, before closing.".into());
        } else if self.dirty() {
            self.confirm_close = true;
        } else {
            self.closed = true;
        }
    }

    fn title(&self) -> &'static str {
        if self.busy {
            "Recording editor — Working…"
        } else {
            "Recording editor"
        }
    }
}

pub struct Editor {
    viewport: egui::ViewportId,
    view: Arc<Mutex<View>>,
    // One latest frame, not an unbounded event queue or one closure per frame.
    playback_frame: Arc<Mutex<Option<PlaybackFrame>>>,
    tx: Sender<Job>,
    events: Sender<Event>,
    rx: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
}

fn wake(ctx: &egui::Context, viewport: egui::ViewportId) {
    // Live drains worker results in the root, including while it is hidden.
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.request_repaint_of(viewport);
}

impl Editor {
    pub fn open(
        ctx: &egui::Context,
        history_root: PathBuf,
        artifact_id: String,
        directory: PathBuf,
    ) -> Self {
        let viewport = egui::ViewportId::from_hash_of(("recording-editor", &artifact_id));
        let (tx, jobs) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let out = events.clone();
        let wake_ctx = ctx.clone();
        let playback_frame = Arc::new(Mutex::new(None));
        let latest_frame = playback_frame.clone();
        let preview_loop = Arc::<AtomicBool>::default();
        let loop_enabled = preview_loop.clone();
        let worker = thread::spawn(move || {
            let mut session = match RecordingEditorSession::open(
                RecordingEditorOpenRequest {
                    history_root,
                    artifact_id,
                },
                MediaToolchain::from_command_names(),
            ) {
                Ok(session) => {
                    let _ = out.send(Event::Presented(Ok(Presented::from_session(&session))));
                    Some(session)
                }
                Err(error) => {
                    let _ = out.send(Event::Presented(Err(error)));
                    None
                }
            };
            wake(&wake_ctx, viewport);
            while let Ok(job) = jobs.recv() {
                let event = match job {
                    Job::Shutdown => break,
                    Job::Apply(request) => Event::Presented(
                        session
                            .as_mut()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| {
                                s.execute(request)?;
                                Ok(Presented::from_session(s))
                            }),
                    ),
                    Job::Save(request, cancel) => Event::Saved(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| {
                                s.save_new(request, &cancel, |progress| {
                                    let _ = out.send(Event::Progress(progress));
                                    wake(&wake_ctx, viewport);
                                })
                            }),
                    ),
                    Job::Estimate(cancel) => Event::Estimated(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| s.estimate_export(&cancel)),
                    ),
                    Job::Thumbnails(cancel) => Event::Thumbnails(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| s.timeline_thumbnails(&cancel)),
                    ),
                    Job::SourceFrame(cancel) => Event::SourceFrame(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| s.source_frame(&cancel)),
                    ),
                    Job::Play(position, sound, cancel) => {
                        let result = (|| {
                            let session =
                                session.as_ref().ok_or("Recording editor is unavailable.")?;
                            let mut position = position;
                            let mut started = false;
                            loop {
                                let mut playback = if sound {
                                    session.playback_with_audio(position, &cancel)?
                                } else {
                                    session.playback(position, &cancel)?
                                };
                                if !started {
                                    let _ = out.send(Event::PlaybackStarted {
                                        audio_enabled: playback.audio_enabled(),
                                    });
                                    wake(&wake_ctx, viewport);
                                    started = true;
                                }
                                let mut decoded_frame = false;
                                while let Some(frame) = playback.next_frame()? {
                                    if cancel.is_cancelled() {
                                        break;
                                    }
                                    decoded_frame = true;
                                    let needs_wake = {
                                        let mut latest = latest_frame.lock().unwrap();
                                        let empty = latest.is_none();
                                        *latest = Some(PlaybackFrame {
                                            position_ms: frame.position_ms,
                                            pixels: frame.pixels(),
                                        });
                                        empty
                                    };
                                    if needs_wake {
                                        wake(&wake_ctx, viewport);
                                    }
                                }
                                // Never restart an empty stream or a cancelled/failed
                                // decoder. Drop finishes teardown before the next lap.
                                if cancel.is_cancelled()
                                    || !decoded_frame
                                    || !loop_enabled.load(Ordering::Relaxed)
                                {
                                    break;
                                }
                                position = session.snapshot().edit.trim_start_ms;
                            }
                            Ok(PlaybackEnd::Ended)
                        })();
                        // Cancellation is expected on Pause/close/focus loss.
                        Event::PlaybackFinished(if cancel.is_cancelled() {
                            Ok(PlaybackEnd::Paused)
                        } else {
                            result
                        })
                    }
                };
                if out.send(event).is_err() {
                    break;
                }
                wake(&wake_ctx, viewport);
            }
        });
        Self {
            viewport,
            view: Arc::new(Mutex::new(View {
                busy: true,
                preview_loop,
                destination: directory
                    .join(format!(
                        "Captures_{}_edited.mp4",
                        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
                    ))
                    .to_string_lossy()
                    .into_owned(),
                ..View::default()
            })),
            playback_frame,
            tx,
            events,
            rx,
            worker: Some(worker),
        }
    }

    pub fn focus(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(self.viewport, egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(self.viewport, egui::ViewportCommand::Focus);
        wake(ctx, self.viewport);
    }

    pub fn closed(&self) -> bool {
        self.view.lock().unwrap().closed
    }
    pub fn take_history_changed(&self) -> bool {
        std::mem::take(&mut self.view.lock().unwrap().history_changed)
    }

    pub fn receive(&self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            let mut view = self.view.lock().unwrap();
            // Drain the final frame before EOF; completion never overtakes it.
            if let Some(frame) = self.playback_frame.lock().unwrap().take() {
                view.receive_playback_frame(ctx, frame);
            }
            let opening = view.presented.is_none();
            view.receive(ctx, event);
            if opening && view.presented.is_some() {
                view.request_thumbnails(&self.tx);
            }
            wake(ctx, self.viewport);
        }
        if let Some(frame) = self.playback_frame.lock().unwrap().take() {
            self.view.lock().unwrap().receive_playback_frame(ctx, frame);
            wake(ctx, self.viewport);
        }
    }

    pub fn flush(&self, ctx: &egui::Context) -> Result<(), String> {
        self.receive(ctx);
        let mut view = self.view.lock().unwrap();
        view.pause_playback();
        if !view.closed && (view.busy || view.picker || view.dirty()) {
            let error = "Recording edits are not saved as drafts. Save a new copy or close the recording editor before quitting.".to_owned();
            view.error = Some(error.clone());
            drop(view);
            self.focus(ctx);
            return Err(error);
        }
        Ok(())
    }

    pub fn show(&self, ctx: &egui::Context, tokens: &Tokens) {
        if self.closed() {
            return;
        }
        let state = self.view.clone();
        let tx = self.tx.clone();
        let events = self.events.clone();
        let tokens = tokens.clone();
        let viewport = self.viewport;
        let title = self.view.lock().unwrap().title();
        ctx.show_viewport_deferred(
            viewport,
            egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([960., 760.])
                .with_min_inner_size([760., 580.]),
            move |ui, _| {
                let mut view = state.lock().unwrap();
                if ui.input(|i| !i.focused || i.viewport().minimized == Some(true)) {
                    view.pause_playback();
                }
                if ui.input(|i| i.viewport().close_requested()) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    view.request_close();
                }
                ui.push_id(viewport, |ui| {
                    show(ui, &tokens, &mut view, &tx, &events, viewport)
                });
                if ui.input(|i| i.viewport().title.as_deref() != Some(view.title())) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title(view.title().into()));
                }
                if view.closed {
                    wake(ui.ctx(), viewport);
                }
            },
        );
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        if let Some(cancel) = &self.view.lock().unwrap().cancel {
            cancel.cancel();
        }
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn show_crop_overlay(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, image: egui::Rect) {
    let Some(initial) = view.crop else {
        view.crop_gesture = None;
        return;
    };
    let Some(presented) = &view.presented else {
        return;
    };
    let (width, height) = (presented.source.width, presented.source.height);
    if initial
        .after_drag(width, height, CropDragHandle::Move, 0., 0., false)
        .is_none()
    {
        view.crop_gesture = None;
        return;
    }
    let to_view = |crop: CropRect| {
        egui::Rect::from_min_max(
            image.min
                + egui::vec2(
                    crop.x as f32 / width as f32 * image.width(),
                    crop.y as f32 / height as f32 * image.height(),
                ),
            image.min
                + egui::vec2(
                    (f64::from(crop.x) + f64::from(crop.width)) as f32 / width as f32
                        * image.width(),
                    (f64::from(crop.y) + f64::from(crop.height)) as f32 / height as f32
                        * image.height(),
                ),
        )
    };
    let handle_positions = |crop: CropRect| {
        let rect = to_view(crop);
        [
            (CropDragHandle::NorthWest, rect.left_top(), "Crop top left"),
            (
                CropDragHandle::NorthEast,
                rect.right_top(),
                "Crop top right",
            ),
            (
                CropDragHandle::SouthEast,
                rect.right_bottom(),
                "Crop bottom right",
            ),
            (
                CropDragHandle::SouthWest,
                rect.left_bottom(),
                "Crop bottom left",
            ),
            (CropDragHandle::North, rect.center_top(), "Crop top"),
            (CropDragHandle::East, rect.right_center(), "Crop right"),
            (CropDragHandle::South, rect.center_bottom(), "Crop bottom"),
            (CropDragHandle::West, rect.left_center(), "Crop left"),
        ]
    };
    let enabled = ui.is_enabled()
        && !view.busy
        && !view.picker
        && !view.confirm_close
        && ui.input(|input| input.focused)
        && !egui::Popup::is_any_open(ui.ctx());
    if !enabled
        || view
            .crop_gesture
            .as_ref()
            .is_some_and(|drag| drag.image != image)
    {
        view.crop_gesture = None;
    }
    let sense = if enabled {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let pending_text = ui
        .ctx()
        .memory(|memory| memory.focused())
        .filter(|id| egui::TextEdit::load_state(ui.ctx(), *id).is_some());
    let mut responses = vec![(
        CropDragHandle::Move,
        ui.interact(to_view(initial), ui.scope_id().with("Move crop"), sense)
            .on_hover_cursor(egui::CursorIcon::Move),
    )];
    responses[0]
        .1
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "Move crop"));
    let hit_size = tokens.number("s-5");
    for (handle, center, label) in handle_positions(initial) {
        let response = ui
            .interact(
                egui::Rect::from_center_size(center, egui::Vec2::splat(hit_size)),
                ui.scope_id().with(label),
                sense,
            )
            .on_hover_text(format!(
                "{label}: drag or use arrows; Shift moves 10 source pixels."
            ));
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
        responses.push((handle, response));
    }
    if enabled && ui.ctx().current_pass_index() == 0 {
        for event in ui.input(|input| input.events.clone()) {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => {
                    view.crop_gesture = None;
                    if !ui.clip_rect().contains(pos) {
                        continue;
                    }
                    let handle = handle_positions(view.crop.unwrap())
                        .into_iter()
                        .find(|(_, center, _)| {
                            egui::Rect::from_center_size(*center, egui::Vec2::splat(hit_size))
                                .contains(pos)
                        })
                        .map(|(handle, _, _)| handle)
                        .or_else(|| {
                            to_view(view.crop.unwrap())
                                .contains(pos)
                                .then_some(CropDragHandle::Move)
                        });
                    if let Some(handle) = handle {
                        if let Some(id) = pending_text {
                            // Numeric fields commit later in this UI pass. Let
                            // that commit finish before capturing drag geometry.
                            ui.memory_mut(|memory| memory.surrender_focus(id));
                            continue;
                        }
                        if let Some((_, response)) =
                            responses.iter().find(|(candidate, _)| *candidate == handle)
                        {
                            response.request_focus();
                        }
                        view.crop_gesture = Some(CropGesture {
                            initial: view.crop.unwrap(),
                            handle,
                            origin: pos,
                            image,
                            locked: !view.crop_aspect_unlocked,
                        });
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    if let Some(drag) = &view.crop_gesture {
                        let delta = pos - drag.origin;
                        if let Some(crop) = drag.initial.after_drag(
                            width,
                            height,
                            drag.handle,
                            f64::from(delta.x / image.width()) * f64::from(width),
                            f64::from(delta.y / image.height()) * f64::from(height),
                            drag.locked,
                        ) {
                            view.crop = Some(crop);
                        }
                    }
                }
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                }
                | egui::Event::PointerGone
                | egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } => view.crop_gesture = None,
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.ctrl && !modifiers.alt && !modifiers.command => {
                    let step = if modifiers.shift { 10. } else { 1. };
                    let delta = match key {
                        egui::Key::ArrowLeft => (-step, 0.),
                        egui::Key::ArrowRight => (step, 0.),
                        egui::Key::ArrowUp => (0., -step),
                        egui::Key::ArrowDown => (0., step),
                        _ => continue,
                    };
                    if let Some((handle, _)) =
                        responses.iter().find(|(_, response)| response.has_focus())
                    {
                        ui.input_mut(|input| input.consume_key(modifiers, key));
                        view.crop_gesture = None;
                        if let Some(crop) = view.crop.unwrap().after_drag(
                            width,
                            height,
                            *handle,
                            delta.0,
                            delta.1,
                            !view.crop_aspect_unlocked,
                        ) {
                            view.crop = Some(crop);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let selected = to_view(view.crop.unwrap()).intersect(image);
    for dim in [
        egui::Rect::from_min_max(image.min, egui::pos2(image.right(), selected.top())),
        egui::Rect::from_min_max(egui::pos2(image.left(), selected.bottom()), image.max),
        egui::Rect::from_min_max(
            egui::pos2(image.left(), selected.top()),
            selected.left_bottom(),
        ),
        egui::Rect::from_min_max(
            selected.right_top(),
            egui::pos2(image.right(), selected.bottom()),
        ),
    ] {
        ui.painter()
            .rect_filled(dim, 0., tokens.color("surface-sunken").gamma_multiply(0.75));
    }
    ui.painter().rect_stroke(
        selected,
        0.,
        egui::Stroke::new(tokens.number("s-1"), tokens.color("theme-accent")),
        egui::StrokeKind::Inside,
    );
    for (_, center, _) in handle_positions(view.crop.unwrap()) {
        let handle = egui::Rect::from_center_size(center, egui::Vec2::splat(tokens.number("s-4")));
        ui.painter()
            .rect_filled(handle, tokens.number("r-xs"), tokens.color("theme-accent"));
        ui.painter().rect_stroke(
            handle,
            tokens.number("r-xs"),
            egui::Stroke::new(tokens.number("s-1"), tokens.color("surface-raised")),
            egui::StrokeKind::Inside,
        );
    }
}

fn show_trim_timeline(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    duration: u64,
    rect: egui::Rect,
) {
    let grip_width = tokens.number("s-6");
    let track = rect.shrink2(egui::vec2(grip_width, 0.));
    if track.width() <= 0. || duration == 0 {
        view.trim_gesture = None;
        return;
    }
    let handles = |start: u64, end: u64| {
        let start = track.left()
            + timeline_ratio(start as f64, duration as f64).unwrap_or(0.) as f32 * track.width();
        let end = track.left()
            + timeline_ratio(end as f64, duration as f64).unwrap_or(1.) as f32 * track.width();
        // Grips sit outside the selected interval, so even a 1 ms selection
        // leaves distinct start/end hit regions. Both remain inside the row.
        [
            egui::Rect::from_min_max(
                egui::pos2(start - grip_width, rect.top()),
                egui::pos2(start, rect.bottom()),
            ),
            egui::Rect::from_min_max(
                egui::pos2(end, rect.top()),
                egui::pos2(end + grip_width, rect.bottom()),
            ),
        ]
    };
    let enabled = ui.is_enabled()
        && !view.busy
        && !view.picker
        && !view.confirm_close
        && view.start_ms < view.end_ms
        && view.end_ms <= duration
        && ui.input(|input| input.focused)
        && !egui::Popup::is_any_open(ui.ctx());
    let handle_rects = handles(view.start_ms, view.end_ms);
    let responses = ["Trim start", "Trim end"].map(|label| {
        let index = usize::from(label == "Trim end");
        ui.interact(handle_rects[index], ui.scope_id().with(label),
                    if enabled { egui::Sense::click_and_drag() } else { egui::Sense::hover() })
            .on_hover_text(format!("{label}: {} ms. Drag or use arrow keys/Page Up/Page Down. Apply edits to update the preview.",
                                   if index == 0 { view.start_ms } else { view.end_ms }))
    });
    if !enabled
        || view
            .trim_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.track != track)
    {
        // Match shipping pointer cancellation: keep the last staged values.
        view.trim_gesture = None;
    }
    if enabled && ui.ctx().current_pass_index() == 0 {
        for event in ui.input(|input| input.events.clone()) {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => {
                    view.trim_gesture = None;
                    if !ui.clip_rect().contains(pos) {
                        continue;
                    }
                    if let Some(index) = handles(view.start_ms, view.end_ms)
                        .iter()
                        .position(|rect| rect.contains(pos))
                    {
                        let edge = if index == 0 {
                            TimelineTrimEdge::Start
                        } else {
                            TimelineTrimEdge::End
                        };
                        if let Some(drag) = TimelineTrimDrag::begin(
                            edge,
                            f64::from(pos.x),
                            view.start_ms as f64,
                            view.end_ms as f64,
                            duration as f64,
                        ) {
                            responses[index].request_focus();
                            view.trim_gesture = Some(TrimGesture { edge, drag, track });
                        }
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    if let Some(gesture) = &mut view.trim_gesture
                        && let Some(update) = gesture.drag.update(
                            f64::from(pos.x),
                            f64::from(track.left()),
                            f64::from(track.width()),
                        )
                    {
                        gesture.drag = update.drag;
                        let value = update.time_ms.round() as u64;
                        match gesture.edge {
                            TimelineTrimEdge::Start => view.start_ms = value,
                            TimelineTrimEdge::End => view.end_ms = value,
                        }
                    }
                }
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                }
                | egui::Event::PointerGone => view.trim_gesture = None,
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } => {
                    view.trim_gesture = None;
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers == egui::Modifiers::NONE => {
                    let step = if duration < 60_000 { 1. } else { 10. };
                    let delta = match key {
                        egui::Key::ArrowLeft | egui::Key::ArrowDown => -step,
                        egui::Key::ArrowRight | egui::Key::ArrowUp => step,
                        egui::Key::PageDown => -1000.,
                        egui::Key::PageUp => 1000.,
                        _ => continue,
                    };
                    if let Some(index) = responses.iter().position(|response| response.has_focus())
                    {
                        ui.input_mut(|input| input.consume_key(modifiers, key));
                        view.trim_gesture = None;
                        if index == 0 {
                            view.start_ms = (view.start_ms as f64 + delta)
                                .clamp(0., (view.end_ms - 1) as f64)
                                as u64;
                        } else {
                            view.end_ms = (view.end_ms as f64 + delta)
                                .clamp((view.start_ms + 1) as f64, duration as f64)
                                as u64;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let [start, end] = handles(view.start_ms, view.end_ms);
    ui.painter()
        .rect_filled(track, 0., tokens.color("surface-sunken"));
    if let Some(texture) = &view.thumbnails {
        // Center-crop the strip vertically to the compact track, preserving
        // thumbnail aspect and the full horizontal source-time mapping.
        let size = texture.size_vec2();
        let uv_height = (track.height() * size.x / (track.width() * size.y)).min(1.);
        ui.painter().image(
            texture.id(),
            track,
            egui::Rect::from_min_max(
                egui::pos2(0., (1. - uv_height) / 2.),
                egui::pos2(1., (1. + uv_height) / 2.),
            ),
            egui::Color32::WHITE,
        );
        for excluded in [
            egui::Rect::from_min_max(track.min, egui::pos2(start.right(), track.bottom())),
            egui::Rect::from_min_max(egui::pos2(end.left(), track.top()), track.max),
        ] {
            ui.painter().rect_filled(
                excluded,
                0.,
                tokens.color("surface-sunken").gamma_multiply(0.7),
            );
        }
    }
    if start.right() <= end.left() {
        let selected = egui::Rect::from_min_max(
            egui::pos2(start.right(), track.top()),
            egui::pos2(end.left(), track.bottom()),
        );
        if view.thumbnails.is_none() {
            ui.painter()
                .rect_filled(selected, 0., tokens.color("surface-selected"));
        }
        ui.painter().rect_stroke(
            selected,
            0.,
            egui::Stroke::new(1., tokens.color("theme-accent")),
            egui::StrokeKind::Inside,
        );
    }
    if view.loading_thumbnails {
        // This status remains readable while the enclosing editing controls
        // are disabled for the serialized thumbnail operation.
        ui.ctx()
            .layer_painter(ui.layer_id())
            .with_clip_rect(ui.clip_rect())
            .text(
                track.center(),
                egui::Align2::CENTER_CENTER,
                "Loading source thumbnails…",
                egui::FontId::proportional(tokens.number("text-sm")),
                tokens.color("text-muted"),
            );
    }
    for (index, handle) in [start, end].into_iter().enumerate() {
        let response = &responses[index];
        if enabled && response.has_focus() {
            // Arrow keys adjust this slider instead of moving egui focus to a
            // neighboring widget before the following key event arrives.
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Slider,
                enabled,
                format!(
                    "Trim {}: {} ms",
                    if index == 0 { "start" } else { "end" },
                    if index == 0 {
                        view.start_ms
                    } else {
                        view.end_ms
                    }
                ),
            )
        });
        ui.painter()
            .rect_filled(handle, tokens.number("r-sm"), tokens.color("control"));
        ui.painter().rect_stroke(
            handle,
            tokens.number("r-sm"),
            egui::Stroke::new(
                1.,
                tokens.color(if enabled && (response.hovered() || response.has_focus()) {
                    "theme-accent"
                } else {
                    "control-border"
                }),
            ),
            egui::StrokeKind::Inside,
        );
        ui.painter().vline(
            handle.center().x,
            handle.y_range().shrink(tokens.number("s-2")),
            egui::Stroke::new(
                1.,
                tokens.color(if enabled { "text" } else { "text-muted" }),
            ),
        );
        if enabled && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }
}

fn show(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    events: &Sender<Event>,
    viewport: egui::ViewportId,
) {
    egui::Panel::bottom("recording-save").show(ui, |ui| {
        if let Some(error) = &view.error {
            ui.colored_label(tokens.color("danger-text"), error);
        }
        if let Some(status) = &view.status {
            ui.label(status);
        }
        if view.confirm_close {
            ui.label("Discard unsaved recording edits? The original recording is unchanged.");
            ui.horizontal(|ui| {
                if ui.button("Keep editing").clicked() {
                    view.confirm_close = false;
                }
                if ui.button("Discard edits and close").clicked() {
                    view.closed = true;
                }
            });
        }
        if let Some(progress) = &view.progress {
            ui.add(
                egui::ProgressBar::new(f32::from(progress.completed_per_mille) / 1000.).text(
                    progress
                        .message
                        .clone()
                        .unwrap_or_else(|| format!("{:?}", progress.stage)),
                ),
            );
        }
        if let Some(cancel) = &view.cancel
            && ui
                .add_enabled(!cancel.is_cancelled(), egui::Button::new(if view.playing { "Pause playback" } else if view.loading_source { "Cancel source preview" } else if view.loading_thumbnails { "Cancel thumbnails" } else if view.estimating { "Cancel estimate" } else { "Cancel export" }))
                .clicked()
        {
            cancel.cancel();
        }
        if let Some(error) = view.thumbnail_error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Source thumbnails unavailable.");
                if ui.add_enabled(!view.busy && !view.picker && !view.confirm_close,
                                  egui::Button::new("Retry thumbnails"))
                    .on_hover_text(error).clicked()
                {
                    view.request_thumbnails(tx);
                }
            });
        }
        ui.add_enabled_ui(!view.busy && !view.picker && !view.confirm_close, |ui| {
            ui.horizontal(|ui| {
                ui.label("Destination");
                ui.add(
                    egui::TextEdit::singleline(&mut view.destination)
                        .desired_width((ui.available_width() - 100.).max(100.)),
                );
                if ui.button("Change…").clicked() {
                    view.picker = true;
                    let events = events.clone();
                    let ctx = ui.ctx().clone();
                    let path = PathBuf::from(&view.destination);
                    thread::spawn(move || {
                        let mut dialog =
                            rfd::FileDialog::new().set_title("Save recording as new file");
                        if let Some(parent) = path.parent() {
                            dialog = dialog.set_directory(parent);
                        }
                        if let Some(name) = path.file_name() {
                            dialog = dialog.set_file_name(name.to_string_lossy());
                        }
                        let _ = events.send(Event::Destination(dialog.save_file()));
                        wake(&ctx, viewport);
                    });
                }
            });
            ui.horizontal(|ui| {
                let old = view.gif;
                ui.selectable_value(&mut view.gif, false, "MP4");
                ui.selectable_value(&mut view.gif, true, "GIF");
                if old != view.gif {
                    view.destination = PathBuf::from(&view.destination)
                        .with_extension(if view.gif { "gif" } else { "mp4" })
                        .to_string_lossy()
                        .into_owned();
                }
                ui.label(view.estimate_label())
                    .on_hover_text("File size for the accepted settings. Longer recordings use encoded samples and are approximate. Estimating creates no History entry or saved file.");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            view.presented.is_some() && !view.unapplied(),
                            egui::Button::new("Save new copy"),
                        )
                        .on_hover_text("Creates a separate copy. The original and existing files are never replaced.")
                        .clicked()
                    {
                        let cancel = CancelToken::default();
                        view.cancel = Some(cancel.clone());
                        view.send(
                            tx,
                            Job::Save(
                                RecordingSaveRequest {
                                    destination: view.destination.clone().into(),
                                    export: view.presented.as_ref().unwrap().export.clone(),
                                },
                                cancel,
                            ),
                        );
                    }
                    if ui
                        .add_enabled(view.unapplied(), egui::Button::new("Apply edits"))
                        .on_hover_text("Update the preview before scrubbing or saving")
                        .clicked()
                    {
                        let edit = view.staged_edit(view.presented.as_ref().unwrap());
                        let export = view.export_spec();
                        view.send(
                            tx,
                            Job::Apply(RecordingEditorRequest::UpdatePreview { edit, export }),
                        );
                    }
                    if ui.add_enabled(view.presented.is_some() && !view.unapplied(), egui::Button::new("Estimate size")).clicked() {
                        view.request_estimate(tx);
                    }
                });
            });
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Edit recording");
            let previous_actual_size = view.preview_actual_size;
            ui.horizontal(|ui| {
                ui.strong("Preview");
                ui.weak(if view.adjusting_crop { "Source crop" } else if view.playback_audio_enabled { "Audio playback" } else if view.preview_sound && !view.playing { "Sound selected" } else { "Silent playback" });
                if view.playing {
                    let pausing = view.cancel.as_ref().is_some_and(CancelToken::is_cancelled);
                    if ui.add_enabled(!pausing, egui::Button::new(if pausing { "Pausing…" } else { "Pause" }).small()).clicked() {
                        view.pause_playback();
                        ui.ctx().request_repaint();
                    }
                } else if ui.add_enabled(!view.busy && !view.picker && !view.confirm_close
                    && !view.adjusting_crop && view.presented.is_some() && !view.unapplied(), egui::Button::new("Play").small())
                    .on_hover_text("Play the accepted trim and mix; Sound is off by default. Apply staged edits first. Motion preview fits within 1280 × 720.")
                    .clicked()
                {
                    view.request_playback(tx);
                    ui.ctx().request_repaint();
                }
                let looping = view.preview_loop.load(Ordering::Relaxed);
                if ui.add_enabled(view.presented.is_some() && (!view.busy || view.playing)
                    && !view.picker && !view.confirm_close
                    && view.cancel.as_ref().is_none_or(|cancel| !cancel.is_cancelled()),
                    egui::Button::new("Loop preview").selected(looping).small())
                    .on_hover_text("Repeat the accepted trim until paused. This changes only playback, not the saved recording.")
                    .clicked()
                {
                    view.preview_loop.store(!looping, Ordering::Relaxed);
                }
                if view.adjusting_crop && ui.add_enabled(!view.busy && !view.picker && !view.confirm_close,
                    egui::Button::new("Done cropping").small()).clicked()
                {
                    view.adjusting_crop = false;
                    view.crop_gesture = None;
                }
                if ui.add_enabled(view.presented.is_some() && !view.busy && !view.picker && !view.confirm_close,
                    egui::Button::new("Sound").selected(view.preview_sound).small())
                    .on_hover_text("Preview accepted MP4 audio on the default output device. Change only while stopped. If the device fails, turn Sound off and retry. This never changes the export.")
                    .clicked()
                {
                    view.preview_sound = !view.preview_sound;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("100%").small().selected(view.preview_actual_size))
                        .on_hover_text("One decoded image pixel per screen point. Scroll to see overflow; playback may use a reduced-size frame.")
                        .clicked()
                    {
                        view.preview_actual_size = true;
                    }
                    if ui.add(egui::Button::new("Fit").small().selected(!view.preview_actual_size)).clicked() {
                        view.preview_actual_size = false;
                    }
                });
            });
            let scale_changed = previous_actual_size != view.preview_actual_size;
            if scale_changed { view.crop_gesture = None; }
            let width = ui.available_width();
            let height = (ui.available_height() - 210.).clamp(140., 380.);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, tokens.number("r-md"), tokens.color("surface-sunken"));
            let preview_texture = if view.adjusting_crop { view.source_texture.clone() } else { view.texture.clone() };
            if let Some(texture) = preview_texture {
                let size = texture.size_vec2();
                let actual_size = view.preview_actual_size;
                let margin = if view.adjusting_crop { tokens.number("s-3") } else { 0. };
                let source_mode = view.adjusting_crop;
                let mut viewport = ui.new_child(egui::UiBuilder::new()
                    .id_salt("recording-preview-viewport").max_rect(rect));
                viewport.shrink_clip_rect(rect);
                let mut paint = |ui: &mut egui::Ui, image_rect: egui::Rect| {
                    ui.painter().image(texture.id(), image_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        egui::Color32::WHITE);
                    if source_mode { show_crop_overlay(ui, tokens, view, image_rect); }
                    else { view.crop_gesture = None; }
                };
                if actual_size {
                    let mut scroll = egui::ScrollArea::both()
                        .id_salt(("recording-preview-scroll", source_mode, texture.size()))
                        .max_width(rect.width()).max_height(rect.height())
                        .auto_shrink([false, false])
                        .scroll_source(egui::scroll_area::ScrollSource {
                            drag: egui::scroll_area::DragScroll::Never,
                            ..Default::default()
                        });
                    if scale_changed { scroll = scroll.scroll_offset(egui::Vec2::ZERO); }
                    scroll.show(&mut viewport, |ui| {
                        let extent = (size + egui::Vec2::splat(margin * 2.)).max(ui.available_size());
                        let (content, _) = ui.allocate_exact_size(extent, egui::Sense::hover());
                        paint(ui, egui::Rect::from_center_size(content.center(), size));
                    });
                } else {
                    let bounds = rect.shrink(margin);
                    let scale = (bounds.width() / size.x).min(bounds.height() / size.y);
                    paint(&mut viewport, egui::Rect::from_center_size(bounds.center(), size * scale));
                }
            } else {
                ui.label(if view.busy {
                    "Decoding recording…"
                } else {
                    "Preview unavailable"
                });
            }
            let Some(p) = &view.presented else {
                return;
            };
            let duration = p.source.duration_ms.unwrap_or(0);
            let displayed_position = if view.adjusting_crop { p.position_ms } else {
                view.playback_position_ms.unwrap_or(p.position_ms)
            };
            let source_size = (p.source.width, p.source.height);
            let system_audio = p.edit.audio.source_has_system_audio;
            let microphone_audio = p.edit.audio.source_has_microphone_audio;
            if view.adjusting_crop {
                ui.label(format!("Uncropped source: {:.3}s · {} × {} · Drag to stage crop; Apply edits to preview output.",
                    displayed_position as f64 / 1000., source_size.0, source_size.1));
            } else {
            ui.label(format!(
                "Source frame: {:.3}s / {:.3}s · {} × {} · {:?} {:?} preview: {} × {}",
                displayed_position as f64 / 1000.,
                duration as f64 / 1000.,
                p.source.width,
                p.source.height,
                p.export.format,
                p.export.quality,
                view.texture.as_ref().map_or(p.frame.width() as usize, |t| t.size()[0]),
                view.texture.as_ref().map_or(p.frame.height() as usize, |t| t.size()[1])
            ));
            }
            ui.add_enabled_ui(!view.busy && !view.picker && !view.confirm_close, |ui| {
                ui.add_enabled_ui(!view.unapplied(), |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::Slider::new(
                                &mut view.position_ms,
                                0..=duration.saturating_sub(1),
                            )
                            .show_value(false),
                        );
                        // Numeric entry must not send its first digit to the
                        // worker and steal focus before the rest can be typed.
                        ui.add(
                            egui::DragValue::new(&mut view.position_ms)
                                .range(0..=duration.saturating_sub(1)),
                        );
                        ui.label("ms");
                        let seek = ui.button("Seek").clicked()
                            || response.drag_stopped()
                            || (response.changed() && !response.dragged());
                        if seek && view.position_ms != displayed_position {
                            view.send(
                                tx,
                                Job::Apply(RecordingEditorRequest::Seek {
                                    position_ms: view.position_ms,
                                }),
                            );
                        }
                    });
                });
                ui.group(|ui| {
                    ui.set_min_width(ui.available_width());
                    let heading = ui.strong("Trim");
                    let timeline = egui::Rect::from_min_max(
                        egui::pos2(heading.rect.right() + tokens.number("s-5"), heading.rect.top()),
                        egui::pos2(ui.max_rect().right(), heading.rect.bottom()),
                    ).expand2(egui::vec2(0., tokens.number("s-2")));
                    show_trim_timeline(ui, tokens, view, duration, timeline);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Start (ms)");
                        ui.add(egui::DragValue::new(&mut view.start_ms).range(0..=duration));
                        ui.label("End (ms)");
                        ui.add(egui::DragValue::new(&mut view.end_ms).range(0..=duration));
                        ui.label(format!(
                            "{:.3}s selected",
                            view.end_ms.saturating_sub(view.start_ms) as f64 / 1000.
                        ));
                        if ui.button("Reset trim").clicked() {
                            view.start_ms = 0;
                            view.end_ms = duration;
                        }
                    });
                });
                ui.group(|ui| {
                    ui.strong("Crop & size");
                    let mut crop_enabled = view.crop.is_some();
                    ui.horizontal_wrapped(|ui| {
                        if ui.checkbox(&mut crop_enabled, "Crop recording").changed() {
                            view.crop = crop_enabled.then_some(CropRect {
                                x: 0,
                                y: 0,
                                width: source_size.0,
                                height: source_size.1,
                            });
                            if !crop_enabled {
                                view.adjusting_crop = false;
                            }
                        }
                        ui.add_enabled_ui(crop_enabled, |ui| {
                            let mut locked = !view.crop_aspect_unlocked;
                            if ui.checkbox(&mut locked, "Lock aspect ratio").changed() {
                                view.crop_aspect_unlocked = !locked;
                            }
                            if ui.button(if view.adjusting_crop { "Done cropping" } else { "Adjust crop" }).clicked() {
                                if view.adjusting_crop {
                                    view.adjusting_crop = false;
                                } else {
                                    view.request_crop_view(tx);
                                }
                            }
                        });
                    });
                    if let Some(crop) = &mut view.crop {
                        ui.horizontal_wrapped(|ui| {
                            for (label, value, minimum, maximum) in [
                                ("X", &mut crop.x, 0, source_size.0.saturating_sub(2)),
                                ("Y", &mut crop.y, 0, source_size.1.saturating_sub(2)),
                            ] {
                                ui.label(label);
                                ui.add(egui::DragValue::new(value).range(minimum..=maximum));
                            }
                            for (label, horizontal, maximum) in [
                                ("Width", true, source_size.0),
                                ("Height", false, source_size.1),
                            ] {
                                let mut value = if horizontal { crop.width } else { crop.height };
                                ui.label(label);
                                if ui.add(egui::DragValue::new(&mut value).range(2..=maximum).update_while_editing(false)).changed() {
                                    if !view.crop_aspect_unlocked {
                                        *crop = crop.resize_aspect_locked(
                                            source_size.0,
                                            source_size.1,
                                            if horizontal {
                                                CropResizeAxis::Width
                                            } else {
                                                CropResizeAxis::Height
                                            },
                                            value,
                                        );
                                    } else if horizontal {
                                        crop.width = value;
                                    } else {
                                        crop.height = value;
                                    }
                                }
                            }
                        });
                    }
                    let mut resize = view.output_size.is_some();
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut resize, "Custom output size").changed() {
                            view.output_size = resize.then(|| {
                                view.output_dimensions(source_size).unwrap_or_else(|| {
                                    view.crop
                                        .map_or(source_size, |crop| (crop.width, crop.height))
                                })
                            });
                        }
                        ui.add_enabled_ui(!resize, |ui| {
                            ui.label("Preset");
                            egui::ComboBox::from_id_salt("recording-resolution")
                                .selected_text(match view.max_resolution {
                                    MaxResolution::Original => "Original",
                                    MaxResolution::P1080 => "1080p maximum",
                                    MaxResolution::P720 => "720p maximum",
                                })
                                .show_ui(ui, |ui| {
                                    for (preset, label) in [
                                        (MaxResolution::Original, "Original"),
                                        (MaxResolution::P1080, "1080p maximum"),
                                        (MaxResolution::P720, "720p maximum"),
                                    ] {
                                        ui.selectable_value(&mut view.max_resolution, preset, label);
                                    }
                                })
                                .response
                                .on_hover_text("Scale down by height, keeping the crop aspect ratio. Never upscale.");
                        });
                    });
                    if let Some((width, height)) = &mut view.output_size {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Width");
                            ui.add(egui::DragValue::new(width).range(2..=u32::MAX));
                            ui.label("Height");
                            ui.add(egui::DragValue::new(height).range(2..=u32::MAX));
                            ui.weak("Aspect ratio is not locked");
                        });
                    }
                    ui.weak("Apply previews even-pixel sizes for the selected format and quality.");
                });
                ui.group(|ui| {
                    ui.strong("Audio");
                    if !system_audio && !microphone_audio {
                        ui.weak("No audio tracks in this recording.");
                        return;
                    }
                    ui.weak(if view.gif {
                        "GIF has no audio. Settings are kept for MP4."
                    } else {
                        "Apply for export and Sound preview"
                    });
                    ui.add_enabled_ui(!view.gif, |ui| {
                        for (available, label, volume, mute) in [
                            (
                                system_audio,
                                "System",
                                &mut view.audio.system_volume,
                                &mut view.audio.mute_system_audio,
                            ),
                            (
                                microphone_audio,
                                "Microphone",
                                &mut view.audio.microphone_volume,
                                &mut view.audio.mute_microphone,
                            ),
                        ] {
                            if available {
                                ui.horizontal(|ui| {
                                    ui.label(label);
                                    ui.add_enabled(
                                        !*mute,
                                        egui::Slider::new(volume, 0.0..=2.0)
                                            .step_by(0.01)
                                            .custom_formatter(|value, _| {
                                                format!("{:.0}%", value * 100.)
                                            })
                                            .custom_parser(|input| {
                                                input
                                                    .trim()
                                                    .trim_end_matches('%')
                                                    .trim()
                                                    .parse::<f64>()
                                                    .ok()
                                                    .map(|value| value / 100.)
                                            }),
                                    );
                                    ui.checkbox(mute, "Mute");
                                });
                            }
                        }
                        ui.checkbox(&mut view.audio.mono_output, "Mono output");
                    });
                });
                if view.gif {
                    ui.horizontal(|ui| {
                        ui.strong("GIF frame rate");
                        let mut fps = view.gif_frames_per_second.unwrap_or(15);
                        egui::ComboBox::from_id_salt("recording-gif-frame-rate")
                            .selected_text(format!("{fps} FPS"))
                            .height(7.0 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.y))
                            .show_ui(ui, |ui| {
                                for value in [8, 10, 12, 15, 20, 24, 30] {
                                    if ui.selectable_value(&mut fps, value, format!("{value} FPS")).changed() {
                                        view.gif_frames_per_second = Some(fps);
                                    }
                                }
                            });
                    });
                }
                ui.horizontal(|ui| {
                    ui.strong("Save quality");
                    egui::ComboBox::from_id_salt("recording-quality")
                        .selected_text(format!("{:?}", view.quality))
                        .show_ui(ui, |ui| {
                            for quality in [
                                QualityPreset::Preserve,
                                QualityPreset::Highest,
                                QualityPreset::High,
                                QualityPreset::Standard,
                                QualityPreset::Small,
                                QualityPreset::Tiny,
                            ] {
                                ui.selectable_value(
                                    &mut view.quality,
                                    quality,
                                    format!("{quality:?}"),
                                );
                            }
                        });
                });
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened() -> View {
        let mut view = View::default();
        view.receive(
            &egui::Context::default(),
            Event::Presented(Ok(Presented {
                source: MediaMetadata {
                    kind: captures_media::MediaKind::Video,
                    mime_type: "video/mp4".into(),
                    width: 4,
                    height: 2,
                    duration_ms: Some(3100),
                    size_bytes: 40,
                },
                edit: EditSpec::default(),
                export: view.export_spec(),
                position_ms: 700,
                frame: Arc::new(RgbaImage::new(4, 2)),
            })),
        );
        view
    }

    #[test]
    fn actual_preview_scroll_and_crop_mapping_are_display_only() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let ctx = egui::Context::default();
        tokens.apply(&ctx, true);
        let mut view = opened();
        let frame = Arc::new(RgbaImage::new(1200, 800));
        let p = view.presented.as_mut().unwrap();
        p.source.width = 1200;
        p.source.height = 800;
        p.frame = frame.clone();
        view.set_frame(&ctx, &frame);
        view.estimate = Some(ExportEstimate {
            size_bytes: 4567,
            exact: true,
        });
        view.playback_position_ms = Some(1337);
        let (tx, jobs) = mpsc::channel();
        let (events, _) = mpsc::channel();
        let mut time = 0.;
        let mut render = |view: &mut View, input: Vec<egui::Event>| {
            time += 0.1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760., 580.),
                    )),
                    time: Some(time),
                    events: input,
                    ..Default::default()
                },
                |ui| show(ui, &tokens, view, &tx, &events, egui::ViewportId::ROOT),
            );
            output.textures_delta.clear();
            output
        };
        let label = |output: &egui::FullOutput, name: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == name => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {name}"))
        };
        let image = |output: &egui::FullOutput, id: egui::TextureId| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == id => {
                        Some((mesh.calc_bounds(), shape.clip_rect))
                    }
                    _ => None,
                })
                .expect("preview image mesh")
        };
        render(&mut view, vec![]);
        let fit = render(&mut view, vec![]);
        let id = view.texture.as_ref().unwrap().id();
        assert!(image(&fit, id).0.width() < 1200.);
        let actual = label(&fit, "100%");
        let output = render(
            &mut view,
            vec![
                egui::Event::PointerMoved(actual),
                trim_pointer(actual, true),
                trim_pointer(actual, false),
            ],
        );
        assert!(view.preview_actual_size);
        let (actual_image, clip) = image(&output, id);
        assert_eq!(actual_image.size(), egui::vec2(1200., 800.));
        assert!(clip.width() <= 760. && clip.height() < 400.);
        assert!(jobs.try_recv().is_err());
        assert!(!view.dirty() && !view.history_changed);
        assert_eq!(view.estimate.as_ref().unwrap().size_bytes, 4567);
        assert_eq!(view.playback_position_ms, Some(1337));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));

        view.crop = Some(CropRect {
            x: 40,
            y: 20,
            width: 200,
            height: 100,
        });
        view.source_texture = Some(view.texture.as_ref().unwrap().clone());
        view.adjusting_crop = true;
        let output = render(&mut view, vec![]);
        let (before, clip) = image(&output, id);
        let origin = before.min + egui::vec2(100., 60.);
        assert!(clip.contains(origin));
        view.crop_gesture = Some(CropGesture {
            initial: view.crop.unwrap(),
            handle: CropDragHandle::Move,
            origin,
            image: before,
            locked: true,
        });
        render(
            &mut view,
            vec![
                egui::Event::PointerMoved(origin),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(-30., -40.),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let output = render(&mut view, vec![]);
        let (scrolled, clip) = image(&output, id);
        assert!(scrolled.left() < before.left() && scrolled.top() < before.top());
        assert_eq!(scrolled.size(), egui::vec2(1200., 800.));
        assert!(
            view.crop_gesture.is_none(),
            "scrolling ends an active source gesture"
        );
        let point = scrolled.min + egui::vec2(100., 60.);
        assert!(clip.contains(point));
        render(
            &mut view,
            vec![
                egui::Event::PointerMoved(point),
                trim_pointer(point, true),
                egui::Event::PointerMoved(point + egui::vec2(13., 7.)),
                trim_pointer(point + egui::vec2(13., 7.), false),
            ],
        );
        assert_eq!(
            view.crop,
            Some(CropRect {
                x: 53,
                y: 27,
                width: 200,
                height: 100
            })
        );
        assert!(
            jobs.try_recv().is_err(),
            "scroll and crop feedback never decode"
        );
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.playback_position_ms, Some(1337));

        view.adjusting_crop = false;
        view.set_frame(&ctx, &RgbaImage::new(200, 80));
        render(&mut view, vec![]);
        let output = render(&mut view, vec![]);
        let (small, clip) = image(&output, id);
        assert_eq!(
            small.size(),
            egui::vec2(200., 80.),
            "100% uses decoded pixels, not source metadata"
        );
        assert!(
            (small.center() - clip.center()).length() < 1.,
            "small images stay centered"
        );
        view.receive(&ctx, Event::Presented(Ok(opened().presented.unwrap())));
        assert!(
            view.preview_actual_size,
            "accepted updates preserve the display preference"
        );
        assert!(!opened().preview_actual_size, "new items default to Fit");
    }

    #[test]
    fn source_crop_view_is_independent_cached_and_invalidated_by_seek() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.crop = Some(CropRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        });
        view.estimate = Some(ExportEstimate {
            size_bytes: 5432,
            exact: true,
        });
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let texture = view.texture.as_ref().unwrap().id();
        let dirty = view.dirty();
        let (tx, jobs) = mpsc::channel();
        view.request_crop_view(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::SourceFrame(_)));
        assert!(view.busy && !view.adjusting_crop);
        view.receive(&ctx, Event::SourceFrame(Ok(Arc::new(RgbaImage::new(4, 2)))));
        assert!(!view.busy && view.adjusting_crop);
        assert_eq!(view.texture.as_ref().unwrap().id(), texture);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.dirty(), dirty);
        assert_eq!(view.estimate.as_ref().unwrap().size_bytes, 5432);
        assert_eq!(view.position_ms, 700);
        view.adjusting_crop = false;
        view.request_crop_view(&tx);
        assert!(
            view.adjusting_crop && jobs.try_recv().is_err(),
            "same-position source is reused"
        );
        view.request_playback(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "source crop mode never starts motion"
        );

        let p = opened().presented.unwrap();
        view.receive(&ctx, Event::Presented(Ok(p)));
        assert!(
            !view.adjusting_crop && view.source_texture.is_some(),
            "same-position Apply retains source pixels"
        );
        let mut p = opened().presented.unwrap();
        p.position_ms = 1337;
        view.receive(&ctx, Event::Presented(Ok(p)));
        assert!(
            view.source_texture.is_none(),
            "Seek cannot reuse another position's source still"
        );
        assert!(
            opened().source_texture.is_none(),
            "another item has no cached source pixels"
        );
    }

    #[test]
    fn source_crop_cancel_late_success_failure_retry_and_busy_gates() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.crop = Some(CropRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        });
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let (tx, jobs) = mpsc::channel();
        view.request_crop_view(&tx);
        let Job::SourceFrame(cancel) = jobs.recv().unwrap() else {
            panic!("source job")
        };
        view.request_crop_view(&tx);
        view.request_close();
        assert!(!view.closed && jobs.try_recv().is_err());
        cancel.cancel();
        view.receive(&ctx, Event::SourceFrame(Ok(Arc::new(RgbaImage::new(4, 2)))));
        assert!(!view.adjusting_crop && view.source_texture.is_none() && !view.busy);
        view.request_crop_view(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::SourceFrame(_)));
        view.receive(&ctx, Event::SourceFrame(Err("missing source".into())));
        assert!(!view.adjusting_crop && !view.busy);
        assert!(view.error.as_ref().unwrap().contains("missing source"));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        view.request_crop_view(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::SourceFrame(_)));
    }

    #[test]
    fn crop_pointer_maps_letterboxed_source_and_preserves_accepted_identity() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let image = egui::Rect::from_min_size(egui::pos2(50., 75.), egui::vec2(400., 225.));
        let initial = CropRect {
            x: 40,
            y: 20,
            width: 160,
            height: 80,
        };
        for (start, end, locked, expected) in [
            (
                egui::pos2(180., 150.),
                egui::pos2(205., 162.5),
                false,
                CropRect {
                    x: 60,
                    y: 30,
                    width: 160,
                    height: 80,
                },
            ),
            (
                egui::pos2(102., 102.),
                egui::pos2(77., 89.5),
                true,
                CropRect {
                    x: 20,
                    y: 10,
                    width: 180,
                    height: 90,
                },
            ),
            (
                egui::pos2(302., 202.),
                egui::pos2(342., 227.),
                false,
                CropRect {
                    x: 40,
                    y: 20,
                    width: 192,
                    height: 100,
                },
            ),
        ] {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, true);
            let mut view = opened();
            view.presented.as_mut().unwrap().source.width = 320;
            view.presented.as_mut().unwrap().source.height = 180;
            view.crop = Some(initial);
            view.crop_aspect_unlocked = !locked;
            let accepted = view.presented.as_ref().unwrap().frame.clone();
            let mut run = |events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(600., 400.),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        show_crop_overlay(ui, &tokens, &mut view, image);
                        if ctx.current_pass_index() == 0 {
                            ctx.request_discard("crop input runs once");
                        }
                    },
                );
                output.textures_delta.clear();
            };
            run(vec![]);
            run(vec![
                egui::Event::PointerMoved(start),
                trim_pointer(start, true),
                egui::Event::PointerMoved(end),
                trim_pointer(end, false),
            ]);
            assert_eq!(view.crop, Some(expected));
            assert!(view.crop_gesture.is_none());
            assert!(Arc::ptr_eq(
                &accepted,
                &view.presented.as_ref().unwrap().frame
            ));
            assert!(view.unapplied(), "only staged geometry changes");
            assert_eq!(view.position_ms, 700);
        }
    }

    #[test]
    fn crop_gesture_ends_on_escape_focus_layout_and_busy_and_keys_use_source_pixels() {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let ctx = egui::Context::default();
        tokens.apply(&ctx, false);
        let image = egui::Rect::from_min_size(egui::pos2(50., 75.), egui::vec2(400., 225.));
        let mut view = opened();
        view.presented.as_mut().unwrap().source.width = 320;
        view.presented.as_mut().unwrap().source.height = 180;
        view.crop = Some(CropRect {
            x: 40,
            y: 20,
            width: 160,
            height: 80,
        });
        let mut run = |events, image, focused, busy| {
            view.busy = busy;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600., 400.),
                    )),
                    events,
                    focused,
                    ..Default::default()
                },
                |ui| show_crop_overlay(ui, &tokens, &mut view, image),
            );
            output.textures_delta.clear();
            (view.crop.unwrap(), view.crop_gesture.is_some())
        };
        run(vec![], image, true, false);
        let start = egui::pos2(180., 150.);
        let moved = egui::pos2(205., 162.5);
        let expected = CropRect {
            x: 60,
            y: 30,
            width: 160,
            height: 80,
        };
        assert_eq!(
            run(
                vec![trim_pointer(start, true), egui::Event::PointerMoved(moved)],
                image,
                true,
                false
            ),
            (expected, true)
        );
        assert_eq!(
            run(
                vec![
                    trim_key(egui::Key::Escape),
                    egui::Event::PointerMoved(egui::pos2(400., 300.))
                ],
                image,
                true,
                false
            ),
            (expected, false)
        );
        run(
            vec![trim_pointer(moved, false), trim_pointer(moved, true)],
            image,
            true,
            false,
        );
        assert_eq!(
            run(
                vec![egui::Event::PointerMoved(egui::pos2(400., 300.))],
                image,
                false,
                false
            ),
            (expected, false)
        );
        assert_eq!(
            run(
                vec![
                    trim_pointer(moved, false),
                    trim_pointer(moved, true),
                    egui::Event::PointerMoved(egui::pos2(400., 300.))
                ],
                image,
                true,
                true
            ),
            (expected, false)
        );
        run(
            vec![trim_pointer(moved, false), trim_pointer(moved, true)],
            image,
            true,
            false,
        );
        assert_eq!(
            run(
                vec![egui::Event::PointerMoved(egui::pos2(400., 300.))],
                image.translate(egui::vec2(10., 0.)),
                true,
                false
            ),
            (expected, false)
        );
        run(
            vec![
                trim_pointer(moved, false),
                trim_pointer(moved, true),
                trim_pointer(moved, false),
            ],
            image,
            true,
            false,
        );
        assert_eq!(
            run(vec![trim_key(egui::Key::ArrowRight)], image, true, false)
                .0
                .x,
            61
        );
        let shift = egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        assert_eq!(
            run(vec![shift], image, true, false).0.y,
            40,
            "Shift nudge is ten source pixels, not ten scaled view points"
        );
    }

    #[test]
    fn crop_pointer_finishes_pending_text_before_starting_a_new_gesture() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let ctx = egui::Context::default();
        tokens.apply(&ctx, true);
        let mut view = opened();
        view.presented.as_mut().unwrap().source.width = 320;
        view.presented.as_mut().unwrap().source.height = 180;
        let initial = CropRect {
            x: 40,
            y: 20,
            width: 160,
            height: 80,
        };
        view.crop = Some(initial);
        let image = egui::Rect::from_min_size(egui::pos2(50., 75.), egui::vec2(400., 225.));
        let id = egui::Id::unique("pending numeric text");
        let mut pending = "200".to_owned();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.add(egui::TextEdit::singleline(&mut pending).id(id))
                    .request_focus();
            });
            output.textures_delta.clear();
        }
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![
                    trim_pointer(egui::pos2(180., 150.), true),
                    egui::Event::PointerMoved(egui::pos2(205., 162.5)),
                    trim_pointer(egui::pos2(205., 162.5), false),
                ],
                ..Default::default()
            },
            |ui| {
                show_crop_overlay(ui, &tokens, &mut view, image);
                ui.add(egui::TextEdit::singleline(&mut pending).id(id));
            },
        );
        output.textures_delta.clear();
        assert_eq!(
            view.crop,
            Some(initial),
            "do not move from stale geometry while a field commits"
        );
        assert!(view.crop_gesture.is_none());
        assert_eq!(pending, "200");
        assert_ne!(ctx.memory(|memory| memory.focused()), Some(id));
    }

    fn timeline_frame(
        ctx: &egui::Context,
        tokens: &Tokens,
        view: &mut View,
        events: Vec<egui::Event>,
        rect: egui::Rect,
        focused: bool,
    ) {
        let duration = view.presented.as_ref().unwrap().source.duration_ms.unwrap();
        let mut passes = 0;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 200.),
                )),
                events,
                focused,
                ..Default::default()
            },
            |ui| {
                passes += 1;
                show_trim_timeline(ui, tokens, view, duration, rect);
                if ctx.current_pass_index() == 0 {
                    ctx.request_discard("trim input must run once");
                }
            },
        );
        output.textures_delta.clear();
        assert_eq!(passes, 2);
    }

    fn trim_pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn trim_key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn trim_pointer_stages_once_preserves_grab_offset_and_never_changes_accepted_frame() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let ctx = egui::Context::default();
        let grip = tokens.number("s-6");
        let rect =
            egui::Rect::from_min_size(egui::pos2(40., 40.), egui::vec2(400. + 2. * grip, 24.));
        let down = egui::pos2(40. + grip / 2., 52.);
        let mut view = opened();
        let accepted = view.presented.as_ref().unwrap().frame.clone();
        let frame =
            |view: &mut View, events| timeline_frame(&ctx, &tokens, view, events, rect, true);
        frame(&mut view, vec![]);
        frame(
            &mut view,
            vec![
                trim_pointer(down, true),
                egui::Event::PointerMoved(down + egui::vec2(2.99, 0.)),
            ],
        );
        assert_eq!(
            view.start_ms, 0,
            "a click or subthreshold move must not jump the trim"
        );
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(down + egui::vec2(3., 0.))],
        );
        assert_eq!(view.start_ms, 23, "3px / 400px × 3100ms rounds to 23ms");
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(down + egui::vec2(40., 0.))],
        );
        assert_eq!(
            view.start_ms, 310,
            "delta is measured from the original grab, not the moved grip"
        );
        assert!(
            view.trim_gesture.is_some(),
            "a second layout pass cannot replay pointer down"
        );
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(egui::pos2(4000., 52.))],
        );
        assert_eq!(
            view.start_ms, 310,
            "wild coordinates retain the last accepted sample"
        );
        let release = down + egui::vec2(100., 0.);
        frame(
            &mut view,
            vec![
                egui::Event::PointerMoved(release),
                trim_pointer(release, false),
                egui::Event::PointerMoved(down + egui::vec2(200., 0.)),
            ],
        );
        assert_eq!((view.start_ms, view.end_ms), (775, 3100));
        assert!(view.trim_gesture.is_none() && view.unapplied() && view.dirty());
        assert_eq!(view.position_ms, 700);
        assert_eq!(view.presented.as_ref().unwrap().edit, EditSpec::default());
        assert!(Arc::ptr_eq(
            &accepted,
            &view.presented.as_ref().unwrap().frame
        ));
        assert!(!view.busy && !view.history_changed);
        assert_eq!(view.estimate_label(), "Apply edits to estimate size");
    }

    #[test]
    fn trim_cancellation_and_busy_gates_keep_staged_values_without_resuming_a_drag() {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let grip = tokens.number("s-6");
        let rect =
            egui::Rect::from_min_size(egui::pos2(40., 40.), egui::vec2(400. + 2. * grip, 24.));
        for reason in [
            "escape",
            "pointer gone",
            "focus",
            "resize",
            "busy",
            "picker",
            "close",
        ] {
            let ctx = egui::Context::default();
            let mut view = opened();
            let down = egui::pos2(rect.right() - grip / 2., 52.);
            timeline_frame(&ctx, &tokens, &mut view, vec![], rect, true);
            timeline_frame(
                &ctx,
                &tokens,
                &mut view,
                vec![
                    trim_pointer(down, true),
                    egui::Event::PointerMoved(down - egui::vec2(40., 0.)),
                ],
                rect,
                true,
            );
            assert_eq!(view.end_ms, 2790, "{reason}: end drag must be established");
            let events = match reason {
                "escape" => vec![trim_key(egui::Key::Escape)],
                "pointer gone" => vec![egui::Event::PointerGone],
                "busy" => {
                    view.busy = true;
                    vec![]
                }
                "picker" => {
                    view.picker = true;
                    vec![]
                }
                "close" => {
                    view.request_close();
                    vec![]
                }
                _ => vec![],
            };
            let resized = if reason == "resize" {
                rect.translate(egui::vec2(3., 0.))
            } else {
                rect
            };
            timeline_frame(&ctx, &tokens, &mut view, events, resized, reason != "focus");
            assert!(view.trim_gesture.is_none(), "{reason}");
            view.busy = false;
            view.picker = false;
            view.confirm_close = false;
            timeline_frame(
                &ctx,
                &tokens,
                &mut view,
                vec![egui::Event::PointerMoved(down - egui::vec2(80., 0.))],
                resized,
                true,
            );
            assert_eq!(
                (view.start_ms, view.end_ms),
                (0, 2790),
                "{reason}: no rollback or resurrection"
            );
        }
    }

    #[test]
    fn trim_keyboard_is_focus_scoped_and_preserves_one_millisecond_span() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let grip = tokens.number("s-6");
        let rect =
            egui::Rect::from_min_size(egui::pos2(40., 40.), egui::vec2(400. + 2. * grip, 24.));
        for duration in [59_999, 60_000] {
            let ctx = egui::Context::default();
            let mut view = opened();
            view.end_ms = duration;
            view.presented.as_mut().unwrap().source.duration_ms = Some(duration);
            let frame =
                |view: &mut View, events| timeline_frame(&ctx, &tokens, view, events, rect, true);
            frame(&mut view, vec![]);
            frame(&mut view, vec![trim_key(egui::Key::PageDown)]);
            assert_eq!(view.end_ms, duration, "unfocused keys must not change trim");
            let down = egui::pos2(rect.right() - grip / 2., 52.);
            frame(
                &mut view,
                vec![trim_pointer(down, true), trim_pointer(down, false)],
            );
            frame(
                &mut view,
                vec![
                    trim_key(egui::Key::ArrowLeft),
                    trim_key(egui::Key::ArrowLeft),
                ],
            );
            assert_eq!(
                view.end_ms,
                duration - if duration < 60_000 { 2 } else { 20 }
            );
            frame(&mut view, vec![trim_key(egui::Key::PageDown)]);
            assert_eq!(
                view.end_ms,
                duration - if duration < 60_000 { 1002 } else { 1020 }
            );
            view.start_ms = view.end_ms - 3;
            frame(&mut view, vec![trim_key(egui::Key::PageDown)]);
            assert_eq!(view.end_ms, view.start_ms + 1);
            let down = egui::pos2(
                rect.left() + grip + 400. * view.start_ms as f32 / duration as f32 - grip / 2.,
                52.,
            );
            frame(
                &mut view,
                vec![trim_pointer(down, true), trim_pointer(down, false)],
            );
            frame(&mut view, vec![trim_key(egui::Key::PageUp)]);
            assert_eq!(
                view.start_ms,
                view.end_ms - 1,
                "adjacent grips keep distinct focus/hit regions"
            );
        }
    }

    #[test]
    fn estimates_follow_accepted_settings_not_seek_or_staged_values() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.receive(
            &ctx,
            Event::Estimated(Ok(ExportEstimate {
                size_bytes: 12345,
                exact: true,
            })),
        );
        assert_eq!(view.estimate_label(), "12345 bytes (exact)");
        assert!(!view.dirty() && !view.history_changed);
        let p = view.presented.as_ref().unwrap();
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                source: p.source.clone(),
                edit: p.edit.clone(),
                export: p.export.clone(),
                position_ms: 1200,
                frame: frame.clone(),
            })),
        );
        assert_eq!(
            view.estimate_label(),
            "12345 bytes (exact)",
            "seek cannot change file size"
        );
        let (tx, jobs) = mpsc::channel();
        view.gif = true;
        assert_eq!(view.estimate_label(), "Apply edits to estimate size");
        view.request_estimate(&tx);
        assert!(jobs.try_recv().is_err() && !view.busy);
        view.receive(&ctx, Event::Presented(Err("bad format preview".into())));
        assert_eq!(view.estimate_label(), "Apply edits to estimate size");
        view.gif = false;
        assert_eq!(
            view.estimate_label(),
            "12345 bytes (exact)",
            "reverting staged edits restores the matching result"
        );
        view.gif = true;
        let p = view.presented.as_ref().unwrap();
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                source: p.source.clone(),
                edit: p.edit.clone(),
                export: view.export_spec(),
                position_ms: 1200,
                frame: frame.clone(),
            })),
        );
        assert!(
            view.estimate.is_none(),
            "accepting different settings invalidates the old result"
        );
        view.request_estimate(&tx);
        assert!(matches!(jobs.try_recv(), Ok(Job::Estimate(_))));
        view.request_estimate(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "only one worker operation is accepted"
        );
        view.request_close();
        assert!(!view.closed && !view.confirm_close);
        view.receive(
            &ctx,
            Event::Estimated(Ok(ExportEstimate {
                size_bytes: 67890,
                exact: false,
            })),
        );
        assert_eq!(view.estimate_label(), "≈ 67890 bytes");
        assert!(!view.busy && !view.estimating && view.cancel.is_none() && view.error.is_none());
        assert!(
            view.dirty() && !view.history_changed,
            "estimating does not save edits"
        );
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
    }

    #[test]
    fn playback_pause_resume_and_eof_preserve_accepted_state() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.start_ms = 123;
        view.presented.as_mut().unwrap().edit.trim_start_ms = 123;
        view.saved_edit = view.presented.as_ref().unwrap().edit.clone();
        let accepted = view.presented.as_ref().unwrap().frame.clone();
        let texture_id = view.texture.as_ref().unwrap().id();
        view.estimate = Some(ExportEstimate {
            size_bytes: 1234,
            exact: true,
        });
        let (tx, jobs) = mpsc::channel();
        // Uncommitted seek text is not the frame currently being displayed.
        view.position_ms = 2200;
        view.request_playback(&tx);
        let Job::Play(position, false, cancel) = jobs.recv().unwrap() else {
            panic!("play queued")
        };
        assert_eq!(position, 700);
        assert_eq!(
            view.position_ms, 700,
            "the playhead labels the retained frame during startup"
        );
        assert!(view.playing && view.busy && !view.dirty());
        view.request_playback(&tx);
        view.request_estimate(&tx);
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 1800 }),
        );
        assert!(
            jobs.try_recv().is_err(),
            "playback owns the serialized worker"
        );
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1100,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        assert_eq!(
            view.texture.as_ref().unwrap().id(),
            texture_id,
            "reuse GPU allocation"
        );
        assert_eq!(view.position_ms, 1100);
        view.pause_playback();
        assert!(
            cancel.is_cancelled() && view.busy,
            "Pause waits for decoder teardown"
        );
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1200,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        assert_eq!(
            view.position_ms, 1100,
            "late frame cannot move a paused playhead"
        );
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        assert!(!view.busy && !view.playing && view.cancel.is_none());
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(1100, false, _)));
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 3066,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Ended)));
        view.request_playback(&tx);
        assert!(
            matches!(jobs.recv().unwrap(), Job::Play(123, false, _)),
            "EOF replays accepted trim"
        );
        let p = view.presented.as_ref().unwrap();
        assert_eq!(p.position_ms, 700);
        assert!(Arc::ptr_eq(&accepted, &p.frame));
        assert_eq!(view.estimate.unwrap().size_bytes, 1234);
        assert!(!view.dirty() && !view.history_changed);
    }

    #[test]
    fn seek_after_playback_replaces_transient_frame_and_resume_position() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.preview_loop.store(true, Ordering::Relaxed);
        view.preview_sound = true;
        let (tx, jobs) = mpsc::channel();
        view.request_playback(&tx);
        jobs.recv().unwrap();
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1800,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.pause_playback();
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 950 }),
        );
        assert!(matches!(jobs.recv().unwrap(), Job::Apply(_)));
        let mut presentation = opened().presented.unwrap();
        presentation.position_ms = 950;
        view.receive(&ctx, Event::Presented(Ok(presentation)));
        assert_eq!(view.texture.as_ref().unwrap().size(), [4, 2]);
        assert_eq!(view.position_ms, 950);
        assert!(view.playback_position_ms.is_none() && !view.playback_ended);
        assert!(
            view.preview_loop.load(Ordering::Relaxed),
            "Pause/seek retains the loop preference"
        );
        assert!(
            view.preview_sound,
            "Pause/seek retains the sound preference"
        );
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(950, true, _)));
        assert!(!view.dirty());
    }

    #[test]
    fn sound_metadata_cancellation_and_device_error_preserve_accepted_state() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.estimate = Some(ExportEstimate {
            size_bytes: 5678,
            exact: true,
        });
        view.preview_sound = true;
        let (tx, jobs) = mpsc::channel();
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(700, true, _)));
        view.receive(
            &ctx,
            Event::PlaybackStarted {
                audio_enabled: true,
            },
        );
        assert!(
            view.playback_audio_enabled && view.status.as_ref().unwrap().contains("default output")
        );
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1337,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.pause_playback();
        view.receive(
            &ctx,
            Event::PlaybackStarted {
                audio_enabled: false,
            },
        );
        assert!(
            view.playback_audio_enabled,
            "late metadata is ignored after Pause"
        );
        assert_eq!(
            view.position_ms, 1337,
            "metadata never changes presentation time"
        );
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        assert!(view.preview_sound && !view.playback_audio_enabled);
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(1337, true, _)));
        view.receive(
            &ctx,
            Event::PlaybackStarted {
                audio_enabled: false,
            },
        );
        assert!(!view.playback_audio_enabled);
        assert!(view.status.as_ref().unwrap().contains("has no sound"));
        view.receive(
            &ctx,
            Event::PlaybackFinished(Err("output device unavailable".into())),
        );
        assert!(view.preview_sound && view.status.is_none());
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("output device unavailable")
        );
        assert_eq!(view.position_ms, 700);
        assert!(
            jobs.try_recv().is_err(),
            "device failure never silently retries without sound"
        );
        view.preview_sound = false;
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(700, false, _)));
        assert!(view.error.is_none());
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.estimate.unwrap().size_bytes, 5678);
        assert!(!view.dirty() && !view.unapplied() && !view.history_changed);
        assert!(
            !opened().preview_sound,
            "new items default to silent playback"
        );
    }

    #[test]
    fn playback_error_restores_still_and_close_retains_dirty_confirmation() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let (tx, jobs) = mpsc::channel();
        view.start_ms = 123;
        view.request_playback(&tx);
        assert!(jobs.try_recv().is_err(), "unapplied fields block Play");
        view.presented.as_mut().unwrap().edit.trim_start_ms = 123;
        assert!(view.dirty() && !view.unapplied());
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(700, false, _)));
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1500,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.receive(&ctx, Event::PlaybackFinished(Err("source removed".into())));
        assert_eq!(view.position_ms, 700);
        assert_eq!(view.texture.as_ref().unwrap().size(), [4, 2]);
        assert!(view.playback_position_ms.is_none() && view.dirty());
        assert!(view.error.as_ref().unwrap().contains("source removed"));
        view.request_playback(&tx);
        let Job::Play(_, false, cancel) = jobs.recv().unwrap() else {
            panic!("retry queued")
        };
        assert!(!cancel.is_cancelled() && view.error.is_none());
        view.request_close();
        assert!(cancel.is_cancelled() && !view.closed && !view.confirm_close);
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        assert!(
            view.confirm_close && !view.closed,
            "only ask discard after playback teardown"
        );
    }

    #[test]
    fn playback_mailbox_is_latest_only_and_drains_before_completion() {
        let ctx = egui::Context::default();
        let (tx, jobs) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let mut view = opened();
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(_, false, _)));
        let editor = Editor {
            viewport: egui::ViewportId::ROOT,
            view: Arc::new(Mutex::new(view)),
            playback_frame: Arc::new(Mutex::new(None)),
            tx,
            events,
            rx,
            worker: None,
        };
        let old = Arc::new(RgbaImage::new(2, 1));
        *editor.playback_frame.lock().unwrap() = Some(PlaybackFrame {
            position_ms: 900,
            pixels: old.clone(),
        });
        *editor.playback_frame.lock().unwrap() = Some(PlaybackFrame {
            position_ms: 3066,
            pixels: Arc::new(RgbaImage::new(2, 1)),
        });
        assert_eq!(
            Arc::strong_count(&old),
            1,
            "replacing latest promptly releases superseded pixels"
        );
        editor
            .events
            .send(Event::PlaybackFinished(Ok(PlaybackEnd::Ended)))
            .unwrap();
        editor.receive(&ctx);
        assert_eq!(editor.view.lock().unwrap().position_ms, 3066);
        assert!(editor.playback_frame.lock().unwrap().is_none());
        assert!(
            editor.flush(&ctx).is_ok(),
            "motion alone never blocks clean quit"
        );
        editor.view.lock().unwrap().request_playback(&editor.tx);
        let Job::Play(_, false, cancel) = jobs.recv().unwrap() else {
            panic!("replay")
        };
        assert!(editor.flush(&ctx).is_err());
        assert!(
            cancel.is_cancelled(),
            "quit requests teardown without freeing a live worker"
        );
        editor
            .events
            .send(Event::PlaybackFinished(Ok(PlaybackEnd::Paused)))
            .unwrap();
        editor.receive(&ctx);
        assert!(editor.flush(&ctx).is_ok());
        editor.view.lock().unwrap().request_playback(&editor.tx);
        editor.view.lock().unwrap().request_close();
        editor
            .events
            .send(Event::PlaybackFinished(Ok(PlaybackEnd::Paused)))
            .unwrap();
        editor.receive(&ctx);
        assert!(
            editor.closed(),
            "clean close finishes only after worker completion"
        );
    }

    #[test]
    fn thumbnail_cancel_failure_retry_keeps_edits_preview_and_estimate() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.estimate = Some(ExportEstimate {
            size_bytes: 987,
            exact: true,
        });
        let (tx, jobs) = mpsc::channel();
        view.request_thumbnails(&tx);
        let Job::Thumbnails(cancel) = jobs.recv().unwrap() else {
            panic!("thumbnails queued")
        };
        assert!(view.busy && view.loading_thumbnails && !view.dirty());
        view.request_thumbnails(&tx);
        view.request_estimate(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "generation is serialized with other media work"
        );
        view.request_close();
        assert!(!view.closed && !view.confirm_close);
        view.cancel.as_ref().unwrap().cancel();
        assert!(cancel.is_cancelled());
        view.receive(&ctx, Event::Thumbnails(Err("cancelled".into())));
        assert!(!view.busy && !view.loading_thumbnails && view.cancel.is_none());
        assert!(
            view.error.is_none(),
            "the busy-close warning no longer applies"
        );
        assert_eq!(view.thumbnail_error.as_deref(), Some("cancelled"));
        assert!(!view.dirty() && !view.history_changed);
        assert_eq!(view.estimate.unwrap().size_bytes, 987);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));

        view.start_ms = 300;
        view.request_thumbnails(&tx);
        let Job::Thumbnails(retry) = jobs.recv().unwrap() else {
            panic!("retry queued")
        };
        assert!(!retry.is_cancelled() && view.thumbnail_error.is_none());
        view.receive(&ctx, Event::Thumbnails(Err("source missing".into())));
        assert!(view.dirty() && view.unapplied());
        assert_eq!(
            (view.start_ms, view.end_ms, view.position_ms),
            (300, 3100, 700)
        );
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.thumbnail_error.as_deref(), Some("source missing"));
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 1000 }),
        );
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Apply(_))),
            "thumbnail failure does not disable the worker"
        );
    }

    #[test]
    fn thumbnails_queue_only_on_first_presentation_and_never_reuse_another_editors_texture() {
        let ctx = egui::Context::default();
        let (tx, jobs) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let editor = Editor {
            viewport: egui::ViewportId::ROOT,
            view: Arc::new(Mutex::new(View {
                busy: true,
                ..View::default()
            })),
            playback_frame: Arc::new(Mutex::new(None)),
            tx,
            events,
            rx,
            worker: None,
        };
        editor
            .events
            .send(Event::Presented(Ok(opened().presented.take().unwrap())))
            .unwrap();
        editor.receive(&ctx);
        assert!(matches!(jobs.try_recv(), Ok(Job::Thumbnails(_))));
        assert!(editor.flush(&ctx).is_err(), "quit waits for generation");
        editor
            .events
            .send(Event::Thumbnails(Err("cancelled".into())))
            .unwrap();
        editor.receive(&ctx);
        assert!(editor.flush(&ctx).is_ok());
        // Uploads are covered by the real X11 test. Retention across published
        // edits/seek must not enqueue another decode or replace this texture.
        let texture = ctx.load_texture(
            "test-source-strip",
            egui::ColorImage::filled([12, 2], egui::Color32::RED),
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        editor.view.lock().unwrap().thumbnails = Some(texture);
        editor
            .events
            .send(Event::Presented(Ok(opened().presented.take().unwrap())))
            .unwrap();
        editor.receive(&ctx);
        assert!(jobs.try_recv().is_err());
        let mut view = editor.view.lock().unwrap();
        assert_eq!(view.thumbnails.as_ref().unwrap().id(), id);
        view.request_thumbnails(&editor.tx);
        assert!(
            jobs.try_recv().is_err(),
            "successful strips are retained for this source"
        );
        assert!(
            View::default().thumbnails.is_none(),
            "new editor/source starts without stale pixels"
        );
    }

    #[test]
    fn cancelled_estimate_preserves_frame_and_can_be_retried() {
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let (tx, jobs) = mpsc::channel();
        view.request_estimate(&tx);
        let Job::Estimate(cancel) = jobs.recv().unwrap() else {
            panic!("estimate queued")
        };
        view.cancel.as_ref().unwrap().cancel();
        assert!(cancel.is_cancelled() && view.busy && view.estimating);
        view.receive(
            &egui::Context::default(),
            Event::Estimated(Err("cancelled".into())),
        );
        assert!(!view.busy && !view.estimating && view.cancel.is_none() && view.estimate.is_none());
        assert_eq!(view.error.as_deref(), Some("cancelled"));
        assert!(!view.dirty() && !view.history_changed);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        view.request_estimate(&tx);
        let Job::Estimate(retry) = jobs.recv().unwrap() else {
            panic!("retry queued")
        };
        assert!(!retry.is_cancelled() && view.error.is_none());
    }

    #[test]
    fn estimate_and_save_controls_fit_the_minimum_window() {
        for (name, tokens) in crate::tokens::load() {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            view.estimate = Some(ExportEstimate {
                size_bytes: 123456789012,
                exact: true,
            });
            view.thumbnail_error = Some("source missing".into());
            view.start_ms = 1550;
            view.end_ms = 1551;
            let accepted = &mut view.presented.as_mut().unwrap().edit;
            accepted.trim_start_ms = 1550;
            accepted.trim_end_ms = Some(1551);
            let (tx, _) = mpsc::channel();
            let (events, _) = mpsc::channel();
            for pass in 0..2 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(760., 580.),
                        )),
                        ..Default::default()
                    },
                    |ui| show(ui, &tokens, &mut view, &tx, &events, egui::ViewportId::ROOT),
                );
                output.textures_delta.clear();
                if pass == 0 {
                    continue;
                }
                let mut rects = Vec::new();
                for label in [
                    "Play",
                    "Loop preview",
                    "Sound",
                    "Fit",
                    "100%",
                    "123456789012 bytes (exact)",
                    "Estimate size",
                    "Apply edits",
                    "Save new copy",
                    "Retry thumbnails",
                ] {
                    let rect = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == label => {
                                Some(text.galley.rect.translate(text.pos.to_vec2()))
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("missing {label} in {name}"));
                    assert!(
                        rect.left() >= 0.
                            && rect.right() <= 760.
                            && rect.top() >= 0.
                            && rect.bottom() <= 580.,
                        "{name}: {label} outside window: {rect:?}"
                    );
                    if label == "Retry thumbnails" {
                        let trim = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.job.text == "Trim" => {
                                    Some(text.galley.rect.translate(text.pos.to_vec2()))
                                }
                                _ => None,
                            })
                            .unwrap();
                        assert!(
                            rect.top() > trim.bottom() + tokens.number("h-md"),
                            "retry must never overlay either grip, even for a 1 ms middle selection"
                        );
                    }
                    for other in &rects {
                        assert!(
                            !rect.intersects(*other),
                            "{name}: overlapping estimate/save labels"
                        );
                    }
                    rects.push(rect);
                }
            }
        }
    }

    #[test]
    fn preview_toggles_are_transient_and_sound_waits_for_worker_teardown() {
        for label in ["Loop preview", "Sound"] {
            for state in 0..6 {
                let tokens = crate::tokens::load().remove("light-mustard").unwrap();
                let ctx = egui::Context::default();
                tokens.apply(&ctx, true);
                let mut view = opened();
                let accepted = view.presented.as_ref().unwrap().frame.clone();
                view.estimate = Some(ExportEstimate {
                    size_bytes: 4321,
                    exact: true,
                });
                let (tx, _jobs) = mpsc::channel();
                let (events, _) = mpsc::channel();
                assert!(!view.preview_loop.load(Ordering::Relaxed));
                if state == 1 || state == 2 {
                    view.request_playback(&tx);
                }
                if state == 2 {
                    view.pause_playback();
                }
                view.busy |= state == 3;
                view.picker = state == 4;
                view.confirm_close = state == 5;
                let mut toggle = egui::Pos2::ZERO;
                for pass in 0..3 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(760., 580.),
                            )),
                            events: if pass == 2 {
                                vec![
                                    egui::Event::PointerMoved(toggle),
                                    trim_pointer(toggle, true),
                                    trim_pointer(toggle, false),
                                ]
                            } else {
                                Vec::new()
                            },
                            ..Default::default()
                        },
                        |ui| show(ui, &tokens, &mut view, &tx, &events, egui::ViewportId::ROOT),
                    );
                    output.textures_delta.clear();
                    if pass == 1 {
                        toggle = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.job.text == label => {
                                    Some(text.pos + text.galley.rect.center().to_vec2())
                                }
                                _ => None,
                            })
                            .expect("preview control visible at minimum size");
                    }
                }
                assert_eq!(
                    if label == "Sound" {
                        view.preview_sound
                    } else {
                        view.preview_loop.load(Ordering::Relaxed)
                    },
                    state == 0 || (label == "Loop preview" && state == 1),
                    "{label}: sound changes only idle; loop also changes playing; both wait for teardown/other work"
                );
                assert!(!view.dirty() && !view.unapplied());
                assert_eq!(view.estimate.as_ref().unwrap().size_bytes, 4321);
                assert!(Arc::ptr_eq(
                    &accepted,
                    &view.presented.as_ref().unwrap().frame
                ));
                assert!(
                    !opened().preview_loop.load(Ordering::Relaxed),
                    "new editors default to non-looping playback"
                );
                assert!(!opened().preview_sound);
            }
        }
    }

    #[test]
    fn fixed_pause_is_clickable_while_worker_owns_the_minimum_window() {
        for (name, tokens) in crate::tokens::load() {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            let (tx, jobs) = mpsc::channel();
            let (events, _) = mpsc::channel();
            view.request_playback(&tx);
            let Job::Play(_, false, cancel) = jobs.recv().unwrap() else {
                panic!("play")
            };
            let mut pause = egui::Pos2::ZERO;
            for pass in 0..3 {
                let input = if pass == 2 {
                    vec![
                        egui::Event::PointerMoved(pause),
                        trim_pointer(pause, true),
                        trim_pointer(pause, false),
                    ]
                } else {
                    Vec::new()
                };
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(760., 580.),
                        )),
                        events: input,
                        ..Default::default()
                    },
                    |ui| show(ui, &tokens, &mut view, &tx, &events, egui::ViewportId::ROOT),
                );
                output.textures_delta.clear();
                if pass == 1 {
                    let rect = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == "Pause playback" => {
                                Some(text.galley.rect.translate(text.pos.to_vec2()))
                            }
                            _ => None,
                        })
                        .expect("fixed footer pause");
                    assert!(
                        rect.left() >= 0.
                            && rect.right() <= 760.
                            && rect.top() > 400.
                            && rect.bottom() < 580.,
                        "{name}: {rect:?}"
                    );
                    pause = rect.center();
                }
            }
            assert!(
                cancel.is_cancelled() && view.busy && view.playing,
                "Pause stays enabled but never releases a live worker"
            );
            assert!(
                jobs.try_recv().is_err(),
                "Pause cancels directly, not behind the active Play job"
            );
        }
    }

    #[test]
    fn failed_seek_retains_frame_position_and_failed_trim_stays_dirty() {
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.position_ms = 2900;
        view.start_ms = 1800;
        view.end_ms = 1100;
        view.receive(
            &egui::Context::default(),
            Event::Presented(Err("invalid trim".into())),
        );
        assert_eq!(view.position_ms, 700);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!((view.start_ms, view.end_ms), (1800, 1100));
        assert!(view.dirty());
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn crop_and_size_are_staged_atomically_and_failures_keep_the_accepted_preview() {
        let mut view = opened();
        let crop = CropRect {
            x: 2,
            y: 0,
            width: 2,
            height: 2,
        };
        view.crop = Some(crop);
        view.output_size = Some((8, 6));
        view.start_ms = 300;
        view.end_ms = 2200;
        let p = view.presented.as_ref().unwrap();
        let frame = p.frame.clone();
        let edit = view.staged_edit(p);
        assert_eq!(
            edit,
            EditSpec {
                trim_start_ms: 300,
                trim_end_ms: Some(2200),
                crop: Some(crop),
                output_width: Some(8),
                output_height: Some(6),
                audio: p.edit.audio.clone(),
            }
        );
        assert!(view.unapplied() && view.dirty());
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("decode failed".into())));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.crop, Some(crop));
        assert_eq!(view.output_size, Some((8, 6)));
        assert!(view.unapplied());
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                source: view.presented.as_ref().unwrap().source.clone(),
                edit,
                export: view.export_spec(),
                position_ms: 700,
                frame: Arc::new(RgbaImage::new(8, 6)),
            })),
        );
        assert!(!view.unapplied() && view.dirty());
        // Removing one transform must retain the accepted trim and other transform.
        view.crop = None;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert!(view.unapplied());
        assert_eq!((edit.trim_start_ms, edit.trim_end_ms), (300, Some(2200)));
        assert_eq!((edit.output_width, edit.output_height), (Some(8), Some(6)));
        assert_eq!(edit.crop, None);
        view.crop = Some(crop);
        view.output_size = None;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(edit.crop, Some(crop));
        assert_eq!((edit.output_width, edit.output_height), (None, None));
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn locked_crop_dimensions_follow_current_ratio_and_origin_bounds() {
        for (source, initial, horizontal, value, expected) in [
            ((320, 180), (0, 0, 320, 180), true, 160, (160, 90)),
            ((640, 1440), (0, 0, 640, 1440), false, 720, (320, 720)),
            ((320, 180), (10, 60, 160, 90), true, 300, (213, 120)),
            ((320, 180), (200, 6, 80, 60), false, 170, (120, 90)),
            ((320, 180), (10, 6, 160, 90), true, 0, (2, 2)),
            ((320, 180), (10, 6, 90, 160), false, 0, (2, 2)),
            ((400, 300), (20, 30, 101, 61), true, 73, (73, 44)),
        ] {
            let mut crop = CropRect {
                x: initial.0,
                y: initial.1,
                width: initial.2,
                height: initial.3,
            };
            crop = crop.resize_aspect_locked(
                source.0,
                source.1,
                if horizontal {
                    CropResizeAxis::Width
                } else {
                    CropResizeAxis::Height
                },
                value,
            );
            assert_eq!((crop.width, crop.height), expected);
            assert_eq!((crop.x, crop.y), (initial.0, initial.1));
        }
        let mut view = opened();
        assert!(
            !view.crop_aspect_unlocked,
            "crop starts locked, as in Tauri"
        );
        view.crop_aspect_unlocked = true;
        assert!(!view.dirty(), "the input preference alone is not an edit");
    }

    #[test]
    fn resolution_presets_use_crop_and_remain_presets_after_acceptance() {
        let mut view = opened();
        let p = view.presented.as_mut().unwrap();
        p.source.width = 4001;
        p.source.height = 2003;
        let frame = p.frame.clone();
        view.crop = Some(CropRect {
            x: 20,
            y: 30,
            width: 1001,
            height: 1501,
        });
        view.max_resolution = MaxResolution::P720;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(
            (edit.output_width, edit.output_height),
            (Some(480), Some(720))
        );
        assert!(view.unapplied());
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("preview failed".into())));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.max_resolution, MaxResolution::P720);
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                source: view.presented.as_ref().unwrap().source.clone(),
                edit,
                export: view.export_spec(),
                position_ms: 700,
                frame: Arc::new(RgbaImage::new(480, 720)),
            })),
        );
        assert_eq!(view.max_resolution, MaxResolution::P720);
        assert!(
            view.output_size.is_none(),
            "acceptance must not turn a preset into a custom size"
        );
        assert!(!view.unapplied() && view.dirty());
        view.crop.as_mut().unwrap().height = 501;
        assert_eq!(view.output_dimensions((4001, 2003)), Some((1000, 500)));
        assert!(
            view.unapplied(),
            "crop changes recalculate the preset without upscaling"
        );
        view.output_size = Some((81, 61));
        assert_eq!(view.output_dimensions((4001, 2003)), Some((81, 61)));
        view.output_size = None;
        view.max_resolution = MaxResolution::Original;
        assert_eq!(view.output_dimensions((4001, 2003)), None);
    }

    #[test]
    fn resolution_caps_preserve_orientation_round_even_and_do_not_upscale() {
        for (preset, source, expected) in [
            (MaxResolution::P1080, (4001, 2003), (2156, 1080)),
            (MaxResolution::P720, (4001, 2003), (1438, 720)),
            (MaxResolution::P1080, (1001, 2003), (540, 1080)),
            (MaxResolution::P720, (1283, 721), (1280, 720)),
            (MaxResolution::P720, (1283, 719), (1282, 718)),
        ] {
            let view = View {
                max_resolution: preset,
                ..View::default()
            };
            assert_eq!(view.output_dimensions(source), Some(expected));
        }
    }

    #[test]
    fn audio_changes_stage_with_geometry_and_survive_failed_apply_and_gif() {
        let mut view = opened();
        let p = view.presented.as_mut().unwrap();
        p.edit.audio.source_has_system_audio = true;
        p.edit.audio.source_has_microphone_audio = true;
        view.audio = p.edit.audio.clone();
        view.saved_edit = p.edit.clone();
        let frame = p.frame.clone();
        view.audio.system_volume = 0.25;
        view.audio.microphone_volume = 1.75;
        view.audio.mute_system_audio = true;
        view.audio.mono_output = true;
        view.start_ms = 300;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(edit.trim_start_ms, 300);
        assert_eq!(
            edit.audio,
            AudioEdit {
                system_volume: 0.25,
                microphone_volume: 1.75,
                mute_system_audio: true,
                mute_microphone: false,
                mono_output: true,
                source_has_system_audio: true,
                source_has_microphone_audio: true,
            }
        );
        assert!(view.unapplied() && view.dirty());
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("decode failed".into())));
        let p = view.presented.as_ref().unwrap();
        assert!(Arc::ptr_eq(&frame, &p.frame));
        assert_eq!(p.edit.audio.system_volume, 1.);
        assert!(!p.edit.audio.mute_system_audio && !p.edit.audio.mono_output);
        assert_eq!(view.audio, edit.audio);
        view.gif = true;
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                source: p.source.clone(),
                edit: edit.clone(),
                export: view.export_spec(),
                position_ms: 700,
                frame,
            })),
        );
        assert!(!view.unapplied() && view.dirty());
        view.gif = false;
        assert_eq!(
            view.staged_edit(view.presented.as_ref().unwrap()).audio,
            edit.audio
        );
        assert!(view.unapplied());
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn gif_frame_rate_is_accepted_output_state_and_retained_across_mp4() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.gif = true;
        assert_eq!(view.gif_frames_per_second.unwrap_or(15), 15);
        view.gif_frames_per_second = Some(8);
        view.estimate = Some(ExportEstimate {
            size_bytes: 1234,
            exact: true,
        });
        assert!(view.unapplied() && view.dirty());
        let (tx, jobs) = mpsc::channel();
        view.request_estimate(&tx);
        view.request_playback(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "staged FPS gates estimate and playback"
        );
        assert_eq!(view.estimate_label(), "Apply edits to estimate size");
        let export = view.export_spec();
        assert_eq!(export.format, ExportFormat::Gif);
        assert_eq!(export.frames_per_second, Some(8));
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::UpdatePreview {
                edit: view.staged_edit(view.presented.as_ref().unwrap()),
                export,
            }),
        );
        let Job::Apply(RecordingEditorRequest::UpdatePreview { edit, export }) =
            jobs.recv().unwrap()
        else {
            panic!("frame rate is sent through Apply")
        };
        let mut p = opened().presented.unwrap();
        p.edit = edit;
        p.export = export;
        view.receive(&ctx, Event::Presented(Ok(p)));
        assert!(!view.unapplied() && view.dirty() && view.estimate.is_none());
        view.receive(
            &ctx,
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "eight-fps.gif".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert!(!view.dirty());
        let accepted_frame = view.presented.as_ref().unwrap().frame.clone();
        view.gif_frames_per_second = Some(24);
        view.receive(&ctx, Event::Presented(Err("preview failed".into())));
        assert!(Arc::ptr_eq(
            &accepted_frame,
            &view.presented.as_ref().unwrap().frame
        ));
        assert_eq!(
            view.gif_frames_per_second,
            Some(24),
            "failed Apply keeps the correction available"
        );
        assert_eq!(
            view.presented.as_ref().unwrap().export.frames_per_second,
            Some(8)
        );
        view.gif_frames_per_second = Some(8);
        let mut seek = opened().presented.unwrap();
        seek.export = view.export_spec();
        seek.position_ms = 1377;
        view.receive(&ctx, Event::Presented(Ok(seek)));
        assert!(!view.dirty() && !view.unapplied());
        assert_eq!(view.gif_frames_per_second, Some(8));
        view.gif = false;
        assert_eq!(
            view.export_spec().frames_per_second,
            None,
            "GIF FPS never changes MP4 cadence"
        );
        let mut mp4 = opened().presented.unwrap();
        mp4.export = view.export_spec();
        view.receive(&ctx, Event::Presented(Ok(mp4)));
        assert_eq!(
            view.gif_frames_per_second,
            Some(8),
            "MP4 acceptance retains the GIF choice"
        );
        view.gif = true;
        assert_eq!(view.export_spec().frames_per_second, Some(8));
        assert_eq!(
            opened().gif_frames_per_second,
            None,
            "new items start at the 15 FPS default"
        );
        assert!(!view.history_changed);
    }

    #[test]
    fn format_and_quality_changes_require_preview_acceptance_and_a_successful_save() {
        let mut view = opened();
        assert!(!view.dirty());
        view.gif = true;
        view.quality = QualityPreset::High;
        assert!(view.unapplied() && view.dirty());
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("preview failed".into())));
        let p = view.presented.as_ref().unwrap();
        assert!(Arc::ptr_eq(&frame, &p.frame));
        assert_eq!(p.export.format, ExportFormat::Mp4);
        assert_eq!(p.export.quality, QualityPreset::Preserve);
        assert!(view.gif && view.quality == QualityPreset::High && view.unapplied());
        let export = ExportSpec {
            format: ExportFormat::Gif,
            quality: QualityPreset::High,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                source: p.source.clone(),
                edit: p.edit.clone(),
                position_ms: p.position_ms,
                frame,
                export: export.clone(),
            })),
        );
        assert!(
            !view.unapplied() && view.dirty(),
            "format-only accepted work is unsaved"
        );
        view.receive(&ctx, Event::Saved(Err("destination exists".into())));
        assert!(view.dirty());
        view.receive(
            &ctx,
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "saved.gif".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert_eq!(view.saved_export, Some(export));
        assert!(!view.dirty());
        view.quality = QualityPreset::Standard;
        assert!(
            view.unapplied(),
            "quality-only changes also need preview acceptance"
        );
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn close_blocks_accepted_work_and_save_warning_is_not_failure() {
        let mut view = opened();
        view.busy = true;
        view.request_close();
        assert!(!view.closed && !view.confirm_close);
        let edit = EditSpec {
            trim_start_ms: 300,
            trim_end_ms: Some(2200),
            ..EditSpec::default()
        };
        view.presented.as_mut().unwrap().edit = edit.clone();
        view.start_ms = 300;
        view.end_ms = 2200;
        view.receive(
            &egui::Context::default(),
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "saved.mp4".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert_eq!(view.saved_edit, edit);
        assert!(!view.dirty() && !view.history_changed);
        assert!(view.status.as_ref().unwrap().contains("saved.mp4"));
        assert_eq!(view.error.as_deref(), Some("History unavailable"));
        view.request_close();
        assert!(view.closed);
    }

    #[test]
    fn worker_queue_accepts_only_one_operation() {
        let (tx, rx) = mpsc::channel();
        let mut view = opened();
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 900 }),
        );
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 1200 }),
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(RecordingEditorRequest::Seek {
                position_ms: 900
            }))
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn error_and_cancel_render_in_every_theme_and_cancel_keeps_work_pending() {
        for (name, tokens) in crate::tokens::load() {
            for estimating in [false, true] {
                let ctx = egui::Context::default();
                tokens.apply(&ctx, name.contains("light"));
                let mut view = opened();
                view.error = Some("Export cannot replace an existing recording.".into());
                view.busy = true;
                view.estimating = estimating;
                let cancel_label = if estimating {
                    "Cancel estimate"
                } else {
                    "Cancel export"
                };
                let cancel = CancelToken::default();
                view.cancel = Some(cancel.clone());
                let (tx, jobs) = mpsc::channel();
                let (events, _) = mpsc::channel();
                let frame = |view: &mut View, input| {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(760., 580.),
                            )),
                            events: input,
                            ..Default::default()
                        },
                        |ui| show(ui, &tokens, view, &tx, &events, egui::ViewportId::ROOT),
                    );
                    output.textures_delta.clear();
                    output
                };
                frame(&mut view, vec![]);
                let output = frame(&mut view, vec![]);
                let button = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == cancel_label => {
                            Some(text.pos + text.galley.rect.center().to_vec2())
                        }
                        _ => None,
                    })
                    .expect("cancel action is visible even with an error at minimum size");
                assert!(button.y < 580.);
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.job.text == view.error.as_ref().unwrap().as_str())));
                frame(&mut view, vec![egui::Event::PointerMoved(button)]);
                for pressed in [true, false] {
                    frame(
                        &mut view,
                        vec![egui::Event::PointerButton {
                            pos: button,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                }
                assert!(cancel.is_cancelled());
                assert!(
                    view.busy,
                    "cancel waits for the worker's publication outcome"
                );
                assert!(
                    jobs.try_recv().is_err(),
                    "cancel does not queue behind the export"
                );
            }
        }
    }
}
