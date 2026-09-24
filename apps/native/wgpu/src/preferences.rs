use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use captures_app::shortcuts::{
    ShortcutKeyEvent, ShortcutPlatform, ShortcutRecording, record_shortcut, shortcut_display_tokens,
};
use captures_settings::{AppSettings, theme::normalize_hex_color};
use eframe::egui::{self, RichText, Stroke};
use serde_json::{Value, json};

use crate::{
    shortcut_input,
    tokens::{self, Tokens},
};

const SECTIONS: [&str; 7] = [
    "Appearance",
    "Capture",
    "Shortcuts",
    "Recording",
    "GIF export",
    "Updates",
    "About",
];
const THEMES: [(&str, &str, &str); 10] = [
    ("mustard", "Mustard", "Captures mustard and signal red"),
    ("ember", "Ember", "Warm orange and electric pink"),
    ("rose", "Rose", "Bright rose and coral"),
    ("violet", "Violet", "Orchid violet and raspberry"),
    ("cobalt", "Cobalt", "True blue and coral"),
    ("aqua", "Aqua", "Clear cyan and watermelon"),
    ("mint", "Mint", "Fresh mint and vermilion"),
    ("lime", "Lime", "Crisp lime and vermilion"),
    ("mono", "Mono", "Vercel-like black and white"),
    ("custom", "Custom", "Build your own RGB palette"),
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ShortcutField {
    NewCapture,
    Region,
    Window,
    Display,
    RecordRegion,
    RecordWindow,
    RecordDisplay,
}

const SHORTCUT_FIELDS: [ShortcutField; 7] = [
    ShortcutField::NewCapture,
    ShortcutField::Region,
    ShortcutField::Window,
    ShortcutField::Display,
    ShortcutField::RecordRegion,
    ShortcutField::RecordWindow,
    ShortcutField::RecordDisplay,
];

impl ShortcutField {
    fn label(self) -> &'static str {
        match self {
            Self::NewCapture => "New Capture",
            Self::Region => "Region",
            Self::Window => "Window",
            Self::Display => "Full Screen",
            Self::RecordRegion => "Record Region",
            Self::RecordWindow => "Record Window",
            Self::RecordDisplay => "Record Full Screen",
        }
    }

    fn path(self) -> &'static [&'static str] {
        match self {
            Self::NewCapture => &["new_capture_shortcut"],
            Self::Region => &["region_shortcut"],
            Self::Window => &["window_shortcut"],
            Self::Display => &["display_shortcut"],
            Self::RecordRegion => &["recording", "video_shortcut"],
            Self::RecordWindow => &["recording", "window_shortcut"],
            Self::RecordDisplay => &["recording", "display_shortcut"],
        }
    }
}

fn shortcut_scope_id(field: ShortcutField) -> egui::Id {
    egui::Id::unique(("preferences-shortcut-recorder", field))
}

#[derive(Debug)]
struct ShortcutRecorder {
    field: ShortcutField,
    keys: Vec<String>,
    error: Option<String>,
    super_held: bool,
}

impl ShortcutRecorder {
    fn new(field: ShortcutField) -> Self {
        Self {
            field,
            keys: Vec::new(),
            error: None,
            super_held: false,
        }
    }
}

enum Command {
    Save(u64, Box<AppSettings>),
    Load,
    Flush,
}
enum Message {
    Loaded(Result<AppSettings, String>),
    Saved(u64, Result<AppSettings, String>),
    Folder(Option<PathBuf>),
}

/// One owner serializes disk operations. Closing the window flushes the newest
/// queued edit, including one still inside the debounce window.
struct SettingsIo {
    tx: Sender<Command>,
    worker: Option<JoinHandle<()>>,
}
impl SettingsIo {
    fn start(path: PathBuf, out: Sender<Message>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let send_load = || {
                let _ = out.send(Message::Loaded(
                    captures_settings::load(&path).map_err(|e| e.to_string()),
                ));
                wake();
            };
            send_load();
            while let Ok(command) = rx.recv() {
                match command {
                    Command::Load => send_load(),
                    Command::Flush => break,
                    Command::Save(mut revision, mut settings) => {
                        let mut finish = false;
                        loop {
                            match rx.recv_timeout(Duration::from_millis(250)) {
                                Ok(Command::Save(next_revision, next)) => {
                                    revision = next_revision;
                                    settings = next;
                                }
                                Ok(Command::Flush) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                                    finish = true;
                                    break;
                                }
                                Ok(Command::Load) => {}
                                Err(mpsc::RecvTimeoutError::Timeout) => break,
                            }
                        }
                        let result =
                            captures_settings::save(&path, &settings).map_err(|e| e.to_string());
                        let _ = out.send(Message::Saved(revision, result));
                        wake();
                        if finish {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            tx,
            worker: Some(worker),
        }
    }
    fn flush(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.tx.send(Command::Flush);
            let _ = worker.join();
        }
    }
}
impl Drop for SettingsIo {
    fn drop(&mut self) {
        self.flush();
    }
}

pub struct Preferences {
    value: Value,
    load_error: Option<String>,
    save_error: Option<String>,
    saving: bool,
    saved_until: Option<Instant>,
    revision: u64,
    io: SettingsIo,
    rx: Receiver<Message>,
    out: Sender<Message>,
    overrides: Option<(Option<String>, Option<String>)>,
    find_open: bool,
    query: String,
    matches: Vec<egui::Rect>,
    match_index: usize,
    find_jump: bool,
    section_jump: Option<usize>,
    active_section: usize,
    custom_accent: String,
    custom_signal: String,
    folder_open: bool,
    variants: std::collections::BTreeMap<String, Tokens>,
    persisted_generation: u64,
    shortcut_recorder: Option<ShortcutRecorder>,
    shortcut_input: shortcut_input::Bridge,
    suppress_shortcut_commands: bool,
    feedback: crate::feedback::Feedback,
}

impl Preferences {
    #[cfg(test)]
    pub fn new(
        ctx: egui::Context,
        path: PathBuf,
        appearance: Option<String>,
        theme: Option<String>,
    ) -> Self {
        Self::new_with_shortcut_input(
            ctx,
            path,
            appearance,
            theme,
            shortcut_input::Bridge::default(),
        )
    }

