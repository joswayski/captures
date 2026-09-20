//! First connected screenshot editor: one serialized worker per open artifact.
//! Only snapshots and retained pixels cross to the UI; disk/render work does not.
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use captures_app::{
    editor::{Document, Element, LayerEdit, LayerPlacement, Rect},
    editor_session::{EditorSession, OpenRequest, Request},
};
use eframe::egui::{self, RichText};
use image::RgbaImage;

use crate::tokens::Tokens;

enum Job {
    Apply(Request),
    Flush(Sender<Result<(), String>>),
    Shutdown,
}

struct Presented {
    document: Arc<Document>,
    pixels: Arc<RgbaImage>,
    can_undo: bool,
    can_redo: bool,
    unsaved: bool,
    has_draft: bool,
}

impl Presented {
    fn from_session(session: &EditorSession) -> Self {
        let snapshot = session.snapshot();
        Self {
            document: Arc::new(snapshot.document.clone()),
            pixels: session.pixels(),
            can_undo: snapshot.can_undo,
            can_redo: snapshot.can_redo,
            unsaved: snapshot.unsaved_changes,
            has_draft: snapshot.has_draft,
        }
    }
}

struct View {
    presented: Option<Presented>,
    texture: Option<egui::TextureHandle>,
    crop: [f64; 4],
    canvas: [f64; 2],
    show_layers: bool,
    selected_layer: Option<String>,
    layer_name: String,
    layer_opacity: f64,
    layer_position: [f64; 2],
    pending: bool,
    closed: bool,
    close_requested: bool,
    close_after_save: bool,
    confirm_discard: bool,
    error: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            presented: None,
            texture: None,
            crop: [0., 0., 1., 1.],
            canvas: [1., 1.],
            show_layers: false,
            selected_layer: None,
            layer_name: String::new(),
            layer_opacity: 100.,
            layer_position: [0., 0.],
            pending: true,
            closed: false,
            close_requested: false,
            close_after_save: false,
            confirm_discard: false,
            error: None,
        }
    }
}

impl View {
    fn unsaved(&self) -> bool {
        self.presented.as_ref().is_some_and(|value| value.unsaved)
    }

    fn request_close(&mut self) {
        if self.pending || self.unsaved() {
            self.close_requested = true;
        } else {
            self.closed = true;
        }
    }

    fn receive(&mut self, ctx: &egui::Context, result: Result<Presented, String>) {
        if self.closed {
            return;
        }
        self.pending = false;
        match result {
            Ok(presented) => {
                let changed = self
                    .presented
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(&old.pixels, &presented.pixels));
                if changed {
                    let image = &presented.pixels;
                    self.texture = Some(ctx.load_texture(
                        "edited-screenshot",
                        egui::ColorImage::from_rgba_unmultiplied(
                            [image.width() as usize, image.height() as usize],
                            image.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    ));
                    self.canvas = [f64::from(image.width()), f64::from(image.height())];
                    self.crop = [0., 0., self.canvas[0], self.canvas[1]];
                }
                self.presented = Some(presented);
                self.select_layer(self.selected_layer.clone());
                self.error = None;
                if self.close_after_save || (self.close_requested && !self.unsaved()) {
                    self.closed = true;
                }
            }
            Err(error) => {
                self.error = Some(error);
                self.select_layer(self.selected_layer.clone());
            }
        }
        if self.close_requested && !self.unsaved() {
            self.closed = true;
        }
        self.close_after_save = false;
    }

    fn submit(&mut self, tx: &Sender<Job>, request: Request) {
        match tx.send(Job::Apply(request)) {
            Ok(()) => {
                self.pending = true;
                self.error = None;
            }
            Err(_) => {
                self.error =
                    Some("The editor worker stopped. Your last saved draft is preserved.".into())
            }
        }
    }

    fn select_layer(&mut self, id: Option<String>) {
        let elements = self
            .presented
            .as_ref()
            .map(|value| &value.document.elements);
        let layer = elements.and_then(|elements| {
            elements
                .iter()
                .find(|element| Some(&element.base().id) == id.as_ref())
                .or_else(|| elements.last())
        });
        self.selected_layer = layer.map(|element| element.base().id.clone());
        if let Some(layer) = layer {
            self.layer_name = layer_label(layer).into();
            self.layer_opacity = layer.base().opacity;
            self.layer_position = [layer.base().x, layer.base().y];
        }
    }

    fn submit_layer(&mut self, tx: &Sender<Job>, edit: LayerEdit) {
        if let Some(id) = self.selected_layer.clone() {
            self.submit(tx, Request::Layer { id, edit });
        }
    }
}

