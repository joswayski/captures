use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use captures_app::{Artifact, Request, Response, capture_flow::CaptureFlow};
use captures_capture::DisplayDescriptor;
use captures_settings::AppSettings;
use eframe::egui::{self, RichText};

use crate::tokens::Tokens;

enum Job {
    Execute(Request),
    Decode { generation: u64, path: PathBuf },
    Copy(PathBuf),
    Shutdown,
}

enum Reply {
    Executed(Result<Box<Response>, String>),
    Copied(Result<(), String>),
    Decoded {
        generation: u64,
        path: PathBuf,
        result: Result<Decoded, String>,
    },
}

struct Decoded {
    image: egui::ColorImage,
}

/// The nonvisual state machine is intentionally independent of egui so stale
/// worker replies can be tested without constructing a renderer.
#[derive(Default)]
struct Selection {
    generation: u64,
    id: Option<String>,
}

impl Selection {
    fn clear(&mut self) {
        self.generation += 1;
        self.id = None;
    }

    fn begin(&mut self, id: String) -> u64 {
        self.generation += 1;
        self.id = Some(id);
        self.generation
    }

    fn accepts(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

pub struct Live {
    root: PathBuf,
    tx: Sender<Job>,
    rx: Receiver<Reply>,
    worker: Option<thread::JoinHandle<()>>,
    displays: Vec<DisplayDescriptor>,
    display_id: Option<String>,
    artifacts: Vec<Artifact>,
    selection: Selection,
    texture: Option<egui::TextureHandle>,
    preview_loading: bool,
    decoded_path: Option<PathBuf>,
    status: String,
    error: Option<String>,
    pending: usize,
    capture_waiting_for_hide: bool,
    hide_started: Option<Instant>,
    hidden_since: Option<Instant>,
    capture_in_flight: bool,
    auto_copy_on_capture: bool,
    include_cursor: bool,
    flow: Option<CaptureFlow>,
    countdown_target: Option<(usize, egui::Pos2, egui::Vec2)>,
    can_hide: Option<bool>,
    confirm_delete: Option<String>,
}

impl Live {
    pub fn new(ctx: egui::Context, root: Option<PathBuf>) -> Self {
        let root = root.unwrap_or_else(captures_app::default_history_root);
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            // Retain ownership on X11. Disk decode and clipboard encoding never
            // block the UI, and the full uncompressed image is not retained by it.
            let mut clipboard = None;
            while let Ok(job) = jobs.recv() {
                let reply = match job {
                    Job::Shutdown => break,
                    Job::Execute(request) => Reply::Executed(
                        captures_app::execute(request)
                            .map(Box::new)
                            .map_err(|error| error.to_string()),
                    ),
                    Job::Decode { generation, path } => Reply::Decoded {
                        generation,
                        result: decode(&path),
                        path,
                    },
                    Job::Copy(path) => Reply::Copied(copy_image(&path, &mut clipboard)),
                };
                if out.send(reply).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        let mut live = Self {
            root,
            tx,
            rx,
            worker: Some(worker),
            displays: vec![],
            display_id: None,
            artifacts: vec![],
            selection: Selection::default(),
            texture: None,
            preview_loading: false,
            decoded_path: None,
            status: "Loading capture workspace…".into(),
            error: None,
            pending: 0,
            capture_waiting_for_hide: false,
            hide_started: None,
            hidden_since: None,
            capture_in_flight: false,
            auto_copy_on_capture: false,
            include_cursor: false,
            flow: None,
            countdown_target: None,
            can_hide: None,
            confirm_delete: None,
        };
        live.send(Request::History {
            root: live.root.clone(),
        });
        live.send(Request::Displays);
        live
    }

    pub fn is_capturing(&self) -> bool {
        self.flow.is_some() || self.capture_in_flight
    }

    pub fn flush(&mut self) {
        self.flow = None;
        // Finish accepted capture/export/delete operations before process teardown.
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn send(&mut self, request: Request) {
        self.pending += 1;
        self.error = None;
        let _ = self.tx.send(Job::Execute(request));
    }

    fn load_selected(&mut self) {
        let Some(item) = self
            .artifacts
            .iter()
            .find(|item| self.selection.id.as_deref() == Some(item.entry.id.as_str()))
        else {
            self.texture = None;
            self.decoded_path = None;
            self.preview_loading = false;
            return;
        };
        let generation = self.selection.generation;
        let _ = self.tx.send(Job::Decode {
            generation,
            path: item.image_path.clone(),
        });
    }

    fn select(&mut self, id: String) {
        self.selection.begin(id);
        self.texture = None;
        self.decoded_path = None;
        self.preview_loading = true;
        self.load_selected();
    }

    pub fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.can_hide = frame
            .winit_window()
            .map(|window| window.is_visible().is_some());
        if let Some(flow) = &self.flow {
            if !flow.is_current() {
                self.flow = None;
                self.capture_waiting_for_hide = false;
                self.auto_copy_on_capture = false;
                self.status = "Capture cancelled (Escape or desktop session unavailable).".into();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            } else {
                // Only active captures poll; settled history/preferences stay event-driven.
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if self.capture_waiting_for_hide {
            let visible = frame.winit_window().and_then(|window| window.is_visible());
            if visible == Some(false) {
                self.hidden_since.get_or_insert_with(Instant::now);
            } else {
                self.hidden_since = None;
            }
            if self
                .hidden_since
                .is_some_and(|since| since.elapsed() >= Duration::from_millis(150))
            {
                self.capture_waiting_for_hide = false;
                let Some(display_id) = self.display_id.clone() else {
                    self.flow = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    return;
                };
                let Some(flow) = &self.flow else { return };
                let generation = flow.generation();
                self.capture_in_flight = true;
                self.send(Request::CaptureDisplay {
                    root: self.root.clone(),
                    display_id,
                    generation,
                    include_cursor: self.include_cursor,
                });
            } else if self
                .hide_started
                .is_some_and(|since| since.elapsed() > Duration::from_secs(2))
            {
                self.capture_waiting_for_hide = false;
                self.flow = None;
                self.error =
                    Some("Could not hide the capture window. No screenshot was taken.".into());
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Copied(result) => {
                    self.pending = self.pending.saturating_sub(1);
                    match result {
                        Ok(()) => self.status = "Copied actual capture pixels".into(),
                        Err(error) => self.error = Some(error),
                    }
                }
                Reply::Executed(result) => {
                    self.pending = self.pending.saturating_sub(1);
                    if self.capture_in_flight {
                        self.capture_in_flight = false;
                        self.flow = None;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    }
                    match result {
                        Err(error) => self.error = Some(error),
                        Ok(response) => self.apply(*response),
                    }
                }
                Reply::Decoded {
                    generation,
                    path,
                    result,
                } if self.selection.accepts(generation) => match result {
                    Ok(decoded) => {
                        self.preview_loading = false;
                        self.texture = Some(ctx.load_texture(
                            format!("capture:{}", path.display()),
                            decoded.image,
                            egui::TextureOptions::LINEAR,
                        ));
                        self.decoded_path = Some(path);
                    }
                    Err(error) => {
                        self.preview_loading = false;
                        self.error = Some(error);
                    }
                },
                Reply::Decoded { .. } => {}
            }
        }
    }

    fn apply(&mut self, response: Response) {
        match response {
            Response::Displays { displays } => {
                self.display_id = self
                    .display_id
                    .take()
                    .filter(|id| displays.iter().any(|display| &display.id == id))
                    .or_else(|| {
                        displays
                            .iter()
                            .find(|display| display.is_primary)
                            .or(displays.first())
                            .map(|d| d.id.clone())
                    });
                self.displays = displays;
                self.status = "Displays refreshed".into();
            }
            Response::History { artifacts } => {
                self.selection.clear();
                self.texture = None;
                self.decoded_path = None;
                self.preview_loading = false;
                self.artifacts = artifacts;
                if let Some(id) = self.artifacts.first().map(|item| item.entry.id.clone()) {
                    self.select(id);
                }
                self.status = "History loaded".into();
            }
            Response::Captured { artifact } => {
                let id = artifact.entry.id.clone();
                let path = artifact.image_path.clone();
                self.artifacts.insert(0, artifact);
                self.select(id);
                self.status = "Full display captured as PNG".into();
                if std::mem::take(&mut self.auto_copy_on_capture) {
                    self.pending += 1;
                    let _ = self.tx.send(Job::Copy(path));
                }
            }
            Response::Saved { artifact, path } => {
                if let Some(item) = self
                    .artifacts
                    .iter_mut()
                    .find(|item| item.entry.id == artifact.entry.id)
                {
                    *item = artifact;
                }
                self.status = format!("Saved {}", path.display());
            }
            Response::Deleted { id } => {
                self.artifacts.retain(|item| item.entry.id != id);
                self.confirm_delete = None;
                self.selection.clear();
                self.texture = None;
                self.decoded_path = None;
                self.preview_loading = false;
                if let Some(id) = self.artifacts.first().map(|item| item.entry.id.clone()) {
                    self.select(id);
                }
                self.status = "Removed from capture history; exported files were kept".into();
            }
            Response::PermissionGranted => {
                self.status = "Capture permission granted".into();
                self.send(Request::Displays);
            }
            Response::HistoryRoot { .. } => {}
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        frame: &eframe::Frame,
        settings: impl Fn() -> Result<AppSettings, String>,
    ) {
        if let Some(flow) = &self.flow
            && !self.capture_waiting_for_hide
            && !self.capture_in_flight
        {
            let clock = flow.countdown();
            if clock.remaining(Instant::now()) > 0 {
                let t = t.clone();
                let generation = flow.generation();
                let (monitor, position, size) =
                    self.countdown_target.expect("countdown target validated");
                ui.ctx().show_viewport_deferred(
                    egui::ViewportId::from_hash_of("screenshot-countdown"),
                    egui::ViewportBuilder::default()
                        .with_title("Captures Screenshot Countdown")
                        .with_visible(true)
                        .with_monitor(monitor)
                        .with_position(position)
                        .with_inner_size(size)
                        .with_fullscreen(true)
                        .with_decorations(false)
                        .with_resizable(false)
                        .with_transparent(true)
                        .with_has_shadow(false)
                        .with_always_on_top()
                        .with_taskbar(false),
                    move |ui, _| {
                        if ui.input(|i| {
                            i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                        }) {
                            captures_app::capture_flow::cancel(generation);
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                        crate::countdown::show(ui, &t, clock.remaining(Instant::now()).max(1));
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                    },
                );
            } else {
                // Stop declaring the child before hiding the root. Hidden-root
                // logic then verifies visibility and waits for compositor settling.
                self.capture_waiting_for_hide = true;
                self.hide_started = Some(Instant::now());
                self.hidden_since = None;
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
        if self.flow.is_some() || self.capture_in_flight {
            ui.disable();
        }
        egui::Panel::top("live-header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Captures");
                ui.label(RichText::new("Native display capture").color(t.color("text-muted")));
            });
            ui.label("Full-display capture with countdown, cursor inclusion, automatic copy and save format/folder preferences. Regions, recording, editing, and mini previews are not connected yet.");
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Display")
                    .selected_text(self.displays.iter().find(|d| Some(&d.id) == self.display_id.as_ref()).map_or("No display", |d| d.name.as_str()))
                    .show_ui(ui, |ui| for display in &self.displays {
                        ui.selectable_value(&mut self.display_id, Some(display.id.clone()), format!("{} — {}×{}{}", display.name, display.width, display.height, if display.is_primary { " (Primary)" } else { "" }));
                    });
                if ui.button("Refresh displays").clicked() { self.send(Request::Displays); }
                let capture = ui.add_enabled(self.pending == 0 && self.display_id.is_some() && self.can_hide == Some(true), egui::Button::new("Capture display"));
                if capture.clicked() {
                    match settings() {
                        Ok(settings) => {
                            self.countdown_target = frame.winit_window().and_then(|window| {
                                let display = self.displays.iter().find(|d| Some(&d.id) == self.display_id.as_ref())?;
                                let (index, monitor) = window.available_monitors().enumerate().find(|(_, m)| m.position().x == display.x && m.position().y == display.y)?;
                                let scale = monitor.scale_factor();
                                let position = monitor.position().to_logical::<f32>(scale);
                                let size = monitor.size().to_logical::<f32>(scale);
                                Some((index, egui::pos2(position.x, position.y), egui::vec2(size.width, size.height)))
                            });
                            if settings.screenshot_countdown_seconds > 0 && self.countdown_target.is_none() {
                                self.error = Some("The selected display is no longer available for countdown.".into());
                            } else {
                                match CaptureFlow::begin(settings.screenshot_countdown_seconds) {
                                    Ok(flow) => {
                                        self.flow = Some(flow);
                                        self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
                                        self.include_cursor = settings.show_cursor_in_screenshots;
                                        self.status = "Preparing screenshot… Press Escape to cancel.".into();
                                        ui.ctx().request_repaint();
                                    }
                                    Err(error) => self.error = Some(format!("Could not arm capture Escape: {error}")),
                                }
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                if ui.button("Request permission").clicked() { self.send(Request::RequestPermission); }
            });
            if self.can_hide == Some(false) {
                ui.colored_label(t.color("theme-signal"), "Capture unavailable: this Wayland window backend cannot hide and verify the root window.");
            }
            if let Some(error) = &self.error { ui.colored_label(t.color("theme-signal"), error); }
            else { ui.label(RichText::new(&self.status).small().color(t.color("text-muted"))); }
        });

        egui::Panel::left("live-history")
            .resizable(true)
            .default_size(270.)
            .min_size(220.)
            .show(ui, |ui| {
                ui.heading("History");
                ui.label(
                    RichText::new(self.root.display().to_string())
                        .small()
                        .color(t.color("text-muted")),
                );
                if self.artifacts.is_empty() {
                    ui.label("No captures yet");
                }
                egui::ScrollArea::vertical().show_rows(
                    ui,
                    40.,
                    self.artifacts.len(),
                    |ui, rows| {
                        for row in rows {
                            let item = &self.artifacts[row].entry;
                            let id = item.id.clone();
                            let date = chrono::DateTime::parse_from_rfc3339(&item.created_at)
                                .map(|date| {
                                    date.with_timezone(&chrono::Local)
                                        .format("%b %d, %H:%M")
                                        .to_string()
                                })
                                .unwrap_or_else(|_| item.created_at.clone());
                            if ui
                                .add_sized(
                                    [ui.available_width(), 40.],
                                    egui::Button::new(format!(
                                        "{}×{}\n{}",
                                        item.width, item.height, date
                                    ))
                                    .wrap_mode(egui::TextWrapMode::Extend)
                                    .selected(self.selection.id.as_deref() == Some(&id)),
                                )
                                .clicked()
                            {
                                self.select(id);
                            }
                        }
                    },
                );
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let selected = self.selection.id.clone();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.decoded_path.is_some() && self.pending == 0,
                        egui::Button::new("Copy pixels"),
                    )
                    .clicked()
                {
                    self.copy();
                }
                if ui
                    .add_enabled(
                        selected.is_some() && self.pending == 0,
                        egui::Button::new("Save image"),
                    )
                    .clicked()
                    && let Some(id) = selected.clone()
                {
                    match settings() {
                        Ok(settings) => self.send(Request::SaveScreenshot {
                            root: self.root.clone(),
                            id,
                            directory: settings.output_directory.into(),
                            format: settings.screenshot_format,
                        }),
                        Err(error) => self.error = Some(error),
                    }
                }
                if ui
                    .add_enabled(
                        selected.is_some() && self.pending == 0,
                        egui::Button::new("Delete from history…"),
                    )
                    .clicked()
                {
                    self.confirm_delete = selected.clone();
                }
                let saved = self
                    .artifacts
                    .iter()
                    .find(|a| Some(&a.entry.id) == selected.as_ref())
                    .and_then(|a| a.entry.saved_path.as_deref());
                if ui
                    .add_enabled(saved.is_some(), egui::Button::new("Reveal export"))
                    .clicked()
                    && let Some(path) = saved
                    && let Err(error) = reveal(Path::new(path))
                {
                    self.error = Some(format!("Could not reveal export: {error}"));
                }
            });
            if let Some(id) = self.confirm_delete.clone() {
                ui.group(|ui| {
                    ui.label("Delete this capture from history? Exported files are never deleted.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete = None;
                        }
                        if ui.button("Delete capture").clicked() {
                            self.send(Request::Delete {
                                root: self.root.clone(),
                                id,
                            });
                        }
                    });
                });
            }
            ui.add_space(t.number("s-4"));
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(ui.available_size())
                        .maintain_aspect_ratio(true),
                );
            } else if self.preview_loading {
                ui.spinner();
            } else if selected.is_some() {
                ui.label("Preview unavailable. Select another capture or retry this one.");
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Capture a display or select a history item to preview it.");
                });
            }
        });
    }

    fn copy(&mut self) {
        if let Some(path) = &self.decoded_path {
            self.pending += 1;
            self.error = None;
            let _ = self.tx.send(Job::Copy(path.clone()));
        }
    }
}