    pub fn new_with_shortcut_input(
        ctx: egui::Context,
        path: PathBuf,
        appearance: Option<String>,
        theme: Option<String>,
        shortcut_input: shortcut_input::Bridge,
    ) -> Self {
        shortcut_input.attach(ctx.clone());
        let (out, rx) = mpsc::channel();
        let io = SettingsIo::start(path, out.clone(), move || ctx.request_repaint());
        Self {
            value: Value::Null,
            load_error: None,
            save_error: None,
            saving: false,
            saved_until: None,
            revision: 0,
            io,
            rx,
            out,
            overrides: Some((appearance, theme)),
            find_open: false,
            query: String::new(),
            matches: vec![],
            match_index: 0,
            find_jump: false,
            section_jump: None,
            active_section: 0,
            custom_accent: String::new(),
            custom_signal: String::new(),
            folder_open: false,
            variants: tokens::load(),
            persisted_generation: 0,
            shortcut_recorder: None,
            shortcut_input,
            suppress_shortcut_commands: false,
            feedback: crate::feedback::Feedback::default(),
        }
    }

    pub fn appearance_theme(&self) -> Option<(String, String)> {
        Some((
            self.value.get("appearance")?.as_str()?.into(),
            self.value.get("theme")?.as_str()?.into(),
        ))
    }
    pub fn is_loading(&self) -> bool {
        self.value.is_null() && self.load_error.is_none()
    }

    pub fn snapshot(&self) -> Result<AppSettings, String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        serde_json::from_value(self.value.clone())
            .map_err(|error| format!("Capture settings are not available: {error}"))
    }
    pub fn custom_colors(&self) -> Option<(String, String)> {
        (string_at(&self.value, &["theme"]) == "custom").then(|| {
            (
                string_at(&self.value, &["custom_theme", "accent"]),
                string_at(&self.value, &["custom_theme", "signal"]),
            )
        })
    }
    pub fn flush(&mut self) {
        self.io.flush();
    }

    pub fn persisted_generation(&self) -> u64 {
        self.persisted_generation
    }

    pub fn is_recording_shortcut(&self) -> bool {
        self.shortcut_recorder.is_some()
    }

    pub fn set_presented(&mut self, presented: bool) {
        if !presented {
            self.cancel_shortcut_recording();
        }
    }