pub struct Editor {
    viewport: egui::ViewportId,
    view: Arc<Mutex<View>>,
    tx: Sender<Job>,
    rx: Receiver<Result<Presented, String>>,
    worker: Option<thread::JoinHandle<()>>,
}

fn save_dirty(session: &mut EditorSession) -> Result<(), String> {
    if session.snapshot().unsaved_changes {
        session.execute(Request::SaveDraft {
            updated_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
        })?;
    }
    Ok(())
}

fn wake(ctx: &egui::Context, viewport: egui::ViewportId) {
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.request_repaint_of(viewport);
}

impl Editor {
    pub fn open(ctx: &egui::Context, root: PathBuf, artifact_id: String) -> Self {
        let viewport = egui::ViewportId::from_hash_of(("screenshot-editor", &artifact_id));
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let wake_ctx = ctx.clone();
        let worker = thread::spawn(move || {
            let opened = EditorSession::open(OpenRequest {
                drafts_root: root.with_file_name("editor-drafts"),
                history_root: root,
                artifact_id,
            });
            let mut session = match opened {
                Ok(session) => {
                    let _ = out.send(Ok(Presented::from_session(&session)));
                    Some(session)
                }
                Err(error) => {
                    let _ = out.send(Err(error));
                    None
                }
            };
            wake(&wake_ctx, viewport);
            while let Ok(job) = jobs.recv() {
                let result = match job {
                    Job::Shutdown => break,
                    Job::Apply(request) => session
                        .as_mut()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            session.execute(request)?;
                            Ok(Presented::from_session(session))
                        }),
                    Job::Flush(reply) => {
                        let result = session.as_mut().map_or(Ok(()), save_dirty);
                        let _ = reply.send(result.clone());
                        match (result, session.as_ref()) {
                            (Ok(()), Some(session)) => Ok(Presented::from_session(session)),
                            (Err(error), _) => Err(error),
                            _ => continue,
                        }
                    }
                };
                if out.send(result).is_err() {
                    break;
                }
                wake(&wake_ctx, viewport);
            }
        });
        Self {
            viewport,
            view: Arc::new(Mutex::new(View::default())),
            tx,
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

    pub fn receive(&self, ctx: &egui::Context) {
        while let Ok(result) = self.rx.try_recv() {
            self.view.lock().unwrap().receive(ctx, result);
            ctx.request_repaint_of(self.viewport);
        }
    }

    /// Application quit drains accepted edits and saves their final draft on the
    /// worker. Failure cancels normal quit; the live session/window stays usable.
    pub fn flush(&self, ctx: &egui::Context) -> Result<(), String> {
        if self.closed() {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Job::Flush(tx))
            .map_err(|_| "Editor worker stopped.".to_owned())?;
        let result = rx.recv().map_err(|_| "Editor worker stopped.".to_owned())?;
        self.receive(ctx);
        if let Err(error) = &result {
            self.view.lock().unwrap().error =
                Some(format!("Could not save edits before quitting: {error}"));
            self.focus(ctx);
        }
        result
    }

    pub fn show(&self, ctx: &egui::Context, tokens: &Tokens) {
        if self.closed() {
            return;
        }
        let state = self.view.clone();
        let tx = self.tx.clone();
        let tokens = tokens.clone();
        let viewport = self.viewport;
        ctx.show_viewport_deferred(
            viewport,
            egui::ViewportBuilder::default()
                .with_title("Screenshot editor — Captures")
                .with_inner_size([1000., 700.])
                .with_min_inner_size([760., 540.]),
            move |ui, _| {
                let mut view = state.lock().unwrap();
                if ui.input(|input| input.viewport().close_requested()) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    view.request_close();
                }
                if view.closed {
                    wake(ui.ctx(), viewport);
                    return;
                }
                ui.push_id(viewport, |ui| show(ui, &tokens, &mut view, &tx));
                if view.closed {
                    wake(ui.ctx(), viewport);
                }
            },
        );
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.view.lock().unwrap().closed = true;
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn show(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    egui::Panel::top("editor-actions").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Screenshot editor");
            ui.label(RichText::new(if view.pending { "Working…" } else if view.unsaved() { "Unsaved edits" }
                else if view.presented.as_ref().is_some_and(|p| p.has_draft) { "Draft saved" } else { "Original screenshot" })
                .color(tokens.color("text-muted")));
        });
        ui.add_enabled_ui(!view.pending, |ui| {
            ui.horizontal(|ui| {
                if ui.add_enabled(view.presented.as_ref().is_some_and(|p| p.can_undo), egui::Button::new("Undo")).clicked() { view.submit(tx, Request::Undo); }
                if ui.add_enabled(view.presented.as_ref().is_some_and(|p| p.can_redo), egui::Button::new("Redo")).clicked() { view.submit(tx, Request::Redo); }
                if ui.add_enabled(view.presented.is_some(), egui::Button::new("Save draft")).clicked() {
                    view.submit(tx, Request::SaveDraft { updated_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64 });
                }
                if ui.add_enabled(view.presented.is_some(), egui::Button::new("Discard edits…")).clicked() { view.confirm_discard = true; }
                ui.separator();
                ui.selectable_value(&mut view.show_layers, false, "Geometry");
                ui.selectable_value(&mut view.show_layers, true, "Layers");
            });
        });
        if let Some(error) = &view.error { ui.colored_label(tokens.color("theme-signal"), error); }
        if view.close_requested && !view.pending {
            ui.group(|ui| {
                ui.label("Save unsaved edits before closing?");
                ui.horizontal(|ui| {
                    if ui.button("Save and close").clicked() {
                        view.close_after_save = true;
                        view.submit(tx, Request::SaveDraft { updated_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64 });
                    }
                    if ui.button("Close without saving").clicked() { view.closed = true; }
                    if ui.button("Cancel close").clicked() { view.close_requested = false; }
                });
                ui.small("Closing without saving keeps the last saved draft and original capture.");
            });
        }
        if view.confirm_discard {
            ui.group(|ui| {
                ui.label("Discard all edits and the saved draft? The original capture and exports stay unchanged.");
                ui.add_enabled_ui(!view.pending, |ui| ui.horizontal(|ui| {
                    if ui.button("Discard edits").clicked() { view.confirm_discard = false; view.submit(tx, Request::DiscardDraft); }
                    if ui.button("Cancel discard").clicked() { view.confirm_discard = false; }
                }));
            });
        }
    });
    egui::Panel::left("editor-geometry").resizable(false).exact_size(230.).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(view.show_layers).auto_shrink([false, false]).show(ui, |ui| {
        ui.add_enabled_ui(!view.pending && view.presented.is_some(), |ui| {
            if view.show_layers {
                show_layers(ui, view, tx);
                return;
            }
            ui.heading("Crop");
            ui.label("Coordinates in image pixels");
            egui::Grid::new("crop-fields").show(ui, |ui| {
                for (label, value) in ["X", "Y", "Width", "Height"].into_iter().zip(&mut view.crop) {
                    ui.label(label);
                    ui.add(egui::DragValue::new(value).range(0. ..=32768.).speed(1.)); ui.end_row();
                }
            });
            if ui.button("Apply crop").clicked() {
                let [x, y, width, height] = view.crop;
                view.submit(tx, Request::Crop { rect: Rect { x, y, width, height } });
            }
            ui.add_space(tokens.number("s-6"));
            ui.heading("Canvas");
            egui::Grid::new("canvas-fields").show(ui, |ui| {
                for (label, value) in ["Width", "Height"].into_iter().zip(&mut view.canvas) {
                    ui.label(label);
                    ui.add(egui::DragValue::new(value).range(1. ..=16384.).speed(1.)); ui.end_row();
                }
            });
            if ui.button("Resize canvas").clicked() {
                view.submit(tx, Request::ResizeCanvas { width: view.canvas[0], height: view.canvas[1] });
            }
        });
        ui.add_space(tokens.number("s-6"));
        ui.label(RichText::new("Native editor preview").color(tokens.color("text-muted")));
        ui.small("Geometry, layers and drafts are connected. Drawing tools and edited-image export are still in development.");
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        if let Some(texture) = &view.texture {
            ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(ui.available_size())
                    .maintain_aspect_ratio(true),
            );
        } else if view.pending {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Could not open this screenshot. See the error above.");
            });
        }
    });
}