fn decode(path: &Path) -> Result<Decoded, String> {
    let rgba = image::open(path)
        .map_err(|error| format!("Could not decode {}: {error}", path.display()))?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    let bytes = rgba.into_raw();
    Ok(Decoded {
        image: egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &bytes),
    })
}

fn copy_image(path: &Path, clipboard: &mut Option<arboard::Clipboard>) -> Result<(), String> {
    let image = image::open(path).map_err(|e| e.to_string())?.into_rgba8();
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    clipboard
        .as_mut()
        .expect("clipboard initialized")
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: Cow::Owned(image.into_raw()),
        })
        .map_err(|e| format!("Could not copy image: {e}"))
}

fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(target_os = "linux")]
    let result = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_drains_accepted_exports() {
        let root = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        let pixels = image::RgbaImage::from_pixel(7, 3, image::Rgba([21, 96, 177, 255]));
        let artifact = captures_app::persist_screenshot(
            root.path(),
            &pixels,
            captures_capture::CaptureMode::Display,
        )
        .unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.send(Request::SaveScreenshot {
            root: root.path().into(),
            id: artifact.entry.id,
            directory: exports.path().into(),
            format: captures_settings::ScreenshotFormat::Png,
        });
        live.flush();
        let entries = captures_app::list(root.path()).unwrap();
        let saved = entries[0].entry.saved_path.as_ref().unwrap();
        assert_eq!(image::open(saved).unwrap().into_rgba8(), pixels);
        live.flush();
    }

    #[test]
    fn stale_decode_is_rejected_after_selection_changes() {
        let mut selection = Selection::default();
        let old = selection.begin("old".into());
        let current = selection.begin("current".into());
        assert!(!selection.accepts(old));
        assert!(selection.accepts(current));
        assert_eq!(selection.id.as_deref(), Some("current"));
        selection.clear();
        assert!(!selection.accepts(current));
        assert_eq!(selection.id, None);
    }
}
