//! Frame-based recording editor. Media work stays on one serialized worker;
//! the UI never substitutes a poster or unaccepted edit for the decoded frame.
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
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
    AudioEdit, CancelToken, CropRect, CropResizeAxis, EditSpec, ExportEstimate, ExportFormat,
    ExportProgress, ExportSpec, MediaMetadata, MediaToolchain, QualityPreset,
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
    Play(u64, CancelToken),
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
    PlaybackFinished(Result<PlaybackEnd, String>),
    Destination(Option<PathBuf>),
}

struct TrimGesture {
    edge: TimelineTrimEdge,
    drag: TimelineTrimDrag,
    track: egui::Rect,
}

#[derive(Default)]
struct View {
    presented: Option<Presented>,
    texture: Option<egui::TextureHandle>,
    thumbnails: Option<egui::TextureHandle>,
    loading_thumbnails: bool,
    thumbnail_error: Option<String>,
    playing: bool,
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
            frames_per_second: None,
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

    fn request_playback(&mut self, tx: &Sender<Job>) {
        if self.busy || self.picker || self.confirm_close || self.closed || self.unapplied() {
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
        self.send(tx, Job::Play(position, cancel));
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
        let estimating = matches!(job, Job::Estimate(_));
        let loading_thumbnails = matches!(job, Job::Thumbnails(_));
        let playing = matches!(job, Job::Play(..));
        match tx.send(job) {
            Ok(()) => {
                self.busy = true;
                self.estimating = estimating;
                self.loading_thumbnails = loading_thumbnails;
                self.playing = playing;
                if playing {
                    self.playback_ended = false;
                }
                if loading_thumbnails {
                    self.thumbnail_error = None;
                }
                if estimating {
                    self.estimate = None;
                }
                self.error = None;
                self.status = None;
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
                                "Silent playback ended. Play restarts the accepted trim."
                            } else {
                                "Silent playback paused."
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
                        self.error = Some(format!("Playback failed: {error}"));
                    }
                }
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
                    Job::Play(position, cancel) => {
                        let result = (|| {
                            let session =
                                session.as_ref().ok_or("Recording editor is unavailable.")?;
                            let mut playback = session.playback(position, &cancel)?;
                            while let Some(frame) = playback.next_frame()? {
                                if cancel.is_cancelled() {
                                    break;
                                }
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
            ui.colored_label(tokens.color("theme-signal"), error);
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
                .add_enabled(!cancel.is_cancelled(), egui::Button::new(if view.playing { "Pause playback" } else if view.loading_thumbnails { "Cancel thumbnails" } else if view.estimating { "Cancel estimate" } else { "Cancel export" }))
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
            ui.horizontal(|ui| {
                ui.strong("Preview");
                ui.weak("Silent playback");
                if view.playing {
                    let pausing = view.cancel.as_ref().is_some_and(CancelToken::is_cancelled);
                    if ui.add_enabled(!pausing, egui::Button::new(if pausing { "Pausing…" } else { "Pause" }).small()).clicked() {
                        view.pause_playback();
                        ui.ctx().request_repaint();
                    }
                } else if ui.add_enabled(!view.busy && !view.picker && !view.confirm_close
                    && view.presented.is_some() && !view.unapplied(), egui::Button::new("Play").small())
                    .on_hover_text("Play accepted trim without audio. Apply staged edits first. Motion preview fits within 1280 × 720.")
                    .clicked()
                {
                    view.request_playback(tx);
                    ui.ctx().request_repaint();
                }
            });
            let width = ui.available_width();
            let height = (ui.available_height() - 210.).clamp(140., 380.);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, tokens.number("r-md"), tokens.color("surface-sunken"));
            if let Some(texture) = &view.texture {
                let size = texture.size_vec2();
                let scale = (rect.width() / size.x).min(rect.height() / size.y);
                ui.painter().image(
                    texture.id(),
                    egui::Rect::from_center_size(rect.center(), size * scale),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                    egui::Color32::WHITE,
                );
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
            let displayed_position = view.playback_position_ms.unwrap_or(p.position_ms);
            let source_size = (p.source.width, p.source.height);
            let system_audio = p.edit.audio.source_has_system_audio;
            let microphone_audio = p.edit.audio.source_has_microphone_audio;
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
                        }
                        ui.add_enabled_ui(crop_enabled, |ui| {
                            let mut locked = !view.crop_aspect_unlocked;
                            if ui.checkbox(&mut locked, "Lock aspect ratio").changed() {
                                view.crop_aspect_unlocked = !locked;
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
                        "Applied on export · frame preview is silent"
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
        let Job::Play(position, cancel) = jobs.recv().unwrap() else {
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
        assert!(matches!(jobs.recv().unwrap(), Job::Play(1100, _)));
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
            matches!(jobs.recv().unwrap(), Job::Play(123, _)),
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
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(950, _)));
        assert!(!view.dirty());
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
        assert!(matches!(jobs.recv().unwrap(), Job::Play(700, _)));
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
        let Job::Play(_, cancel) = jobs.recv().unwrap() else {
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
        assert!(matches!(jobs.recv().unwrap(), Job::Play(_, _)));
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
        let Job::Play(_, cancel) = jobs.recv().unwrap() else {
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
    fn fixed_pause_is_clickable_while_worker_owns_the_minimum_window() {
        for (name, tokens) in crate::tokens::load() {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            let (tx, jobs) = mpsc::channel();
            let (events, _) = mpsc::channel();
            view.request_playback(&tx);
            let Job::Play(_, cancel) = jobs.recv().unwrap() else {
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