fn layer_label(element: &Element) -> &str {
    match element {
        Element::Image(image) => &image.name,
        Element::Text(_) => "Text",
        Element::Shape(shape) => &shape.shape,
        Element::Path(_) => "Freehand",
    }
}

fn show_layers(ui: &mut egui::Ui, view: &mut View, tx: &Sender<Job>) {
    let Some(presented) = &view.presented else {
        return;
    };
    let document = presented.document.clone();
    let elements = &document.elements;
    ui.heading("Layers");
    ui.small("Front to back");
    egui::ScrollArea::vertical()
        .id_salt("layer-list")
        .max_height(112.)
        .min_scrolled_height(112.)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for element in elements.iter().rev() {
                let base = element.base();
                let label = format!(
                    "{}{}{}",
                    layer_label(element),
                    if base.locked { " · locked" } else { "" },
                    if base.visible { "" } else { " · hidden" }
                );
                let selected = view.selected_layer.as_deref() == Some(&base.id);
                if ui
                    .add_sized(
                        [ui.available_width(), 30.],
                        egui::Button::selectable(selected, &label).truncate(),
                    )
                    .on_hover_text(&label)
                    .clicked()
                {
                    view.select_layer(Some(base.id.clone()));
                }
            }
        });
    let Some(index) = elements
        .iter()
        .position(|element| Some(&element.base().id) == view.selected_layer.as_ref())
    else {
        ui.label("No layers. Undo to restore a deleted layer.");
        return;
    };
    let element = &elements[index];
    let base = element.base();
    ui.separator();
    ui.horizontal(|ui| {
        let mut visible = base.visible;
        let mut locked = base.locked;
        if ui.checkbox(&mut visible, "Visible").changed() {
            view.submit_layer(tx, LayerEdit::Visibility { visible });
        }
        if ui.checkbox(&mut locked, "Locked").changed() {
            view.submit_layer(tx, LayerEdit::Lock { locked });
        }
    });
    if matches!(element, Element::Image(_)) {
        ui.label("Name");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut view.layer_name).desired_width(132.));
            if ui.button("Rename").clicked() {
                view.submit_layer(
                    tx,
                    LayerEdit::Rename {
                        name: view.layer_name.clone(),
                    },
                );
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("Opacity");
        ui.add(
            egui::DragValue::new(&mut view.layer_opacity)
                .range(0. ..=100.)
                .suffix("%"),
        );
        if ui.button("Apply").clicked() {
            view.submit_layer(
                tx,
                LayerEdit::Opacity {
                    opacity: view.layer_opacity,
                },
            );
        }
    });
    ui.add_enabled_ui(!base.locked, |ui| {
        egui::Grid::new("layer-position").show(ui, |ui| {
            for (label, value) in ["X", "Y"].into_iter().zip(&mut view.layer_position) {
                ui.label(label);
                ui.add(
                    egui::DragValue::new(value)
                        .range(-32768. ..=32768.)
                        .speed(1.),
                );
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Move").clicked() {
                view.submit_layer(
                    tx,
                    LayerEdit::Translate {
                        delta_x: view.layer_position[0] - base.x,
                        delta_y: view.layer_position[1] - base.y,
                    },
                );
            }
            for (label, target, placement) in [
                ("Up", elements.get(index + 1), LayerPlacement::Before),
                (
                    "Down",
                    index.checked_sub(1).and_then(|index| elements.get(index)),
                    LayerPlacement::After,
                ),
            ] {
                if ui
                    .add_enabled(
                        target.is_some_and(|element| !element.base().locked),
                        egui::Button::new(label),
                    )
                    .clicked()
                {
                    view.submit_layer(
                        tx,
                        LayerEdit::Reorder {
                            target_id: target.unwrap().base().id.clone(),
                            placement,
                        },
                    );
                }
            }
        });
    });
    ui.horizontal(|ui| {
        if ui.button("Duplicate").clicked() {
            let new_id = uuid::Uuid::new_v4().to_string();
            view.submit_layer(
                tx,
                LayerEdit::Duplicate {
                    new_id: new_id.clone(),
                },
            );
            view.selected_layer = Some(new_id);
        }
        if ui
            .add_enabled(!base.locked, egui::Button::new("Delete"))
            .clicked()
        {
            view.submit_layer(tx, LayerEdit::Delete);
        }
    });
    ui.small("Locked layers stay in place. Hidden layers can still be edited.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Duration};

    fn presented(unsaved: bool) -> Presented {
        Presented {
            document: Arc::new(Document::new_capture("fixture", 7., 3., None)),
            pixels: Arc::new(RgbaImage::new(7, 3)),
            can_undo: unsaved,
            can_redo: false,
            unsaved,
            has_draft: !unsaved,
        }
    }

    #[test]
    fn layer_selection_follows_ids_and_failed_commands_restore_published_fields() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(true);
        Arc::make_mut(&mut value.document)
            .edit_layer(
                "capture-background",
                LayerEdit::Duplicate {
                    new_id: "copy".into(),
                },
            )
            .unwrap();
        view.receive(&ctx, Ok(value));
        assert_eq!(view.selected_layer.as_deref(), Some("copy"));
        assert_eq!(view.layer_position, [24., 24.]);
        let (tx, rx) = mpsc::channel();
        view.submit_layer(&tx, LayerEdit::Visibility { visible: false });
        assert!(
            matches!(rx.recv().unwrap(), Job::Apply(Request::Layer { id, edit: LayerEdit::Visibility { visible: false } }) if id == "copy")
        );
        assert!(view.pending);
        view.selected_layer = Some("rejected-duplicate".into());
        view.layer_position = [999., 999.];
        view.receive(&ctx, Err("unsupported layer".into()));
        assert_eq!(view.selected_layer.as_deref(), Some("copy"));
        assert_eq!(view.layer_position, [24., 24.]);
        assert!(!view.pending);
        view.receive(&ctx, Ok(presented(false))); // Undo removed the selected copy.
        assert_eq!(view.selected_layer.as_deref(), Some("capture-background"));
        assert_eq!(view.layer_position, [0., 0.]);
        let mut empty = presented(true);
        Arc::make_mut(&mut empty.document).elements.clear();
        view.receive(&ctx, Ok(empty));
        assert!(view.selected_layer.is_none());
    }

    #[test]
    fn close_waits_for_pending_edits_and_failed_save_keeps_the_window_recoverable() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.request_close();
        assert!(!view.closed);
        view.receive(&ctx, Ok(presented(true)));
        assert!(view.close_requested && !view.closed && view.unsaved());
        view.close_after_save = true;
        view.receive(&ctx, Err("disk full".into()));
        assert!(!view.closed && view.unsaved());
        assert_eq!(view.error.as_deref(), Some("disk full"));
        assert!(!view.close_after_save);
        view.close_after_save = true;
        view.receive(&ctx, Ok(presented(false)));
        assert!(view.closed);
        // A stale completion cannot reopen a closed controller.
        view.receive(&ctx, Ok(presented(true)));
        assert!(view.closed && !view.unsaved());
        let mut failed_open = View::default();
        failed_open.request_close();
        failed_open.receive(&ctx, Err("missing screenshot".into()));
        assert!(failed_open.closed);
    }

    fn fixture() -> (tempfile::TempDir, String) {
        let data = tempfile::tempdir().unwrap();
        let image = RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([x as u8 * 31, y as u8 * 71, 9, 255])
        });
        let artifact = captures_app::persist_screenshot(
            &data.path().join("history"),
            &image,
            captures_capture::CaptureMode::Region,
        )
        .unwrap();
        (data, artifact.entry.id)
    }

    fn receive(editor: &Editor, ctx: &egui::Context) {
        let reply = editor
            .rx
            .recv_timeout(Duration::from_secs(10))
            .expect("editor reply");
        editor.view.lock().unwrap().receive(ctx, reply);
    }

    fn crop() -> Request {
        Request::Crop {
            rect: Rect {
                x: 2.,
                y: 1.,
                width: 4.,
                height: 2.,
            },
        }
    }

    #[test]
    fn quit_flush_drains_queued_edits_and_save_failure_can_retry() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let editor = Editor::open(&ctx, data.path().join("history"), id.clone());
        receive(&editor, &ctx);
        fs::write(data.path().join("editor-drafts"), b"blocked").unwrap();
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        assert!(editor.flush(&ctx).is_err());
        assert!(!editor.closed());
        fs::remove_file(data.path().join("editor-drafts")).unwrap();
        editor.flush(&ctx).unwrap();
        drop(editor);
        let reopened = EditorSession::open(OpenRequest {
            history_root: data.path().join("history"),
            drafts_root: data.path().join("editor-drafts"),
            artifact_id: id,
        })
        .unwrap();
        assert_eq!(reopened.pixels().dimensions(), (4, 2));
        assert_eq!(reopened.pixels().get_pixel(0, 0).0, [62, 71, 9, 255]);
        assert!(reopened.snapshot().has_draft);
    }

    #[test]
    fn close_without_saving_retains_the_previous_draft_not_the_latest_edit() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let editor = Editor::open(&ctx, data.path().join("history"), id.clone());
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::SaveDraft { updated_at_ms: 42 });
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(
            &editor.tx,
            Request::ResizeCanvas {
                width: 8.,
                height: 5.,
            },
        );
        receive(&editor, &ctx);
        assert!(editor.view.lock().unwrap().unsaved());
        editor.view.lock().unwrap().closed = true;
        editor.flush(&ctx).unwrap(); // Application quit must respect the close decision.
        drop(editor);
        let reopened = EditorSession::open(OpenRequest {
            history_root: data.path().join("history"),
            drafts_root: data.path().join("editor-drafts"),
            artifact_id: id,
        })
        .unwrap();
        assert_eq!(reopened.pixels().dimensions(), (4, 2));
    }
}
