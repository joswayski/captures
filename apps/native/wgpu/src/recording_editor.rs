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
    RecordingSaveRequest, SavedRecording,
};
use captures_media::{
    CancelToken, EditSpec, ExportFormat, ExportProgress, ExportSpec, MediaMetadata, MediaToolchain,
    QualityPreset,
};
use eframe::egui;
use image::RgbaImage;

use crate::tokens::Tokens;

struct Presented {
    source: MediaMetadata,
    edit: EditSpec,
    position_ms: u64,
    frame: Arc<RgbaImage>,
}

impl Presented {
    fn from_session(session: &RecordingEditorSession) -> Self {
        let snapshot = session.snapshot();
        Self {
            source: snapshot.source.clone(),
            edit: snapshot.edit.clone(),
            position_ms: snapshot.position_ms,
            frame: session.frame(),
        }
    }
}

enum Job {
    Apply(RecordingEditorRequest),
    Save(RecordingSaveRequest, CancelToken),
    Shutdown,
}

enum Event {
    Presented(Result<Presented, String>),
    Progress(ExportProgress),
    Saved(Result<SavedRecording, String>),
    Destination(Option<PathBuf>),
}

#[derive(Default)]
struct View {
    presented: Option<Presented>,
    texture: Option<egui::TextureHandle>,
    busy: bool,
    cancel: Option<CancelToken>,
    picker: bool,
    closed: bool,
    confirm_close: bool,
    history_changed: bool,
    saved_edit: EditSpec,
    start_ms: u64,
    end_ms: u64,
    position_ms: u64,
    destination: String,
    gif: bool,
    quality: QualityPreset,
    progress: Option<ExportProgress>,
    status: Option<String>,
    error: Option<String>,
}

impl View {
    fn unapplied(&self) -> bool {
        self.presented.as_ref().is_some_and(|p| {
            self.start_ms != p.edit.trim_start_ms
                || self.end_ms
                    != p.edit
                        .trim_end_ms
                        .unwrap_or(p.source.duration_ms.unwrap_or(0))
        })
    }

    fn dirty(&self) -> bool {
        self.unapplied()
            || self
                .presented
                .as_ref()
                .is_some_and(|p| p.edit != self.saved_edit)
    }

    fn send(&mut self, tx: &Sender<Job>, job: Job) {
        if self.busy || self.picker {
            return;
        }
        match tx.send(job) {
            Ok(()) => {
                self.busy = true;
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
                        let frame = egui::ColorImage::from_rgba_unmultiplied(
                            [p.frame.width() as usize, p.frame.height() as usize],
                            p.frame.as_raw(),
                        );
                        self.texture = Some(ctx.load_texture(
                            "recording-editor-frame",
                            frame,
                            egui::TextureOptions::LINEAR,
                        ));
                        self.start_ms = p.edit.trim_start_ms;
                        self.end_ms = p
                            .edit
                            .trim_end_ms
                            .unwrap_or(p.source.duration_ms.unwrap_or(0));
                        self.position_ms = p.position_ms;
                        // The initial edit includes trusted audio flags.
                        if self.presented.is_none() {
                            self.saved_edit = p.edit.clone();
                        }
                        self.presented = Some(p);
                    }
                    Err(error) => {
                        // Failed commands retain the accepted frame. A failed seek
                        // must not label that frame with the rejected position.
                        if let Some(p) = &self.presented {
                            self.position_ms = p.position_ms;
                        }
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
                        }
                        self.status = Some(format!("Saved new copy: {}", path.display()));
                        self.error = warning;
                    }
                    Err(error) => self.error = Some(error),
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
        if self.busy || self.picker {
            self.error = Some(
                "Wait for the current operation, or cancel the export, before closing.".into(),
            );
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
            self.view.lock().unwrap().receive(ctx, event);
            wake(ctx, self.viewport);
        }
    }

    pub fn flush(&self, ctx: &egui::Context) -> Result<(), String> {
        self.receive(ctx);
        let mut view = self.view.lock().unwrap();
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
                .add_enabled(!cancel.is_cancelled(), egui::Button::new("Cancel export"))
                .clicked()
        {
            cancel.cancel();
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
                ui.label("Original is never replaced");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            view.presented.is_some() && !view.unapplied(),
                            egui::Button::new("Save new copy"),
                        )
                        .clicked()
                    {
                        let cancel = CancelToken::default();
                        view.cancel = Some(cancel.clone());
                        view.send(
                            tx,
                            Job::Save(
                                RecordingSaveRequest {
                                    destination: view.destination.clone().into(),
                                    export: ExportSpec {
                                        format: if view.gif {
                                            ExportFormat::Gif
                                        } else {
                                            ExportFormat::Mp4
                                        },
                                        quality: view.quality,
                                        max_size_bytes: None,
                                        frames_per_second: None,
                                        gif_max_colors: None,
                                    },
                                },
                                cancel,
                            ),
                        );
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
                ui.weak("Frame preview · playback not implemented");
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
            let accepted_position = p.position_ms;
            let accepted_edit = p.edit.clone();
            ui.label(format!(
                "Source frame: {:.3}s / {:.3}s · {} × {}",
                accepted_position as f64 / 1000.,
                duration as f64 / 1000.,
                p.source.width,
                p.source.height
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
                        if seek && view.position_ms != accepted_position {
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
                    ui.strong("Trim");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Start (ms)");
                        ui.add(egui::DragValue::new(&mut view.start_ms).range(0..=duration));
                        ui.label("End (ms)");
                        ui.add(egui::DragValue::new(&mut view.end_ms).range(0..=duration));
                        ui.label(format!(
                            "{:.3}s selected",
                            view.end_ms.saturating_sub(view.start_ms) as f64 / 1000.
                        ));
                        if ui
                            .add_enabled(view.unapplied(), egui::Button::new("Apply trim"))
                            .clicked()
                        {
                            view.send(
                                tx,
                                Job::Apply(RecordingEditorRequest::UpdateEdit {
                                    edit: EditSpec {
                                        trim_start_ms: view.start_ms,
                                        trim_end_ms: (view.end_ms != duration)
                                            .then_some(view.end_ms),
                                        ..accepted_edit.clone()
                                    },
                                }),
                            );
                        }
                        if ui.button("Reset trim").clicked() {
                            view.start_ms = 0;
                            view.end_ms = duration;
                        }
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
                if view.unapplied() {
                    ui.weak(
                        "Apply trim before scrubbing or saving. Closing discards unapplied values.",
                    );
                }
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
                position_ms: 700,
                frame: Arc::new(RgbaImage::new(4, 2)),
            })),
        );
        view
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
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            view.error = Some("Export cannot replace an existing recording.".into());
            view.busy = true;
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
                    egui::Shape::Text(text) if text.galley.job.text == "Cancel export" => {
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