    pub fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Loaded(Ok(settings)) => {
                    self.value = serde_json::to_value(settings).expect("settings serialize");
                    if let Some((appearance, theme)) = self.overrides.take() {
                        if let Some(value) = appearance {
                            set(&mut self.value, &["appearance"], json!(value));
                        }
                        if let Some(value) = theme {
                            set(&mut self.value, &["theme"], json!(value));
                        }
                    }
                    self.sync_colors();
                    self.load_error = None;
                    self.persisted_generation = self.persisted_generation.wrapping_add(1);
                }
                Message::Loaded(Err(error)) => self.load_error = Some(error),
                Message::Saved(revision, result) => {
                    if revision != self.revision {
                        continue;
                    }
                    self.saving = false;
                    match result {
                        Ok(settings) => {
                            self.value =
                                serde_json::to_value(settings).expect("settings serialize");
                            self.save_error = None;
                            self.saved_until = Some(Instant::now() + Duration::from_secs(2));
                            self.persisted_generation = self.persisted_generation.wrapping_add(1);
                            ctx.request_repaint_after(Duration::from_secs(2));
                        }
                        Err(error) => self.save_error = Some(error),
                    }
                }
                Message::Folder(path) => {
                    self.folder_open = false;
                    if let Some(path) = path {
                        self.set(&["output_directory"], json!(path.to_string_lossy()));
                    }
                }
            }
        }
    }
    fn sync_colors(&mut self) {
        self.custom_accent = string_at(&self.value, &["custom_theme", "accent"]);
        self.custom_signal = string_at(&self.value, &["custom_theme", "signal"]);
    }
    fn set(&mut self, path: &[&str], value: Value) {
        set(&mut self.value, path, value);
        self.changed();
    }
    fn changed(&mut self) {
        match serde_json::from_value::<AppSettings>(self.value.clone()) {
            Ok(settings) => {
                self.revision += 1;
                self.saving = true;
                self.saved_until = None;
                self.save_error = None;
                if self
                    .io
                    .tx
                    .send(Command::Save(self.revision, Box::new(settings)))
                    .is_err()
                {
                    self.saving = false;
                    self.save_error =
                        Some("Settings writer is unavailable. Reopen Preferences to retry.".into());
                }
            }
            Err(error) => self.save_error = Some(error.to_string()),
        }
    }
    pub fn exercise(&mut self, cycle: usize) {
        if !self.value.is_null() {
            self.set(
                &["appearance"],
                json!(if cycle.is_multiple_of(2) {
                    "light"
                } else {
                    "dark"
                }),
            );
        }
    }

    pub fn sidebar(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        for (index, title) in SECTIONS.iter().enumerate() {
            let response = ui.add_sized(
                [ui.available_width(), t.number("h-md")],
                egui::Button::new(*title)
                    .frame(false)
                    .selected(self.active_section == index),
            );
            if response.clicked() {
                self.feedback.open = false;
                self.section_jump = Some(index);
                self.active_section = index;
            }
        }
    }

    /// Returns true when the user requests the history window.
    pub fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens, live: bool) -> bool {
        if self.feedback.open {
            self.feedback.ui(ui, t, live);
            return false;
        }
        self.receive_shortcut_input();
        self.keyboard(ui);
        let mut history = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("Preferences");
                ui.label(
                    RichText::new("Changes save automatically.")
                        .small()
                        .color(t.color("text-muted")),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                history = ui.button("Capture History…").clicked();
                if self.saving {
                    ui.label("Saving changes…");
                } else if self.saved_until.is_some_and(|until| Instant::now() < until) {
                    ui.colored_label(t.color("positive-text"), "✓ Changes saved");
                }
            });
        });
        if let Some(error) = self.load_error.clone() {
            ui.colored_label(
                t.color("danger-text"),
                format!("Couldn’t load preferences: {error}"),
            );
            ui.label("The file was left unchanged. Correct it, then retry.");
            if ui.button("Retry loading").clicked() {
                let _ = self.io.tx.send(Command::Load);
            }
            return history;
        }
        if self.value.is_null() {
            ui.label("Loading preferences…");
            return history;
        }
        if let Some(error) = self.save_error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    t.color("danger-text"),
                    format!("Couldn’t save changes: {error}"),
                );
                if ui.button("Retry").clicked() {
                    self.changed();
                }
            });
        }
        if self.find_open {
            self.find_bar(ui);
        }
        ui.add_space(t.number("s-6"));
        egui::ScrollArea::vertical()
            .id_salt("preferences-scroll")
            .show(ui, |ui| {
                ui.set_max_width(664.);
                self.matches.clear();
                self.appearance(ui, t);
                self.capture(ui, t);
                self.shortcuts(ui, t);
                self.recording(ui, t);
                self.gif(ui, t);
                self.updates(ui, t);
                self.about(ui, t);
                self.match_index = self.match_index.min(self.matches.len().saturating_sub(1));
                for (index, rect) in self.matches.iter().enumerate() {
                    ui.painter().rect_stroke(
                        *rect,
                        t.number("r-sm"),
                        Stroke::new(
                            if index == self.match_index { 2. } else { 1. },
                            t.color("theme-accent"),
                        ),
                        egui::StrokeKind::Inside,
                    );
                }
                if self.find_jump {
                    if let Some(rect) = self.matches.get(self.match_index) {
                        ui.scroll_to_rect(*rect, Some(egui::Align::Center));
                    }
                    self.find_jump = false;
                }
                ui.add_space(t.number("s-12"));
            });
        history
    }

    fn keyboard(&mut self, ui: &egui::Ui) {
        if self.is_recording_shortcut() || self.suppress_shortcut_commands {
            return;
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.find_open = true;
            ui.memory_mut(|m| m.request_focus(egui::Id::unique("settings-find")));
        }
        if self.find_open
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.find_open = false;
            self.query.clear();
        }
        if self.find_open
            && ui.input(|i| {
                i.key_pressed(egui::Key::F3)
                    || (i.key_pressed(egui::Key::G) && i.modifiers.command)
                    || i.key_pressed(egui::Key::Enter)
            })
        {
            self.step_match(if ui.input(|i| i.modifiers.shift) {
                -1
            } else {
                1
            });
        }
    }
    fn find_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::unique("settings-find"))
                        .hint_text("Find settings")
                        .desired_width(300.),
                )
                .changed()
            {
                self.match_index = 0;
                self.find_jump = true;
                ui.ctx().request_repaint();
            }
            ui.label(if self.query.trim().is_empty() {
                String::new()
            } else if self.matches.is_empty() {
                "No results".into()
            } else {
                format!("{} of {}", self.match_index + 1, self.matches.len())
            });
            if ui
                .add_enabled(!self.matches.is_empty(), egui::Button::new("↑"))
                .on_hover_text("Previous match")
                .clicked()
            {
                self.step_match(-1);
            }
            if ui
                .add_enabled(!self.matches.is_empty(), egui::Button::new("↓"))
                .on_hover_text("Next match")
                .clicked()
            {
                self.step_match(1);
            }
            if ui.button("×").on_hover_text("Close find").clicked() {
                self.find_open = false;
                self.query.clear();
            }
        });
    }
    fn step_match(&mut self, delta: isize) {
        if !self.matches.is_empty() {
            self.match_index = (self.match_index as isize + delta)
                .rem_euclid(self.matches.len() as isize) as usize;
            self.find_jump = true;
        }
    }
    fn remember(&mut self, text: &str, response: &egui::Response) {
        let query = self.query.trim().to_lowercase();
        if self.find_open && !query.is_empty() && text.to_lowercase().contains(&query) {
            self.matches.push(response.rect);
        }
    }
    fn card(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        index: usize,
        title: &str,
        description: &str,
        add: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let response = egui::Frame::new()
            .fill(t.color("surface-raised"))
            .stroke(Stroke::new(1., t.color("border-subtle")))
            .corner_radius(t.number("r-xl") as u8)
            .inner_margin(t.number("s-6") as i8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let heading = ui
                    .vertical(|ui| {
                        ui.label(RichText::new(title).size(t.number("text-lg")).strong());
                        ui.label(
                            RichText::new(description)
                                .small()
                                .color(t.color("text-subtle")),
                        );
                    })
                    .response;
                self.remember(&format!("{title} {description}"), &heading);
                ui.add_space(t.number("s-4"));
                add(self, ui);
            })
            .response;
        if self.section_jump == Some(index) {
            response.scroll_to_me(Some(egui::Align::Min));
            self.section_jump = None;
        }
        if response
            .rect
            .contains(ui.clip_rect().center_top() + egui::vec2(0., 24.))
        {
            self.active_section = index;
        }
        ui.add_space(t.number("s-6"));
    }
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        desc: &str,
        control: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let response = ui
            .horizontal(|ui| {
                let width = (ui.available_width() - 240.).max(150.);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label(title);
                        if !desc.is_empty() {
                            ui.small(desc);
                        }
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    control(self, ui)
                });
            })
            .response;
        self.remember(&format!("{title} {desc}"), &response);
    }
    fn toggle(&mut self, ui: &mut egui::Ui, path: &[&str], title: &str, desc: &str, enabled: bool) {
        ui.add_enabled_ui(enabled, |ui| {
            self.row(ui, title, desc, |this, ui| {
                let mut value = at(&this.value, path)
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(34., 20.), egui::Sense::click());
                let response = response.on_hover_text(title);
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, value, title)
                });
                if response.clicked() {
                    value = !value;
                    this.set(path, json!(value));
                }
                let visuals = ui.style().interact(&response);
                let background = if value {
                    ui.visuals().text_color()
                } else {
                    visuals.bg_fill
                };
                ui.painter().rect(
                    rect,
                    10.,
                    background,
                    visuals.bg_stroke,
                    egui::StrokeKind::Inside,
                );
                ui.painter().circle_filled(
                    egui::pos2(
                        if value {
                            rect.right() - 10.
                        } else {
                            rect.left() + 10.
                        },
                        rect.center().y,
                    ),
                    7.,
                    if value {
                        ui.visuals().panel_fill
                    } else {
                        ui.visuals().text_color()
                    },
                );
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        rect.expand(2.),
                        12.,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Outside,
                    );
                }
            })
        });
    }
    fn combo(
        &mut self,
        ui: &mut egui::Ui,
        path: &[&str],
        title: &str,
        desc: &str,
        options: &[(Value, String)],
    ) {
        let old = at(&self.value, path).cloned().unwrap_or(Value::Null);
        let mut value = old.clone();
        self.row(ui, title, desc, |_, ui| {
            egui::ComboBox::from_id_salt(path.join("."))
                .width(160.)
                .selected_text(
                    options
                        .iter()
                        .find(|(v, _)| v == &value)
                        .map(|(_, s)| s.clone())
                        .unwrap_or_else(|| value.to_string()),
                )
                .show_ui(ui, |ui| {
                    for (v, label) in options {
                        ui.selectable_value(&mut value, v.clone(), label);
                    }
                });
        });
        if value != old {
            self.set(path, value);
        }
    }
    fn appearance(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui, t, 0, "Appearance", "One look across every Captures window. Capture overlays stay dark so they read on any desktop.", |this, ui| {
            this.row(ui, "Interface theme", "Follow the system setting, or lock Captures to light or dark.", |this, ui| {
                ui.horizontal(|ui| for (id,label) in [("system","System"),("light","Light"),("dark","Dark")] {
                    if ui.selectable_label(string_at(&this.value,&["appearance"]) == id, label).clicked() { this.set(&["appearance"], json!(id)); }
                });
            });
            ui.separator();
            let label = ui.label("Accent color"); this.remember("Accent color capture action selection focus", &label);
            ui.small("Used for the capture action, selection, and focus. Status colors keep their meaning.");
            let width = (ui.available_width() - ui.spacing().item_spacing.x * 4.) / 5.;
            for chunk in THEMES.chunks(5) {
                ui.horizontal(|ui| for (id,name,description) in chunk {
                    let selected = string_at(&this.value, &["theme"]) == *id;
                    let response = ui.add_sized([width, 34.], egui::Button::new(format!("     {name}")).frame(selected));
                    let swatch = egui::Rect::from_min_size(response.rect.left_center() + egui::vec2(6.,-9.), egui::vec2(18.,18.));
                    let palette = this.variants.get(&format!("dark-{id}")).unwrap_or(t);
                    ui.painter().rect_filled(swatch, t.number("r-sm"), palette.color("theme-accent"));
                    ui.painter().add(egui::Shape::convex_polygon(vec![swatch.right_top(), swatch.right_bottom(), swatch.left_bottom()], palette.color("theme-signal"), Stroke::NONE));
                    if response.clicked() { this.set(&["theme"], json!(id)); }
                    this.remember(&format!("{name} {description}"), &response);
                    response.on_hover_text(*description);
                });
            }
            if string_at(&this.value, &["theme"]) == "custom" {
                ui.separator();
                ui.horizontal(|ui| { ui.strong("Custom colors"); if ui.button("Reset colors").clicked() {
                    this.custom_accent = "#32d3ff".into(); this.custom_signal = "#ff4fc3".into(); this.commit_colors();
                }});
                ui.small("Open either RGB picker or enter a hex value. Supporting shades stay readable.");
                this.color_editor(ui, "Accent", "accent");
                this.color_editor(ui, "Recording signal", "signal");
            }
        });
    }
    fn color_editor(&mut self, ui: &mut egui::Ui, title: &str, key: &str) {
        let old = string_at(&self.value, &["custom_theme", key]);
        let normalized = normalize_hex_color(&old).unwrap_or_else(|_| "#000000".into());
        let mut rgb = [0u8; 3];
        for (i, value) in rgb.iter_mut().enumerate() {
            *value =
                u8::from_str_radix(&normalized[1 + i * 2..3 + i * 2], 16).expect("normalized hex");
        }
        let response = ui
            .horizontal(|ui| {
                ui.label(title);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    let hex = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                    if key == "accent" {
                        self.custom_accent = hex;
                    } else {
                        self.custom_signal = hex;
                    }
                    self.commit_colors();
                }
                let field = if key == "accent" {
                    &mut self.custom_accent
                } else {
                    &mut self.custom_signal
                };
                let response = ui.add(egui::TextEdit::singleline(field).desired_width(130.));
                let escape = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
                if escape {
                    *field = old.clone();
                    response.surrender_focus();
                } else if response.lost_focus()
                    || response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    *field = normalize_hex_color(field).unwrap_or(old);
                    response.surrender_focus();
                    self.commit_colors();
                }
            })
            .response;
        self.remember(title, &response);
    }
    fn commit_colors(&mut self) {
        if let (Ok(accent), Ok(signal)) = (
            normalize_hex_color(&self.custom_accent),
            normalize_hex_color(&self.custom_signal),
        ) {
            let next = json!({"accent":accent,"signal":signal});
            if self.value["custom_theme"] != next {
                self.set(&["custom_theme"], next);
            }
        }
    }
    fn capture(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui,t,1,"Capture","Where captures go and what happens right after you take one.",|this,ui| {
            let response = ui.label("Save captures to"); this.remember("Save captures to folder output directory", &response);
            ui.horizontal(|ui| {
                let mut path = string_at(&this.value,&["output_directory"]);
                if ui.add(egui::TextEdit::singleline(&mut path).desired_width(ui.available_width()-100.)).changed() { this.set(&["output_directory"],json!(path)); }
                if ui.add_enabled(!this.folder_open,egui::Button::new("Choose…")).clicked() {
                    this.folder_open = true;
                    let out = this.out.clone(); let ctx = ui.ctx().clone();
                    thread::spawn(move || { let result = rfd::FileDialog::new().set_title("Choose capture folder").pick_folder(); let _=out.send(Message::Folder(result)); ctx.request_repaint(); });
                }
            });
            ui.separator();
            this.toggle(ui,&["auto_copy_to_clipboard"],"Automatically copy captures to the clipboard","Turn this off to preserve existing text or other clipboard contents.",true);
            ui.separator();
            this.toggle(ui,&["auto_start_on_selection"],"Start capture as soon as a target is selected","Drawing a region, choosing a window, or clicking Full screen immediately starts the capture. When this is off, press Enter in the capture menu to confirm.",true);
            ui.separator();
            this.toggle(ui,&["show_mini_previews"],"Show mini previews after screenshots","Turn this off to keep the quick-access preview stack hidden.",true);
            let previews = this.value["show_mini_previews"].as_bool().unwrap_or(false);
            ui.add_enabled_ui(previews,|ui| this.combo(ui,&["mini_preview_placement"],"Mini preview position","Choose a screen corner. The stack opens away from it.",&choices(&[("bottom_left","Bottom left"),("bottom_right","Bottom right"),("top_left","Top left"),("top_right","Top right")])));
            ui.separator();
            this.toggle(ui,&["include_mini_previews_in_captures"],"Show mini previews in screenshots and recordings",if previews { "Turn this off to keep mini previews out of captures." } else { "Mini previews are off, so they won’t show in screenshots or recordings." },previews);
            ui.separator();
            this.toggle(ui,&["include_recording_controls_in_captures"],"Show recording controls in screenshots and recordings",if cfg!(target_os="linux") { "This desktop session cannot keep recording controls out of screenshots and recordings. Use Hide controls on the recording bar to keep them off-screen." } else { "Turn this off to keep recording controls out of captures." },!cfg!(target_os="linux"));
            ui.separator();
            this.toggle(ui,&["freeze_screen"],"Freeze screen when capturing","Holds hover states, tooltips, menus, and motion still while you choose a region or window. Turn this off to select from the live desktop.",true);
            ui.separator();
            this.toggle(ui,&["show_cursor_in_screenshots"],"Show cursor in screenshots","Includes the pointer in still captures. Freeze screen only holds the desktop still; it does not add the cursor by itself.",true);
            ui.separator();
            this.combo(ui,&["screenshot_format"],"Screenshot format","Used when you save or export. Capture History keeps a lossless PNG until then.",&choices(&[("png","PNG"),("jpeg","JPEG"),("webp","WebP")]));
            ui.separator();
            this.combo(ui,&["screenshot_countdown_seconds"],"Screenshot countdown","Wait before capturing so you can open menus or hover states. Press Esc to cancel.",&countdowns());
        });
    }
    fn shortcuts(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui,t,2,"Shortcuts","Select a shortcut, then press the key combination you want. Press Escape to cancel. Live mode registers New Capture, screenshot and recording shortcuts; fixture scenes only edit settings.",|this,ui| {
            for (index, field) in SHORTCUT_FIELDS.into_iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }
                ui.scope_builder(
                    egui::UiBuilder::new().scope_id(shortcut_scope_id(field)),
                    |ui| this.shortcut_row(ui, t, field),
                );
            }
        });
    }

    fn shortcut_row(&mut self, ui: &mut egui::Ui, t: &Tokens, field: ShortcutField) {
        let recording = self
            .shortcut_recorder
            .as_ref()
            .is_some_and(|recorder| recorder.field == field);
        let keys = if recording {
            self.shortcut_recorder
                .as_ref()
                .map(|recorder| recorder.keys.clone())
                .unwrap_or_default()
        } else {
            shortcut_display_tokens(&string_at(&self.value, field.path()), shortcut_platform())
        };
        let error = recording
            .then(|| {
                self.shortcut_recorder
                    .as_ref()
                    .and_then(|recorder| recorder.error.clone())
            })
            .flatten();
        self.row(ui, field.label(), "", |this, ui| {
            let response = ui
                .vertical(|ui| {
                    ui.set_width(230.);
                    let label = if keys.is_empty() {
                        "Press shortcut…".to_owned()
                    } else {
                        keys.join("  +  ")
                    };
                    let response = ui.add_sized(
                        [230., t.number("h-md")],
                        egui::Button::new(label).selected(recording),
                    );
                    if let Some(error) = &error {
                        ui.colored_label(t.color("danger-text"), error);
                    }
                    response
                })
                .inner;
            let started = response.clicked() && !recording;
            if started {
                this.shortcut_recorder = Some(ShortcutRecorder::new(field));
                this.shortcut_input.start();
                response.request_focus();
            }
            let active = this
                .shortcut_recorder
                .as_ref()
                .is_some_and(|recorder| recorder.field == field);
            if shortcut_recording_lost_focus(active, started, response.has_focus()) {
                this.cancel_shortcut_recording();
            } else if active || this.suppress_shortcut_commands {
                ui.input_mut(|input| {
                    input.events.retain(|event| {
                        !matches!(
                            event,
                            egui::Event::Key { .. }
                                | egui::Event::Copy
                                | egui::Event::Cut
                                | egui::Event::Paste(_)
                                | egui::Event::Text(_)
                        )
                    });
                });
            }
        });
    }

    fn receive_shortcut_input(&mut self) {
        let events = self.shortcut_input.drain();
        self.suppress_shortcut_commands = !events.is_empty();
        for event in events {
            match event {
                shortcut_input::Event::Blur => self.cancel_shortcut_recording(),
                shortcut_input::Event::Key {
                    code,
                    pressed,
                    repeat,
                    modifiers,
                } if !repeat => {
                    if !self.apply_shortcut_key(&code, pressed, modifiers) {
                        break;
                    }
                }
                shortcut_input::Event::Key { .. } => {}
            }
        }
    }

    fn apply_shortcut_key(
        &mut self,
        code: &str,
        pressed: bool,
        modifiers: shortcut_input::Modifiers,
    ) -> bool {
        let Some(recorder) = &mut self.shortcut_recorder else {
            return false;
        };
        let modifier = modifier_kind(code);
        if modifier == Some(ShortcutModifier::Super) {
            recorder.super_held = pressed;
        }
        if !pressed && modifier.is_none() {
            return true;
        }
        let event = ShortcutKeyEvent {
            code: code.to_owned(),
            ctrl_key: if modifier == Some(ShortcutModifier::Control) {
                pressed
            } else {
                modifiers.ctrl
            },
            shift_key: if modifier == Some(ShortcutModifier::Shift) {
                pressed
            } else {
                modifiers.shift
            },
            alt_key: if modifier == Some(ShortcutModifier::Alt) {
                pressed
            } else {
                modifiers.alt
            },
            meta_key: modifiers.meta || recorder.super_held,
        };
        match record_shortcut(&event, shortcut_platform()) {
            ShortcutRecording::Cancel => {
                self.cancel_shortcut_recording();
                false
            }
            ShortcutRecording::Waiting { keys } => {
                recorder.keys = keys;
                recorder.error = None;
                true
            }
            ShortcutRecording::Invalid { keys, message } => {
                recorder.keys = keys;
                recorder.error = Some(message);
                true
            }
            ShortcutRecording::Complete { shortcut, .. } => {
                let field = recorder.field;
                self.set(field.path(), json!(shortcut));
                self.cancel_shortcut_recording();
                false
            }
        }
    }

    fn cancel_shortcut_recording(&mut self) {
        self.shortcut_recorder = None;
        self.shortcut_input.stop();
    }
    fn recording(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui,t,3,"Recording","Defaults for new screen recordings. You can still change them in the capture menu.",|this,ui| {
            this.combo(ui,&["recording","video_format"],"Recording format","Recordings are captured as H.264 MP4. GIF and WebM are converted when you save or export.",&choices(&[("mp4","MP4"),("gif","GIF"),("webm","WebM")]));
            ui.separator();
            this.combo(ui,&["recording","video_fps"],"Frames per second","",&numbers(&[60,30,15]," FPS"));
            this.combo(ui,&["recording","video_max_resolution"],"Maximum resolution","",&choices(&[("original","Original"),("p1080","1080p"),("p720","720p")]));
            ui.separator();
            this.combo(ui,&["recording","countdown_seconds"],"Countdown","Delay before a recording starts.",&countdowns());
            ui.separator();
            this.row(ui,"Default microphone","Microphone device discovery is not connected yet.",|_,ui| { ui.add_enabled(false,egui::Button::new("Unavailable")); });
            for (key,title,desc) in [("capture_system_audio","Record desktop audio","Records sound playing through the system output."),("mono_audio","Export recording audio in mono",""),("show_cursor","Show cursor in recordings",""),("highlight_clicks","Show clicks in recordings",""),("open_editor_after_recording","Open the editor after recording","The recording is kept in Capture History for 30 days, so closing the editor never loses it.")] {
                ui.separator(); this.toggle(ui,&["recording",key],title,desc,true);
            }
        });
    }
    fn gif(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(
            ui,
            t,
            4,
            "GIF export",
            "Starting point when a recording is exported as an animated GIF.",
            |this, ui| {
                this.combo(
                    ui,
                    &["recording", "gif_fps"],
                    "Frames per second",
                    "",
                    &numbers(&[8, 10, 12, 15, 20, 24, 30], " FPS"),
                );
                this.combo(
                    ui,
                    &["recording", "gif_max_width"],
                    "Maximum width",
                    "",
                    &numbers(&[320, 480, 640, 800, 1200], " px"),
                );
                this.combo(
                    ui,
                    &["recording", "gif_max_colors"],
                    "Palette colors",
                    "",
                    &numbers(&[64, 96, 128, 256], ""),
                );
            },
        );
    }
    fn updates(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(
            ui,
            t,
            5,
            "Updates",
            "Experimental native build — signed Preview updates are not connected yet.",
            |this, ui| {
                this.toggle(
                    ui,
                    &["show_update_changelog"],
                    "Show what’s new with Preview updates",
                    "Turn this off to keep the update notice compact.",
                    true,
                );
                ui.add_enabled(false, egui::Button::new("Check for updates"));
            },
        );
    }
    fn about(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui,t,6,"About","Captures is in active development. Telling us what breaks is the fastest way to fix it.",|this,ui| {
            this.row(ui,"Send feedback","No captures or diagnostics are attached.",|this,ui| {
                if ui.button("Open").clicked() { this.feedback.open(ui.ctx()); }
            });
            ui.separator();
            this.toggle(ui,&["launch_at_login"],"Launch Captures when I sign in","Login-item integration is not connected yet.",false);
        });
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ShortcutModifier {
    Control,
    Shift,
    Alt,
    Super,
}

fn modifier_kind(code: &str) -> Option<ShortcutModifier> {
    match code {
        "ControlLeft" | "ControlRight" => Some(ShortcutModifier::Control),
        "ShiftLeft" | "ShiftRight" => Some(ShortcutModifier::Shift),
        "AltLeft" | "AltRight" => Some(ShortcutModifier::Alt),
        "MetaLeft" | "MetaRight" => Some(ShortcutModifier::Super),
        _ => None,
    }
}

fn shortcut_recording_lost_focus(active: bool, just_started: bool, has_focus: bool) -> bool {
    active && !just_started && !has_focus
}

fn shortcut_platform() -> ShortcutPlatform {
    #[cfg(target_os = "windows")]
    return ShortcutPlatform::Windows;
    #[cfg(target_os = "linux")]
    return ShortcutPlatform::Linux;
    #[cfg(target_os = "macos")]
    return ShortcutPlatform::Macos;
    #[allow(unreachable_code)]
    ShortcutPlatform::Linux
}

fn choices(values: &[(&str, &str)]) -> Vec<(Value, String)> {
    values
        .iter()
        .map(|(v, s)| (json!(v), (*s).into()))
        .collect()
}
fn numbers(values: &[u16], suffix: &str) -> Vec<(Value, String)> {
    values
        .iter()
        .map(|v| (json!(v), format!("{v}{suffix}")))
        .collect()
}
fn countdowns() -> Vec<(Value, String)> {
    (0..=10)
        .map(|v| {
            (
                json!(v),
                match v {
                    0 => "Off".into(),
                    1 => "1 second".into(),
                    _ => format!("{v} seconds"),
                },
            )
        })
        .collect()
}
fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.get(*k))
}
fn string_at(v: &Value, path: &[&str]) -> String {
    at(v, path)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into()
}
fn set(v: &mut Value, path: &[&str], value: Value) {
    let mut target = v;
    for key in &path[..path.len() - 1] {
        target = &mut target[*key];
    }
    target[path[path.len() - 1]] = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_shortcut_button(ctx: &egui::Context, show_error: bool, focus: bool) -> egui::Id {
        let mut button_id = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if show_error {
                ui.label("Couldn’t save changes");
            }
            egui::ScrollArea::vertical()
                .id_salt("preferences-scroll")
                .show(ui, |ui| {
                    egui::Frame::new().show(ui, |ui| {
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .scope_id(shortcut_scope_id(ShortcutField::Window)),
                            |ui| {
                                ui.horizontal(|ui| {
                                    let response = ui.button("Window shortcut");
                                    if focus {
                                        response.request_focus();
                                    }
                                    button_id = Some(response.id);
                                });
                            },
                        );
                    });
                });
        });
        output.textures_delta.clear();
        button_id.unwrap()
    }

    #[test]
    fn recorder_uses_physical_codes_and_keeps_invalid_chords_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        captures_settings::save(&path, &AppSettings::default()).unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), path, None, None);
        prefs.io.flush();
        prefs.receive(&ctx);
        let original = string_at(&prefs.value, ShortcutField::Region.path());

        prefs.shortcut_recorder = Some(ShortcutRecorder::new(ShortcutField::Region));
        prefs.shortcut_input.start();
        assert!(prefs.apply_shortcut_key("KeyP", true, shortcut_input::Modifiers::default()));
        let recorder = prefs.shortcut_recorder.as_ref().unwrap();
        assert!(recorder.error.is_some());
        assert_eq!(
            string_at(&prefs.value, ShortcutField::Region.path()),
            original
        );

        assert!(prefs.apply_shortcut_key(
            "Unidentified",
            true,
            shortcut_input::Modifiers {
                ctrl: true,
                ..Default::default()
            }
        ));
        let recorder = prefs.shortcut_recorder.as_ref().unwrap();
        assert_eq!(recorder.keys, ["Ctrl", "Unidentified"]);
        assert_eq!(
            recorder.error.as_deref(),
            Some("That key cannot be used as a global shortcut.")
        );

        assert!(!prefs.apply_shortcut_key(
            "Escape",
            true,
            shortcut_input::Modifiers {
                ctrl: true,
                ..Default::default()
            }
        ));
        assert!(prefs.shortcut_recorder.is_none());

        prefs.shortcut_recorder = Some(ShortcutRecorder::new(ShortcutField::Region));
        prefs.shortcut_input.start();
        assert!(!prefs.apply_shortcut_key(
            "KeyD",
            true,
            shortcut_input::Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            }
        ));
        assert_eq!(
            string_at(&prefs.value, ShortcutField::Region.path()),
            "Control+Shift+KeyD"
        );
        assert!(prefs.shortcut_recorder.is_none());
    }

    #[test]
    fn recorder_focus_is_acquired_before_blur_can_cancel() {
        assert!(!shortcut_recording_lost_focus(true, true, false));
        assert!(!shortcut_recording_lost_focus(true, false, true));
        assert!(shortcut_recording_lost_focus(true, false, false));
        assert!(!shortcut_recording_lost_focus(false, false, false));
    }

    #[test]
    fn recorder_button_id_and_focus_survive_error_ui_above_scroll() {
        let ctx = egui::Context::default();
        let before = render_shortcut_button(&ctx, false, true);
        assert!(ctx.memory(|memory| memory.has_focus(before)));

        let after = render_shortcut_button(&ctx, true, false);
        assert_eq!(after, before);
        assert!(ctx.memory(|memory| memory.has_focus(after)));
    }

    #[test]
    fn leaving_preferences_cancels_shortcut_recording_and_raw_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        captures_settings::save(&path, &AppSettings::default()).unwrap();
        let mut prefs = Preferences::new(egui::Context::default(), path, None, None);
        prefs.shortcut_recorder = Some(ShortcutRecorder::new(ShortcutField::Window));
        prefs.shortcut_input.start();

        prefs.set_presented(false);

        assert!(!prefs.is_recording_shortcut());
        assert!(!prefs.shortcut_input.is_active());
    }

    #[test]
    fn failed_shortcut_save_keeps_the_edit_and_reports_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::create_dir(&path).unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), path, None, None);
        prefs.value = serde_json::to_value(AppSettings::default()).unwrap();
        prefs.load_error = None;

        prefs.set(ShortcutField::Window.path(), json!("Alt+KeyW"));
        prefs.io.flush();
        prefs.receive(&ctx);

        assert_eq!(
            string_at(&prefs.value, ShortcutField::Window.path()),
            "Alt+KeyW"
        );
        assert!(prefs.save_error.is_some());
    }

    #[test]
    fn capture_snapshot_uses_current_edits_not_last_disk_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        captures_settings::save(&path, &AppSettings::default()).unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), path.clone(), None, None);
        prefs.io.flush();
        prefs.receive(&ctx);
        set(&mut prefs.value, &["auto_copy_to_clipboard"], json!(false));
        set(&mut prefs.value, &["screenshot_format"], json!("webp"));
        let snapshot = prefs.snapshot().unwrap();
        assert!(!snapshot.auto_copy_to_clipboard);
        assert_eq!(
            snapshot.screenshot_format,
            captures_settings::ScreenshotFormat::Webp
        );
        assert!(
            captures_settings::load(&path)
                .unwrap()
                .auto_copy_to_clipboard
        );
        prefs.load_error = Some("settings unreadable".into());
        assert!(prefs.snapshot().is_err());
    }

    #[test]
    fn flush_persists_last_edit_and_reopens_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (tx, rx) = mpsc::channel();
        let mut io = SettingsIo::start(path.clone(), tx, || {});
        assert!(matches!(rx.recv().unwrap(), Message::Loaded(Ok(_))));
        let mut settings = AppSettings::default();
        for revision in 1..=20 {
            settings.screenshot_countdown_seconds = (revision % 11) as u8;
            io.tx
                .send(Command::Save(revision, Box::new(settings.clone())))
                .unwrap();
        }
        io.flush();
        let messages: Vec<_> = rx.try_iter().collect();
        assert!(matches!(messages.last(), Some(Message::Saved(20, Ok(_)))));
        assert_eq!(
            captures_settings::load(&path)
                .unwrap()
                .screenshot_countdown_seconds,
            9
        );
    }
    #[test]
    fn initial_settings_loading_is_distinct_from_ready_and_failed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), dir.path().join("settings.json"), None, None);
        assert!(prefs.is_loading());
        prefs.io.flush();
        prefs.receive(&ctx);
        assert!(!prefs.is_loading());
        assert!(prefs.snapshot().is_ok());
        prefs.value = Value::Null;
        prefs.load_error = Some("unreadable settings".into());
        assert!(
            !prefs.is_loading(),
            "a load failure must not leave file opens waiting forever"
        );
        assert_eq!(prefs.snapshot().unwrap_err(), "unreadable settings");
    }

    #[test]
    fn stale_save_reply_does_not_replace_newer_edit() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), dir.path().join("settings.json"), None, None);
        prefs.io.flush();
        while prefs.rx.try_recv().is_ok() {}
        prefs.value = serde_json::to_value(AppSettings::default()).unwrap();
        set(&mut prefs.value, &["appearance"], json!("dark"));
        prefs.revision = 2;
        prefs.saving = true;
        prefs
            .out
            .send(Message::Saved(1, Ok(AppSettings::default())))
            .unwrap();
        prefs.receive(&ctx);
        assert_eq!(prefs.value["appearance"], "dark");
        assert!(prefs.saving);
    }
    #[test]
    fn failed_save_can_retry_without_discarding_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // Reading a directory as JSON fails on every host. A file used as a
        // parent instead reports NotFound on Windows (a valid defaults load).
        std::fs::create_dir(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut io = SettingsIo::start(path.clone(), tx, || {});
        assert!(matches!(rx.recv().unwrap(), Message::Loaded(Err(_))));
        let settings = AppSettings {
            screenshot_countdown_seconds: 7,
            ..Default::default()
        };
        io.tx
            .send(Command::Save(1, Box::new(settings.clone())))
            .unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Message::Saved(1, Err(_))
        ));
        std::fs::remove_dir(&path).unwrap();
        io.tx.send(Command::Save(2, Box::new(settings))).unwrap();
        io.flush();
        assert_eq!(
            captures_settings::load(&path)
                .unwrap()
                .screenshot_countdown_seconds,
            7
        );
    }
}
