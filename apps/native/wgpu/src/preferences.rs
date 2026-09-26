use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use captures_app::capture_menu::{self, PreferenceTarget};
use captures_app::preferences;
use captures_app::shortcuts::{
    ShortcutKeyEvent, ShortcutPlatform, ShortcutRecording, record_shortcut, shortcut_display_tokens,
};
use captures_settings::{AppSettings, theme::normalize_hex_color};
use eframe::egui::{self, RichText, Stroke};
use serde_json::{Value, json};

use crate::{
    preferences_widgets as widgets, shortcut_input,
    tokens::{self, Tokens},
};

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
    fn index(self) -> usize {
        SHORTCUT_FIELDS
            .iter()
            .position(|field| *field == self)
            .expect("listed shortcut field")
    }

    fn label(self) -> &'static str {
        preferences::SHORTCUT_ROWS[self.index()].0
    }

    fn path(self) -> &'static [&'static str] {
        preferences::SHORTCUT_ROWS[self.index()].1
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
    Onboarding(captures_app::onboarding::Action),
    PermissionRecovery(captures_app::onboarding::Action),
    LoginItem(PathBuf, Option<bool>),
    MotionPreference,
    Flush,
}
enum Message {
    Loaded(Result<AppSettings, String>),
    Saved(u64, Result<AppSettings, String>),
    Folder(Option<PathBuf>),
    LoginItem(Result<bool, String>),
    Onboarding(Result<captures_app::onboarding::State, String>),
    PermissionRecovery(Result<captures_app::onboarding::State, String>),
    MotionPreference(Option<bool>),
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
            let mut onboarding = captures_app::onboarding::Session::new();
            let send_load = || {
                let _ = out.send(Message::Loaded(
                    captures_settings::load(&path).map_err(|e| e.to_string()),
                ));
                wake();
            };
            let login_item = |root: PathBuf, enabled| {
                let result = captures_app::login_item::configure(&root, &path, enabled);
                let _ = out.send(Message::LoginItem(result));
                wake();
            };
            let motion_preference = || {
                let _ = out.send(Message::MotionPreference(
                    captures_session::prefers_reduced_motion(),
                ));
                wake();
            };
            send_load();
            let _ = out.send(Message::Onboarding(
                onboarding.execute(&path, captures_app::onboarding::Action::Check),
            ));
            wake();
            while let Ok(command) = rx.recv() {
                match command {
                    Command::Load => send_load(),
                    Command::Onboarding(action) => {
                        let _ = out.send(Message::Onboarding(onboarding.execute(&path, action)));
                        wake();
                    }
                    Command::PermissionRecovery(action) => {
                        let _ = out.send(Message::PermissionRecovery(
                            onboarding.execute(&path, action),
                        ));
                        wake();
                    }
                    Command::LoginItem(root, enabled) => login_item(root, enabled),
                    Command::MotionPreference => motion_preference(),
                    Command::Flush => break,
                    Command::Save(mut revision, mut settings) => {
                        let mut finish = false;
                        let mut after_save = None;
                        let mut recovery_after_save = None;
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
                                Ok(Command::Onboarding(action)) => {
                                    after_save = Some(action);
                                    break;
                                }
                                Ok(Command::PermissionRecovery(action)) => {
                                    recovery_after_save = Some(action);
                                    break;
                                }
                                Ok(Command::LoginItem(root, enabled)) => login_item(root, enabled),
                                Ok(Command::MotionPreference) => motion_preference(),
                                Err(mpsc::RecvTimeoutError::Timeout) => break,
                            }
                        }
                        let result =
                            captures_settings::save(&path, &settings).map_err(|e| e.to_string());
                        let _ = out.send(Message::Saved(revision, result));
                        wake();
                        if let Some(action) = after_save {
                            let _ =
                                out.send(Message::Onboarding(onboarding.execute(&path, action)));
                            wake();
                        }
                        if let Some(action) = recovery_after_save {
                            let _ = out.send(Message::PermissionRecovery(
                                onboarding.execute(&path, action),
                            ));
                            wake();
                        }
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
    /// When the save status pill appeared, for its pop-in.
    status_shown_at: Option<Instant>,
    revision: u64,
    io: SettingsIo,
    rx: Receiver<Message>,
    out: Sender<Message>,
    overrides: Option<(Option<String>, Option<String>)>,
    find_open: bool,
    query: String,
    matches: Vec<(egui::Rect, egui::layers::ShapeIdx)>,
    find_rows: Vec<FindRow>,
    card_tops: Vec<f32>,
    live: bool,
    keyboard_settings_error: Option<String>,
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
    login_root: Option<PathBuf>,
    login_enabled: Option<bool>,
    login_pending: bool,
    login_error: Option<String>,
    onboarding: Option<captures_app::onboarding::State>,
    onboarding_error: Option<String>,
    /// The in-flight setup request, if any.
    onboarding_busy: Option<captures_app::onboarding::Action>,
    permission_recovery_open: bool,
    permission_recovery: Option<captures_app::onboarding::State>,
    permission_recovery_error: Option<String>,
    permission_recovery_busy: Option<captures_app::onboarding::Action>,
    system_reduced_motion: bool,
    motion_pending: bool,
    /// Microphones for the Default microphone select, enumerated off the UI thread.
    microphones: Option<Vec<(String, String)>>,
    microphones_rx: Option<Receiver<Vec<(String, String)>>>,
    /// Capture-menu deep link: scroll to this row once and highlight it.
    highlight: Option<PreferenceHighlight>,
}

/// Labelled select choices: the persisted value and its display label.
type Options = Vec<(Value, String)>;

/// One find target: its text, rect and a background slot for the match wash.
struct FindRow {
    text: String,
    rect: egui::Rect,
    background: egui::layers::ShapeIdx,
}

struct PreferenceHighlight {
    target: PreferenceTarget,
    until: Instant,
    scrolled: bool,
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
        let io = SettingsIo::start(path, out.clone(), move || {
            ctx.request_repaint_of(egui::ViewportId::ROOT)
        });
        Self {
            value: Value::Null,
            load_error: None,
            save_error: None,
            saving: false,
            saved_until: None,
            status_shown_at: None,
            revision: 0,
            io,
            rx,
            out,
            overrides: Some((appearance, theme)),
            find_open: false,
            query: String::new(),
            matches: vec![],
            find_rows: vec![],
            card_tops: vec![],
            live: false,
            keyboard_settings_error: None,
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
            login_root: None,
            login_enabled: None,
            login_pending: false,
            login_error: None,
            onboarding: None,
            onboarding_error: None,
            onboarding_busy: Some(captures_app::onboarding::Action::Check),
            permission_recovery_open: false,
            permission_recovery: None,
            permission_recovery_error: None,
            permission_recovery_busy: None,
            system_reduced_motion: false,
            motion_pending: false,
            highlight: None,
            microphones: None,
            microphones_rx: None,
        }
    }

    pub fn refresh_motion_preference(&mut self) {
        if !self.motion_pending {
            self.motion_pending = self.io.tx.send(Command::MotionPreference).is_ok();
        }
    }

    pub fn reduced_motion(&self, force: bool) -> bool {
        force || self.system_reduced_motion
    }

    pub fn connect_login_item(&mut self, history_root: PathBuf) {
        self.login_root = Some(history_root);
        self.request_login_item(None);
    }

    fn request_login_item(&mut self, enabled: Option<bool>) {
        let Some(root) = &self.login_root else { return };
        if self.login_pending {
            return;
        }
        self.login_pending = true;
        self.login_error = None;
        if self
            .io
            .tx
            .send(Command::LoginItem(root.clone(), enabled))
            .is_err()
        {
            self.login_pending = false;
            self.login_error =
                Some("Login item service is unavailable. Reopen Preferences to retry.".into());
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

    pub fn onboarding_complete(&self) -> bool {
        self.load_error.is_none()
            && self.onboarding_error.is_none()
            && self
                .onboarding
                .as_ref()
                .is_some_and(|state| state.onboarding_completed)
    }

    pub fn onboarding_pending(&self) -> bool {
        self.onboarding_busy.is_some()
            || (self.onboarding.is_none() && self.onboarding_error.is_none())
    }

    pub fn onboarding_error(&self) -> Option<&str> {
        self.onboarding_error.as_deref()
    }

    pub fn retry_onboarding(&mut self) {
        let _ = self.io.tx.send(Command::Load);
        self.send_onboarding(captures_app::onboarding::Action::Check);
    }

    pub fn complete_onboarding(&mut self) {
        self.send_onboarding(captures_app::onboarding::Action::Complete);
    }

    pub fn request_onboarding_screen(&mut self) {
        self.send_onboarding(captures_app::onboarding::Action::RequestScreen);
    }

    pub fn request_onboarding_microphone(&mut self) {
        self.send_onboarding(captures_app::onboarding::Action::RequestMicrophone);
    }

    pub fn onboarding_state(&self) -> Option<&captures_app::onboarding::State> {
        self.onboarding.as_ref()
    }

    pub fn onboarding_busy_action(&self) -> Option<captures_app::onboarding::Action> {
        self.onboarding_busy
    }

    fn send_onboarding(&mut self, action: captures_app::onboarding::Action) {
        if self.onboarding_busy.is_some() {
            return;
        }
        self.onboarding_busy = Some(action);
        self.onboarding_error = None;
        if self.io.tx.send(Command::Onboarding(action)).is_err() {
            self.onboarding_busy = None;
            self.onboarding_error =
                Some("Setup service is unavailable. Restart Captures to retry.".into());
        }
    }

    pub fn open_permission_recovery(&mut self) {
        self.permission_recovery_open = true;
        self.send_permission_recovery(captures_app::onboarding::Action::Check);
    }

    pub fn show_permission_recovery_error_fixture(&mut self) {
        self.permission_recovery_open = true;
        self.permission_recovery_busy = None;
        self.permission_recovery = None;
        self.permission_recovery_error = Some("Permission check fixture failed.".into());
    }

    pub fn permission_recovery_open(&self) -> bool {
        self.permission_recovery_open
    }

    pub fn permission_recovery_state(&self) -> Option<&captures_app::onboarding::State> {
        self.permission_recovery.as_ref()
    }

    pub fn permission_recovery_error(&self) -> Option<&str> {
        self.permission_recovery_error.as_deref()
    }

    #[cfg(test)]
    pub fn permission_recovery_busy(&self) -> bool {
        self.permission_recovery_busy.is_some()
    }

    pub fn permission_recovery_busy_action(&self) -> Option<captures_app::onboarding::Action> {
        self.permission_recovery_busy
    }

    pub fn request_screen_permission(&mut self) {
        self.send_permission_recovery(captures_app::onboarding::Action::RequestScreen);
    }

    pub fn request_microphone_permission(&mut self) {
        self.send_permission_recovery(captures_app::onboarding::Action::RequestMicrophone);
    }

    pub fn close_permission_recovery(&mut self) {
        if self.permission_recovery_busy.is_none() {
            self.permission_recovery_open = false;
            self.permission_recovery = None;
            self.permission_recovery_error = None;
        }
    }

    fn send_permission_recovery(&mut self, action: captures_app::onboarding::Action) {
        if self.permission_recovery_busy.is_some() {
            return;
        }
        debug_assert_ne!(action, captures_app::onboarding::Action::Complete);
        self.permission_recovery_busy = Some(action);
        self.permission_recovery_error = None;
        if self
            .io
            .tx
            .send(Command::PermissionRecovery(action))
            .is_err()
        {
            self.permission_recovery_busy = None;
            self.permission_recovery_error =
                Some("Permission service is unavailable. Reopen Captures to retry.".into());
        }
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

    /// The update notice's Hide / What's new toggle persists like shipping.
    pub fn set_show_update_changelog(&mut self, show: bool) {
        if !self.value.is_null() {
            self.set(&["show_update_changelog"], json!(show));
        }
    }

    /// Capture-menu note links: reveal `target`'s row and highlight it briefly,
    /// like the shipping `preferences-target` event.
    pub fn open_target(&mut self, target: PreferenceTarget) {
        self.highlight = Some(PreferenceHighlight {
            target,
            until: Instant::now() + capture_menu::PREFERENCE_HIGHLIGHT,
            scrolled: false,
        });
    }

    #[cfg(test)]
    fn highlighted_target(&self) -> Option<PreferenceTarget> {
        self.highlight.as_ref().map(|highlight| highlight.target)
    }

    /// Tray "Send Feedback…": show the feedback form in Preferences.
    pub fn open_feedback(&mut self, ctx: &egui::Context) {
        self.feedback.open(ctx);
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
                Message::LoginItem(result) => {
                    self.login_pending = false;
                    match result {
                        Ok(enabled) => {
                            self.login_enabled = Some(enabled);
                            self.login_error = None;
                        }
                        Err(error) => self.login_error = Some(error),
                    }
                }
                Message::Onboarding(result) => {
                    self.onboarding_busy = None;
                    match result {
                        Ok(state) => {
                            self.onboarding = Some(state);
                            self.onboarding_error = None;
                        }
                        Err(error) => self.onboarding_error = Some(error),
                    }
                }
                Message::PermissionRecovery(result) => {
                    self.permission_recovery_busy = None;
                    match result {
                        Ok(state) => {
                            self.permission_recovery = Some(state);
                            self.permission_recovery_error = None;
                        }
                        Err(error) => self.permission_recovery_error = Some(error),
                    }
                }
                Message::MotionPreference(value) => {
                    self.motion_pending = false;
                    // A temporarily missing portal must not turn motion back on
                    // after the desktop explicitly requested it be reduced.
                    if let Some(value) = value {
                        self.system_reduced_motion = value;
                    }
                    crate::emit(
                        "motion-preference",
                        json!({
                            "available": value.is_some(), "reduced": self.system_reduced_motion
                        }),
                    );
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

    /// Shipping `.preferences-nav`: the brand, then one entry per card with the
    /// card in view highlighted.
    pub fn sidebar(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        widgets::brand(ui, t);
        ui.add_space(t.number("s-6"));
        ui.spacing_mut().item_spacing.y = 1.;
        for (index, section) in preferences::SECTIONS.iter().enumerate() {
            if widgets::nav_item(ui, t, section.title, self.active_section == index).clicked() {
                self.feedback.open = false;
                self.section_jump = Some(index);
                self.active_section = index;
            }
        }
    }

    /// The sidebar panel frame: sunken, padded, with a subtle right border.
    pub fn sidebar_frame(t: &Tokens) -> egui::Frame {
        egui::Frame::new()
            .fill(t.color("surface-sunken"))
            .inner_margin(egui::Margin::symmetric(
                t.number("s-5") as i8,
                t.number("s-6") as i8,
            ))
    }

    /// Paints the shipping `border-right: 1px solid var(--border-subtle)`.
    pub fn sidebar_border(ui: &egui::Ui, t: &Tokens) {
        let rect = ui.max_rect();
        ui.painter().vline(
            rect.right() + t.number("s-5") - 0.5,
            (rect.top() - t.number("s-6"))..=(rect.bottom() + t.number("s-6")),
            Stroke::new(1., t.color("border-subtle")),
        );
    }

    /// Returns true when the user requests the history window.
    pub fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens, live: bool) -> bool {
        if self.feedback.open {
            // The feedback form keeps the panel margin it was designed with
            // (egui's 8 pt live central panel, the fixture's `--s-8`).
            let margin = if live { 8. } else { t.number("s-8") };
            egui::Frame::new()
                .inner_margin(margin as i8)
                .show(ui, |ui| self.feedback.ui(ui, t, live));
            return false;
        }
        self.live = live;
        self.receive_shortcut_input();
        self.keyboard(ui);
        ui.spacing_mut().item_spacing.y = 0.;
        let history = self.header(ui, t);
        if let Some(error) = self.load_error.clone() {
            egui::Frame::new()
                .inner_margin(t.number("s-8") as i8)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = t.number("s-4");
                    ui.colored_label(t.color("danger-text"), preferences::load_error(&error));
                    ui.label("The file was left unchanged. Correct it, then retry.");
                    if widgets::button(ui, t, "Retry loading", false).clicked() {
                        let _ = self.io.tx.send(Command::Load);
                    }
                });
            return history;
        }
        if self.value.is_null() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(preferences::LOADING).color(t.color("text-subtle")));
            });
            return history;
        }
        let margin = egui::Margin {
            left: t.number("s-8") as i8,
            right: t.number("s-8") as i8,
            top: t.number("s-8") as i8,
            bottom: t.number("s-12") as i8,
        };
        let output = egui::ScrollArea::vertical()
            .id_salt("preferences-scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(margin).show(ui, |ui| {
                    ui.set_max_width(720. - 2. * t.number("s-8"));
                    ui.spacing_mut().item_spacing.y = 0.;
                    self.find_rows.clear();
                    self.card_tops.clear();
                    self.appearance(ui, t);
                    self.capture(ui, t);
                    self.shortcuts(ui, t);
                    self.recording(ui, t);
                    self.gif(ui, t);
                    self.updates(ui, t);
                    self.about(ui, t);
                    self.paint_find(ui, t);
                });
            });
        let viewport = output.inner_rect;
        let at_end = output.state.offset.y + viewport.height() >= output.content_size.y - 1.;
        self.active_section = preferences::visible_section(
            &self.card_tops,
            viewport.top() + 80.,
            at_end && output.state.offset.y > 0.,
        );
        history
    }

    /// Shipping `.preferences-header`: title, autosave note, History action,
    /// save status and (when open) the find bar, over a subtle rule.
    fn header(&mut self, ui: &mut egui::Ui, t: &Tokens) -> bool {
        let mut history = false;
        let header = egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(
                t.number("s-8") as i8,
                t.number("s-6") as i8,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.;
                        ui.label(RichText::new(preferences::TITLE).size(t.number("text-xl")));
                        ui.label(
                            RichText::new(preferences::SUBTITLE)
                                .size(t.number("text-sm"))
                                .color(t.color("text-subtle")),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = t.number("s-4");
                        let status = if let Some(error) = &self.save_error {
                            Some(("error", preferences::save_error(error)))
                        } else if self.saving {
                            Some(("saving", preferences::SAVING.to_owned()))
                        } else if self.saved_until.is_some_and(|until| Instant::now() < until) {
                            Some(("saved", preferences::SAVED.to_owned()))
                        } else {
                            None
                        };
                        // Shipping `ui-pop-in` (--dur-2) when the pill appears.
                        let now = Instant::now();
                        self.status_shown_at =
                            status.as_ref().map(|_| self.status_shown_at.unwrap_or(now));
                        if let Some((kind, message)) = status {
                            if kind == "error" && widgets::button(ui, t, "Retry", false).clicked() {
                                self.changed();
                            }
                            let pop = t.motion(captures_app::motion::Motion::PopoverIn);
                            let elapsed = self
                                .status_shown_at
                                .map_or(f64::INFINITY, |at| crate::motion::elapsed_ms(at, now));
                            let reduced = crate::motion::reduced(ui.ctx());
                            if pop.running(elapsed, reduced) {
                                ui.ctx().request_repaint();
                            }
                            widgets::status_pill(
                                ui,
                                t,
                                kind,
                                &message,
                                pop.pose_at(elapsed, reduced),
                            );
                        }
                        history =
                            widgets::button(ui, t, preferences::HISTORY_ACTION, true).clicked();
                    });
                });
                if self.find_open && !self.value.is_null() {
                    ui.add_space(t.number("s-6"));
                    self.find_bar(ui, t);
                }
            })
            .response;
        let rect = header.rect;
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1., t.color("border-subtle")),
        );
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
            self.close_find();
        }
        let in_find = ui.memory(|m| m.has_focus(egui::Id::unique("settings-find")));
        if self.find_open
            && ui.input(|i| {
                i.key_pressed(egui::Key::F3)
                    || (i.key_pressed(egui::Key::G) && i.modifiers.command)
                    || (in_find && i.key_pressed(egui::Key::Enter))
            })
        {
            self.step_match(if ui.input(|i| i.modifiers.shift) {
                -1
            } else {
                1
            });
        }
    }

    fn close_find(&mut self) {
        self.find_open = false;
        self.query.clear();
        self.matches.clear();
    }

    /// Shipping `.preferences-find`: field, count, previous/next and close.
    fn find_bar(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = t.number("s-3");
            let side = t.number("h-md");
            let reserved = 5.5 * t.number("text-md") + 3. * side + 4. * t.number("s-3");
            let response = field_scope(ui, t, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::unique("settings-find"))
                        .hint_text(preferences::FIND_PLACEHOLDER)
                        .margin(egui::vec2(t.number("s-4"), t.number("s-2")))
                        .desired_width((ui.available_width() - reserved).max(120.))
                        .min_size(egui::vec2(0., side))
                        .align(egui::Align2::LEFT_CENTER),
                )
            });
            if response.changed() {
                self.match_index = 0;
                self.find_jump = true;
                ui.ctx().request_repaint();
            }
            let count =
                preferences::find_count_label(&self.query, self.matches.len(), self.match_index);
            ui.allocate_ui_with_layout(
                egui::vec2(5.5 * t.number("text-md"), side),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        RichText::new(count)
                            .size(t.number("text-sm"))
                            .color(t.color("text-subtle")),
                    );
                },
            );
            let any = !self.matches.is_empty();
            if ui
                .add_enabled_ui(any, |ui| {
                    widgets::icon_button(ui, t, "chevron-up", preferences::FIND_PREVIOUS)
                })
                .inner
                .clicked()
            {
                self.step_match(-1);
            }
            if ui
                .add_enabled_ui(any, |ui| {
                    widgets::icon_button(ui, t, "chevron-down", preferences::FIND_NEXT)
                })
                .inner
                .clicked()
            {
                self.step_match(1);
            }
            if widgets::icon_button(ui, t, "close", preferences::FIND_CLOSE).clicked() {
                self.close_find();
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

    /// Registers one find target (shipping `PREFERENCE_FIND_SELECTOR`) whose
    /// background shape can later show the match wash.
    fn remember(&mut self, text: String, rect: egui::Rect, background: egui::layers::ShapeIdx) {
        self.find_rows.push(FindRow {
            text,
            rect,
            background,
        });
    }

    /// Shipping `.preference-find-match` wash and `.preference-find-current` ring.
    fn paint_find(&mut self, ui: &egui::Ui, t: &Tokens) {
        self.matches = if self.find_open {
            self.find_rows
                .iter()
                .filter(|row| preferences::find_matches(&row.text, &self.query))
                .map(|row| (row.rect, row.background))
                .collect()
        } else {
            Vec::new()
        };
        self.match_index = self.match_index.min(self.matches.len().saturating_sub(1));
        let inset = t.number("s-3");
        for (index, (rect, background)) in self.matches.iter().enumerate() {
            let stroke = if index == self.match_index {
                Stroke::new(2., t.color("theme-accent").gamma_multiply(0.62))
            } else {
                Stroke::NONE
            };
            ui.painter().set(
                *background,
                egui::epaint::RectShape::new(
                    rect.expand(inset),
                    t.number("r-lg"),
                    t.color("surface-selected"),
                    stroke,
                    egui::StrokeKind::Outside,
                ),
            );
        }
        if self.find_jump {
            if let Some((rect, _)) = self.matches.get(self.match_index) {
                ui.scroll_to_rect(*rect, Some(egui::Align::Center));
            }
            self.find_jump = false;
        }
    }

    fn card(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        index: usize,
        add: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let description = preferences::SECTIONS[index].description.to_owned();
        self.card_described(ui, t, index, &description, add);
    }

    fn card_described(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        index: usize,
        description_text: &str,
        add: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let section = preferences::SECTIONS[index];
        let response = egui::Frame::new()
            .fill(t.color("surface-raised"))
            .stroke(Stroke::new(1., t.color("border-subtle")))
            .corner_radius(t.number("r-xl") as u8)
            .inner_margin(t.number("s-6") as i8)
            .shadow(widgets::shadow_sm(ui.visuals().dark_mode))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.;
                let background = ui.painter().add(egui::Shape::Noop);
                let heading = ui
                    .vertical(|ui| {
                        ui.label(RichText::new(section.title).size(t.number("text-lg")));
                        ui.add_space(t.number("s-2"));
                        description(ui, t, description_text, None);
                    })
                    .response;
                self.remember(
                    format!("{} {description_text}", section.title),
                    heading.rect,
                    background,
                );
                ui.add_space(t.number("s-5") + t.number("s-1"));
                add(self, ui);
            })
            .response;
        self.card_tops.push(response.rect.top());
        if self.section_jump == Some(index) {
            // Shipping `scroll-margin-top: var(--s-6)`.
            let mut target = response.rect;
            target.min.y -= t.number("s-6");
            ui.scroll_to_rect(target, Some(egui::Align::Min));
            self.section_jump = None;
        }
        ui.add_space(t.number("s-6"));
    }

    /// Shipping `.settings-card > * + *` rule between rows.
    fn divider(ui: &mut egui::Ui, t: &Tokens) {
        ui.add_space(t.number("s-5"));
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.), egui::Sense::hover());
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            Stroke::new(1., t.color("border-subtle")),
        );
        ui.add_space(t.number("s-5"));
    }

    /// Shipping `.setting-row-inline`: copy on the left and `control` in a
    /// right-aligned column of `control_size`, both centered on the row.
    /// Returns the row rect and its background shape (used for the deep-link
    /// and find highlights).
    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        title: &str,
        desc: &str,
        emphasis: Option<preferences::Emphasized>,
        control_size: egui::Vec2,
        control: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> (egui::Rect, egui::layers::ShapeIdx) {
        let background = ui.painter().add(egui::Shape::Noop);
        let full = ui.available_width();
        let copy_width = (full - control_size.x - t.number("s-6")).max(150.);
        let title_galley = ui.painter().layout(
            title.to_owned(),
            egui::FontId::proportional(t.number("text-md")),
            t.color("text"),
            copy_width,
        );
        let has_description = !desc.is_empty() || emphasis.is_some();
        let description_galley = has_description.then(|| {
            ui.painter()
                .layout_job(description_job(t, desc, emphasis, copy_width))
        });
        let copy_height = title_galley.size().y
            + description_galley
                .as_ref()
                .map_or(0., |galley| 3. + galley.size().y);
        let height = copy_height.max(control_size.y);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(full, height), egui::Sense::hover());
        let top = rect.top() + (height - copy_height) / 2.;
        let title_height = title_galley.size().y;
        ui.painter()
            .galley(egui::pos2(rect.left(), top), title_galley, t.color("text"));
        if let Some(galley) = description_galley {
            ui.painter().galley(
                egui::pos2(rect.left(), top + title_height + 3.),
                galley,
                t.color("text-subtle"),
            );
        }
        let control_rect = egui::Rect::from_min_max(
            egui::pos2(rect.right() - control_size.x, rect.top()),
            rect.max,
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| control(self, ui),
        );
        let text = emphasis.map_or_else(|| desc.to_owned(), |e| e.text());
        self.remember(format!("{title} {text}"), rect, background);
        (rect, background)
    }

    /// Shipping `.check-row.switch-row`: the whole row toggles the switch.
    fn toggle(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        path: &[&str],
        title: &str,
        desc: &str,
        enabled: bool,
    ) {
        self.toggle_with(ui, t, path, title, desc, None, enabled);
    }

    #[allow(clippy::too_many_arguments)]
    fn toggle_with(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        path: &[&str],
        title: &str,
        desc: &str,
        emphasis: Option<preferences::Emphasized>,
        enabled: bool,
    ) {
        let value = at(&self.value, path)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        ui.add_enabled_ui(enabled, |ui| {
            let mut switch_rect = egui::Rect::NOTHING;
            let (rect, background) = self.row(
                ui,
                t,
                title,
                desc,
                emphasis,
                egui::vec2(32., 19.),
                |_, ui| {
                    switch_rect = ui
                        .allocate_exact_size(egui::vec2(32., 19.), egui::Sense::hover())
                        .0;
                },
            );
            let response = ui
                .interact(
                    rect,
                    ui.scope_id().with(("preference-toggle", path)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                self.set(path, json!(!value));
            }
            let value = at(&self.value, path)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            widgets::paint_switch(ui, t, switch_rect, &response, value, title);
            self.highlight_row(ui, t, path, rect, background);
        });
    }

    fn highlight_row(
        &mut self,
        ui: &egui::Ui,
        t: &Tokens,
        path: &[&str],
        row: egui::Rect,
        background: egui::layers::ShapeIdx,
    ) {
        let Some(highlight) = self
            .highlight
            .as_mut()
            .filter(|highlight| path == [highlight.target.setting_key()])
        else {
            return;
        };
        let remaining = highlight.until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.highlight = None;
            return;
        }
        if !highlight.scrolled {
            ui.scroll_to_rect(row, Some(egui::Align::Center));
            highlight.scrolled = true;
        }
        // Shipping `.preference-target-highlight`: selected wash, accent ring.
        ui.painter().set(
            background,
            egui::epaint::RectShape::new(
                row.expand(t.number("s-4")),
                t.number("r-lg"),
                t.color("surface-selected"),
                Stroke::new(2., t.color("theme-accent").gamma_multiply(0.62)),
                egui::StrokeKind::Outside,
            ),
        );
        ui.ctx().request_repaint_after(remaining);
    }

    /// A shipping `CustomSelect` row, or a stacked grid cell when `width` is set.
    fn select_control(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        path: &[&str],
        width: f32,
        options: &[(Value, String)],
    ) {
        let old = at(&self.value, path).cloned().unwrap_or(Value::Null);
        let mut value = old.clone();
        let selected = options
            .iter()
            .find(|(v, _)| v == &value)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| value.to_string());
        widgets::select(ui, t, path.join("."), width, &selected, |ui| {
            for (v, label) in options {
                ui.selectable_value(&mut value, v.clone(), label);
            }
        });
        if value != old {
            self.set(path, value);
        }
    }

    fn combo(&mut self, ui: &mut egui::Ui, t: &Tokens, path: &[&str], options: &[(Value, String)]) {
        let key = path.join(".");
        let copy = preferences::row(&key);
        self.row(
            ui,
            t,
            copy.title,
            copy.description,
            None,
            egui::vec2(160., t.number("h-md")),
            |this, ui| {
                this.select_control(ui, t, path, 160., options);
            },
        );
    }

    /// Shipping `.setting-grid`: stacked title-over-select cells.
    fn select_grid(&mut self, ui: &mut egui::Ui, t: &Tokens, cells: &[(&[&str], Options)]) {
        let gap = t.number("s-5");
        let width = (ui.available_width() - gap * (cells.len() as f32 - 1.)) / cells.len() as f32;
        let background = ui.painter().add(egui::Shape::Noop);
        let response = ui
            .with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (path, options) in cells {
                    let copy = preferences::row(&path.join("."));
                    let cell_background = ui.painter().add(egui::Shape::Noop);
                    let cell = ui
                        .allocate_ui_with_layout(
                            egui::vec2(width, 0.),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width);
                                ui.spacing_mut().item_spacing.y = 0.;
                                ui.label(RichText::new(copy.title).size(t.number("text-md")));
                                ui.add_space(t.number("s-3"));
                                self.select_control(ui, t, path, width, options);
                            },
                        )
                        .response;
                    self.remember(copy.title.to_owned(), cell.rect, cell_background);
                }
            })
            .response;
        let _ = (background, response);
    }

    fn appearance(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui, t, 0, |this, ui| {
            let copy = preferences::row("appearance");
            let selected = string_at(&this.value, &["appearance"]);
            let mut chosen = None;
            this.row(
                ui,
                t,
                copy.title,
                copy.description,
                None,
                egui::vec2(200., t.number("h-sm") + 8.),
                |_, ui| {
                    chosen = widgets::segmented(
                        ui,
                        t,
                        copy.title,
                        &preferences::APPEARANCE_MODES,
                        &selected,
                    );
                },
            );
            if let Some(mode) = chosen {
                this.set(&["appearance"], json!(mode));
            }
            Self::divider(ui, t);
            let copy = preferences::row("theme");
            let background = ui.painter().add(egui::Shape::Noop);
            let heading = ui
                .vertical(|ui| {
                    ui.label(RichText::new(copy.title).size(t.number("text-md")));
                    ui.add_space(3.);
                    description(ui, t, copy.description, None);
                })
                .response;
            this.remember(
                format!("{} {}", copy.title, copy.description),
                heading.rect,
                background,
            );
            ui.add_space(t.number("s-4"));
            let gap = t.number("s-2");
            let columns = preferences::THEME_COLUMNS;
            let width = (ui.available_width() - gap * (columns as f32 - 1.)) / columns as f32;
            let mode = if ui.visuals().dark_mode {
                "dark"
            } else {
                "light"
            };
            let theme = string_at(&this.value, &["theme"]);
            for (row, chunk) in preferences::THEMES.chunks(columns).enumerate() {
                if row > 0 {
                    ui.add_space(gap);
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for choice in chunk {
                        let palette = (choice.id != "custom").then(|| {
                            let palette = this
                                .variants
                                .get(&format!("{mode}-{}", choice.id))
                                .unwrap_or(t);
                            (palette.color("theme-accent"), palette.color("theme-signal"))
                        });
                        let background = ui.painter().add(egui::Shape::Noop);
                        let response = widgets::theme_chip(
                            ui,
                            t,
                            width,
                            choice.name,
                            &preferences::theme_accessibility_label(choice),
                            palette,
                            theme == choice.id,
                        )
                        .on_hover_text(choice.description);
                        if response.clicked() {
                            this.set(&["theme"], json!(choice.id));
                        }
                        this.remember(
                            format!("{} {}", choice.name, choice.description),
                            response.rect,
                            background,
                        );
                    }
                });
            }
            if string_at(&this.value, &["theme"]) == "custom" {
                Self::divider(ui, t);
                this.custom_theme_editor(ui, t);
            }
        });
    }

    /// Shipping `.custom-theme-editor`: heading, Reset colors and two fields.
    fn custom_theme_editor(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let copy = &preferences::CUSTOM_THEME;
        let background = ui.painter().add(egui::Shape::Noop);
        let response = egui::Frame::new()
            .fill(t.color("surface-canvas"))
            .stroke(Stroke::new(1., t.color("border")))
            .corner_radius(t.number("r-lg") as u8)
            .inner_margin(t.number("s-5") as i8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_max_width(44. * t.number("text-sm") * 0.55);
                        ui.label(RichText::new(copy.title).size(t.number("text-md")));
                        ui.add_space(3.);
                        description(ui, t, copy.description, None);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if widgets::button_sized(ui, t, copy.reset, false, t.number("h-sm"))
                            .clicked()
                        {
                            let defaults = captures_settings::CustomThemeSettings::default();
                            self.custom_accent = defaults.accent;
                            self.custom_signal = defaults.signal;
                            self.commit_colors();
                        }
                    });
                });
                ui.add_space(t.number("s-5"));
                let gap = t.number("s-4");
                let width = (ui.available_width() - gap) / 2.;
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (key, label, detail) in copy.fields {
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 0.),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| self.color_field(ui, t, key, label, detail, width),
                        );
                    }
                });
            })
            .response;
        let text = copy.fields.iter().fold(
            format!("{} {}", copy.title, copy.description),
            |text, (_, label, detail)| format!("{text} {label} {detail}"),
        );
        self.remember(text, response.rect, background);
    }

    fn color_field(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        key: &str,
        title: &str,
        detail: &str,
        width: f32,
    ) {
        let old = string_at(&self.value, &["custom_theme", key]);
        let normalized = normalize_hex_color(&old).unwrap_or_else(|_| "#000000".into());
        let mut rgb = [0u8; 3];
        for (i, value) in rgb.iter_mut().enumerate() {
            *value =
                u8::from_str_radix(&normalized[1 + i * 2..3 + i * 2], 16).expect("normalized hex");
        }
        egui::Frame::new()
            .fill(t.color("surface-raised"))
            .stroke(Stroke::new(1., t.color("border-subtle")))
            .corner_radius(t.number("r-md") as u8)
            .inner_margin(t.number("s-4") as i8)
            .show(ui, |ui| {
                ui.set_width(width - 2. * t.number("s-4"));
                ui.spacing_mut().item_spacing.y = 0.;
                ui.label(RichText::new(title).size(t.number("text-sm")));
                ui.add_space(t.number("s-3"));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = t.number("s-3");
                    ui.spacing_mut().interact_size = egui::vec2(36., t.number("h-md"));
                    if ui
                        .color_edit_button_srgb(&mut rgb)
                        .on_hover_text(format!("{title} color picker"))
                        .changed()
                    {
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
                    let response = field_scope(ui, t, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(field)
                                .font(egui::FontId::monospace(t.number("text-xs")))
                                .margin(egui::vec2(t.number("s-3"), t.number("s-1")))
                                .desired_width(ui.available_width())
                                .min_size(egui::vec2(0., t.number("h-md")))
                                .align(egui::Align2::LEFT_CENTER),
                        )
                    });
                    let escape =
                        response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
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
                });
                ui.add_space(t.number("s-3"));
                ui.label(
                    RichText::new(detail)
                        .size(t.number("text-xs"))
                        .color(t.color("text-subtle")),
                );
            });
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
        self.card(ui, t, 1, |this, ui| {
            let copy = preferences::row("output_directory");
            let background = ui.painter().add(egui::Shape::Noop);
            let response = ui
                .vertical(|ui| {
                    ui.label(RichText::new(copy.title).size(t.number("text-md")));
                    ui.add_space(t.number("s-4"));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = t.number("s-4");
                        let mut path = string_at(&this.value, &["output_directory"]);
                        let choose = 2. * t.number("s-5") + 7. * t.number("text-sm");
                        let changed = field_scope(ui, t, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut path)
                                    .font(egui::FontId::monospace(t.number("text-sm")))
                                    .margin(egui::vec2(t.number("s-4"), t.number("s-2")))
                                    .desired_width(ui.available_width() - choose - t.number("s-4"))
                                    .min_size(egui::vec2(0., t.number("h-md")))
                                    .align(egui::Align2::LEFT_CENTER),
                            )
                        })
                        .changed();
                        if changed {
                            this.set(&["output_directory"], json!(path));
                        }
                        if ui
                            .add_enabled_ui(!this.folder_open, |ui| {
                                widgets::button(ui, t, "Choose…", false)
                            })
                            .inner
                            .clicked()
                        {
                            this.folder_open = true;
                            let out = this.out.clone();
                            let ctx = ui.ctx().clone();
                            thread::spawn(move || {
                                let result = rfd::FileDialog::new()
                                    .set_title("Choose capture folder")
                                    .pick_folder();
                                let _ = out.send(Message::Folder(result));
                                ctx.request_repaint_of(egui::ViewportId::ROOT);
                            });
                        }
                    });
                })
                .response;
            this.remember(
                format!("{} folder output directory", copy.title),
                response.rect,
                background,
            );
            for key in [
                "auto_copy_to_clipboard",
                "auto_start_on_selection",
                "show_mini_previews",
            ] {
                Self::divider(ui, t);
                let copy = preferences::row(key);
                this.toggle(ui, t, &[key], copy.title, copy.description, true);
            }
            Self::divider(ui, t);
            let previews = this.value["show_mini_previews"].as_bool().unwrap_or(false);
            let copy = preferences::row("mini_preview_placement");
            let placement = string_at(&this.value, &["mini_preview_placement"]);
            let mut chosen = None;
            this.row(
                ui,
                t,
                copy.title,
                copy.description,
                None,
                egui::vec2(108., 94.),
                |_, ui| {
                    ui.add_enabled_ui(previews, |ui| {
                        chosen = widgets::corner_picker(
                            ui,
                            t,
                            &preferences::MINI_PREVIEW_PLACEMENTS,
                            &placement,
                            preferences::mini_preview_placement_name(&placement),
                        );
                    });
                },
            );
            if let Some(value) = chosen {
                this.set(&["mini_preview_placement"], json!(value));
            }
            Self::divider(ui, t);
            let include_previews = this.value["include_mini_previews_in_captures"]
                .as_bool()
                .unwrap_or(false);
            let copy = preferences::row("include_mini_previews_in_captures");
            this.toggle(
                ui,
                t,
                &["include_mini_previews_in_captures"],
                copy.title,
                preferences::mini_previews_in_captures_description(previews, include_previews),
                previews,
            );
            Self::divider(ui, t);
            let include_controls = this.value["include_recording_controls_in_captures"]
                .as_bool()
                .unwrap_or(false);
            let can_exclude = !cfg!(target_os = "linux");
            let copy = preferences::row("include_recording_controls_in_captures");
            this.toggle_with(
                ui,
                t,
                &["include_recording_controls_in_captures"],
                copy.title,
                "",
                Some(preferences::recording_controls_description(
                    can_exclude,
                    include_controls,
                )),
                can_exclude,
            );
            for key in ["freeze_screen", "show_cursor_in_screenshots"] {
                Self::divider(ui, t);
                let copy = preferences::row(key);
                this.toggle(ui, t, &[key], copy.title, copy.description, true);
            }
            Self::divider(ui, t);
            this.combo(
                ui,
                t,
                &["screenshot_format"],
                &choices(&preferences::SCREENSHOT_FORMATS),
            );
            Self::divider(ui, t);
            this.combo(ui, t, &["screenshot_countdown_seconds"], &countdowns());
        });
    }

    fn shortcuts(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let help = preferences::shortcut_help(shortcut_platform());
        let mut intro = format!("{} {}", preferences::SECTIONS[2].description, help.intro);
        if !self.live {
            intro = format!("{intro} {}", preferences::FIXTURE_SHORTCUTS_NOTE);
        }
        self.card_described(ui, t, 2, &intro, |this, ui| {
            let live = this.live;
            let mut open = false;
            this.row(
                ui,
                t,
                help.system_title,
                help.system_body,
                None,
                egui::vec2(80., t.number("h-md")),
                |_, ui| {
                    open = ui
                        .add_enabled_ui(live, |ui| {
                            widgets::button(ui, t, help.system_action, false)
                        })
                        .inner
                        .clicked();
                },
            );
            if open {
                this.keyboard_settings_error =
                    preferences::open_keyboard_settings(shortcut_platform()).err();
            }
            if let Some(error) = &this.keyboard_settings_error {
                ui.add_space(t.number("s-2"));
                ui.colored_label(
                    t.color("danger-text"),
                    RichText::new(error).size(t.number("text-xs")),
                );
            }
            Self::divider(ui, t);
            for (index, field) in SHORTCUT_FIELDS.into_iter().enumerate() {
                if index > 0 {
                    ui.add_space(t.number("s-2"));
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
        let width = (ui.available_width() * 0.45).clamp(180., 260.);
        let height = t.number("h-md")
            + if error.is_some() {
                t.number("s-2") + 16.
            } else {
                0.
            };
        self.row(
            ui,
            t,
            field.label(),
            "",
            None,
            egui::vec2(width, height),
            |this, ui| {
                let response = ui
                    .allocate_ui_with_layout(
                        egui::vec2(width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let response =
                                shortcut_recorder(ui, t, field.label(), &keys, recording, width);
                            if let Some(error) = &error {
                                ui.add_space(t.number("s-2"));
                                ui.colored_label(
                                    t.color("danger-text"),
                                    RichText::new(error).size(t.number("text-xs")),
                                );
                            }
                            response
                        },
                    )
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
            },
        );
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
        self.card(ui, t, 3, |this, ui| {
            this.combo(
                ui,
                t,
                &["recording", "video_format"],
                &choices(&preferences::RECORDING_FORMATS),
            );
            Self::divider(ui, t);
            this.select_grid(
                ui,
                t,
                &[
                    (
                        &["recording", "video_fps"],
                        numbers(&preferences::RECORDING_FPS, preferences::fps_label),
                    ),
                    (
                        &["recording", "video_max_resolution"],
                        choices(&preferences::RESOLUTIONS),
                    ),
                ],
            );
            Self::divider(ui, t);
            this.combo(ui, t, &["recording", "countdown_seconds"], &countdowns());
            Self::divider(ui, t);
            this.microphone_combo(ui, t);
            for key in preferences::RECORDING_TOGGLES {
                Self::divider(ui, t);
                let copy = preferences::row(&format!("recording.{key}"));
                this.toggle(
                    ui,
                    t,
                    &["recording", key],
                    copy.title,
                    copy.description,
                    true,
                );
            }
        });
    }
    /// Shipping Default microphone select: Off, then each input device. A saved
    /// device that is not connected stays selectable by its id. Devices are
    /// enumerated off the UI thread the first time the menu opens, because
    /// ALSA/PulseAudio probing can be slow or start an audio daemon.
    fn microphone_combo(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        if let Some(devices) = self
            .microphones_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.microphones = Some(devices);
            self.microphones_rx = None;
        }
        let path = ["recording", "microphone_device_id"];
        let saved = at(&self.value, &path)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut options = vec![(None, preferences::MICROPHONE_OFF.to_owned())];
        options.extend(
            self.microphones
                .iter()
                .flatten()
                .map(|(id, name)| (Some(id.clone()), name.clone())),
        );
        if let Some(saved) = &saved
            && !options.iter().any(|(id, _)| id.as_ref() == Some(saved))
        {
            options.push((Some(saved.clone()), saved.clone()));
        }
        let loading = self.microphones.is_none();
        let mut open_requested = false;
        let mut chosen = None;
        let selected = options.iter().find(|(id, _)| *id == saved).map_or_else(
            || preferences::MICROPHONE_OFF.to_owned(),
            |(_, label)| label.clone(),
        );
        let copy = preferences::row("recording.microphone_device_id");
        self.row(
            ui,
            t,
            copy.title,
            copy.description,
            None,
            egui::vec2(160., t.number("h-md")),
            |_, ui| {
                widgets::select(ui, t, path.join("."), 160., &selected, |ui| {
                    open_requested = true;
                    for (id, label) in &options {
                        if ui.selectable_label(*id == saved, label).clicked() {
                            chosen = Some(id.clone());
                        }
                    }
                    if loading {
                        ui.add_enabled(false, egui::Label::new(preferences::MICROPHONES_LOADING));
                    }
                });
            },
        );
        if open_requested && loading && self.microphones_rx.is_none() {
            let (tx, rx) = mpsc::channel();
            let wake = ui.ctx().clone();
            std::thread::spawn(move || {
                let devices = captures_recording_platform::microphone_devices()
                    .into_iter()
                    .map(|device| (device.id, device.name))
                    .collect();
                let _ = tx.send(devices);
                wake.request_repaint_of(egui::ViewportId::ROOT);
            });
            self.microphones_rx = Some(rx);
        }
        if let Some(id) = chosen
            && id != saved
        {
            self.set(&path, id.map_or(Value::Null, Value::String));
        }
    }

    fn gif(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui, t, 4, |this, ui| {
            this.select_grid(
                ui,
                t,
                &[
                    (
                        &["recording", "gif_fps"],
                        numbers(&preferences::GIF_FPS, preferences::fps_label),
                    ),
                    (
                        &["recording", "gif_max_width"],
                        numbers(&preferences::GIF_MAX_WIDTHS, preferences::width_label),
                    ),
                    (
                        &["recording", "gif_max_colors"],
                        numbers(&preferences::GIF_PALETTE_COLORS, |v| v.to_string()),
                    ),
                ],
            );
        });
    }

    /// Shipping `.settings-utility-row`: copy and one secondary action.
    fn utility_row(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        title: &str,
        detail: &str,
        action: &str,
        enabled: bool,
    ) -> bool {
        let mut clicked = false;
        let reserve =
            2. * t.number("s-5") + action.chars().count() as f32 * 0.6 * t.number("text-sm");
        self.row(
            ui,
            t,
            title,
            detail,
            None,
            egui::vec2(reserve, t.number("h-md")),
            |_, ui| {
                clicked = ui
                    .add_enabled_ui(enabled, |ui| widgets::button(ui, t, action, false))
                    .inner
                    .clicked();
            },
        );
        clicked
    }

    fn updates(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui, t, 5, |this, ui| {
            this.utility_row(
                ui,
                t,
                preferences::UPDATES_TITLE,
                preferences::UPDATES_DETAIL,
                preferences::UPDATES_ACTION,
                false,
            );
            Self::divider(ui, t);
            let copy = preferences::row("show_update_changelog");
            this.toggle(
                ui,
                t,
                &["show_update_changelog"],
                copy.title,
                copy.description,
                true,
            );
        });
    }

    fn about(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.card(ui, t, 6, |this, ui| {
            if this.utility_row(
                ui,
                t,
                preferences::FEEDBACK_TITLE,
                preferences::FEEDBACK_DETAIL,
                preferences::FEEDBACK_ACTION,
                true,
            ) {
                this.feedback.open(ui.ctx());
            }
            Self::divider(ui, t);
            let detail = if let Some(error) = &this.login_error {
                preferences::login_item_error(error)
            } else if this.login_root.is_none() {
                preferences::login_item_unavailable(shortcut_platform()).to_owned()
            } else if this.login_pending {
                preferences::LOGIN_ITEM_CHECKING.to_owned()
            } else {
                preferences::LOGIN_ITEM_DETAIL.to_owned()
            };
            if this.login_error.is_some() {
                if this.utility_row(
                    ui,
                    t,
                    preferences::LOGIN_ITEM_TITLE,
                    &detail,
                    preferences::LOGIN_ITEM_RETRY,
                    !this.login_pending,
                ) {
                    this.request_login_item(None);
                }
                return;
            }
            let enabled = this.login_root.is_some() && !this.login_pending;
            let on = this.login_enabled == Some(true);
            ui.add_enabled_ui(enabled, |ui| {
                let mut switch_rect = egui::Rect::NOTHING;
                let (rect, _) = this.row(
                    ui,
                    t,
                    preferences::LOGIN_ITEM_TITLE,
                    &detail,
                    None,
                    egui::vec2(32., 19.),
                    |_, ui| {
                        switch_rect = ui
                            .allocate_exact_size(egui::vec2(32., 19.), egui::Sense::hover())
                            .0;
                    },
                );
                let response =
                    ui.interact(rect, ui.scope_id().with("login-item"), egui::Sense::click());
                if response.clicked() {
                    this.request_login_item(Some(!on));
                }
                widgets::paint_switch(
                    ui,
                    t,
                    switch_rect,
                    &response,
                    on,
                    preferences::LOGIN_ITEM_TITLE,
                );
            });
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

pub(crate) fn shortcut_platform() -> ShortcutPlatform {
    #[cfg(target_os = "windows")]
    return ShortcutPlatform::Windows;
    #[cfg(target_os = "linux")]
    return ShortcutPlatform::Linux;
    #[cfg(target_os = "macos")]
    return ShortcutPlatform::Macos;
    #[allow(unreachable_code)]
    ShortcutPlatform::Linux
}

/// Shipping `.shortcut-recorder`: a field with `<kbd>` key chips, or the
/// "Press shortcut…" prompt while empty.
fn shortcut_recorder(
    ui: &mut egui::Ui,
    t: &Tokens,
    label: &str,
    keys: &[String],
    recording: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, t.number("h-md")), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, recording, label)
    });
    let radius = t.number("r-md");
    let border = if recording || response.has_focus() {
        t.color("theme-accent")
    } else if response.hovered() {
        t.color("border-strong")
    } else {
        t.color("control-border")
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        radius,
        t.color("surface-field"),
        egui::Stroke::new(1., border),
        egui::StrokeKind::Inside,
    );
    if recording || response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.),
            radius + 2.,
            egui::Stroke::new(2., t.color("theme-accent").gamma_multiply(0.35)),
            egui::StrokeKind::Outside,
        );
    }
    let mut x = rect.left() + t.number("s-4");
    if keys.is_empty() {
        let prompt = painter.layout_no_wrap(
            preferences::SHORTCUT_PROMPT.into(),
            egui::FontId::proportional(t.number("text-sm")),
            t.color("text-faint"),
        );
        painter.galley(
            egui::pos2(x, rect.center().y - prompt.size().y / 2.),
            prompt,
            t.color("text-faint"),
        );
        return response;
    }
    // Shipping `kbd` chips; long chords tighten spacing, then shrink the
    // text (to 80%) rather than dropping keys.
    let base = t.number("text-2xs");
    let measure = |size: f32| -> Vec<f32> {
        keys.iter()
            .map(|key| {
                painter
                    .layout_no_wrap(
                        key.clone(),
                        egui::FontId::proportional(size),
                        t.color("text-muted"),
                    )
                    .size()
                    .x
            })
            .collect()
    };
    let available = rect.width() - 2. * t.number("s-4");
    let (scale, padding, gap) = fit_chips(&measure(base), available);
    for key in keys {
        let text = painter.layout_no_wrap(
            key.clone(),
            egui::FontId::proportional(base * scale),
            t.color("text-muted"),
        );
        let size = egui::vec2((text.size().x + 2. * padding).max(20.), text.size().y + 6.);
        let chip = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - size.y / 2.), size);
        if chip.right() > rect.right() - t.number("s-4") + 0.5 {
            break;
        }
        let chip_radius = t.number("r-xs");
        painter.rect(
            chip,
            chip_radius,
            t.color("surface-raised"),
            egui::Stroke::new(1., t.color("border")),
            egui::StrokeKind::Inside,
        );
        // `border-bottom-width: 2px`
        painter.line_segment(
            [
                chip.left_bottom() + egui::vec2(chip_radius, -1.5),
                chip.right_bottom() + egui::vec2(-chip_radius, -1.5),
            ],
            egui::Stroke::new(1., t.color("border")),
        );
        painter.galley(
            chip.center() - text.size() / 2. - egui::vec2(0., 1.),
            text,
            t.color("text-muted"),
        );
        x = chip.right() + gap;
    }
    response
}

/// Chip text scale, horizontal padding and gap so every key fits `available`:
/// shipping 5 pt padding and 6 pt gaps, then 4/4, then text down to 80%.
fn fit_chips(text_widths: &[f32], available: f32) -> (f32, f32, f32) {
    let total = |scale: f32, padding: f32, gap: f32| {
        text_widths
            .iter()
            .map(|width| (width * scale + 2. * padding).max(20.))
            .sum::<f32>()
            + gap * text_widths.len().saturating_sub(1) as f32
    };
    if total(1., 5., 6.) <= available {
        return (1., 5., 6.);
    }
    if total(1., 4., 4.) <= available {
        return (1., 4., 4.);
    }
    let mut scale = 1.;
    while scale > 0.8 && total(scale, 4., 4.) > available {
        scale -= 0.02;
    }
    (scale.max(0.8), 4., 4.)
}

/// Shipping `.setting-copy small`: `--text-sm` in `--text-subtle`, at most
/// 52ch wide, with an optional `<strong>` word in `--text`.
fn description_job(
    t: &Tokens,
    text: &str,
    emphasis: Option<preferences::Emphasized>,
    available: f32,
) -> egui::text::LayoutJob {
    let size = t.number("text-sm");
    let subtle = egui::TextFormat::simple(egui::FontId::proportional(size), t.color("text-subtle"));
    let strong = egui::TextFormat::simple(egui::FontId::proportional(size), t.color("text"));
    let mut job = egui::text::LayoutJob::default();
    match emphasis {
        Some(parts) => {
            job.append(parts.lead, 0., subtle.clone());
            job.append(parts.emphasis, 0., strong);
            job.append(parts.trail, 0., subtle);
        }
        None => job.append(text, 0., subtle),
    }
    job.wrap.max_width = (52. * size * 0.56).min(available);
    job
}

fn description(
    ui: &mut egui::Ui,
    t: &Tokens,
    text: &str,
    emphasis: Option<preferences::Emphasized>,
) {
    let job = description_job(t, text, emphasis, ui.available_width());
    ui.label(job);
}

/// Shipping text fields: `--surface-field`, `--control-border`, accent focus.
fn field_scope<R>(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        visuals.extreme_bg_color = t.color("surface-field");
        visuals.text_edit_bg_color = Some(t.color("surface-field"));
        visuals.widgets.inactive.bg_stroke = Stroke::new(1., t.color("control-border"));
        visuals.widgets.hovered.bg_stroke = Stroke::new(1., t.color("border-strong"));
        visuals.selection.stroke = Stroke::new(1., t.color("theme-accent"));
        ui.spacing_mut().button_padding.x = t.number("s-4");
        add(ui)
    })
    .inner
}

fn choices(values: &[(&str, &str)]) -> Vec<(Value, String)> {
    values
        .iter()
        .map(|(v, s)| (json!(v), (*s).into()))
        .collect()
}
fn numbers(values: &[u16], label: impl Fn(u16) -> String) -> Vec<(Value, String)> {
    values.iter().map(|v| (json!(v), label(*v))).collect()
}
fn countdowns() -> Vec<(Value, String)> {
    preferences::COUNTDOWN_SECONDS
        .map(|v| (json!(v), preferences::countdown_label(v)))
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

    #[test]
    fn login_item_is_explicit_pending_guarded_and_os_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), dir.path().join("settings.json"), None, None);
        prefs.io.flush();
        prefs.receive(&ctx);
        let (tx, rx) = mpsc::channel();
        prefs.io.tx = tx;
        prefs.request_login_item(Some(true));
        assert!(
            rx.try_recv().is_err(),
            "fixtures have no live login context"
        );
        prefs.connect_login_item(dir.path().to_owned());
        assert!(matches!(rx.try_recv(), Ok(Command::LoginItem(root, None)) if root == dir.path()));
        prefs.request_login_item(Some(true));
        assert!(rx.try_recv().is_err(), "query is still pending");
        prefs.out.send(Message::LoginItem(Ok(true))).unwrap();
        prefs.receive(&ctx);
        assert_eq!(prefs.login_enabled, Some(true));
        prefs.request_login_item(Some(false));
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::LoginItem(_, Some(false)))
        ));
        assert_eq!(
            prefs.login_enabled,
            Some(true),
            "no optimistic registration state"
        );
        prefs
            .out
            .send(Message::LoginItem(Err("collision".into())))
            .unwrap();
        prefs.receive(&ctx);
        assert_eq!(prefs.login_enabled, Some(true));
        assert_eq!(prefs.login_error.as_deref(), Some("collision"));
        assert!(!prefs.login_pending);
        prefs.request_login_item(None);
        assert!(matches!(rx.try_recv(), Ok(Command::LoginItem(_, None))));
        prefs.out.send(Message::LoginItem(Ok(false))).unwrap();
        prefs.receive(&ctx);
        assert_eq!(prefs.login_enabled, Some(false));
        assert!(prefs.login_error.is_none());
    }

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
    fn long_chords_tighten_then_shrink_chips_instead_of_dropping_keys() {
        assert_eq!(fit_chips(&[20., 22.], 200.), (1., 5., 6.));
        let widths = [18., 24., 16., 30., 110.];
        let (scale, padding, gap) = fit_chips(&widths, 228.);
        assert!((0.8..1.).contains(&scale));
        let total: f32 = widths
            .iter()
            .map(|w| (w * scale + 2. * padding).max(20.))
            .sum::<f32>()
            + gap * 4.;
        assert!(total <= 228.);
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
    fn capture_menu_target_scrolls_to_and_briefly_highlights_its_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        captures_settings::save(&path, &AppSettings::default()).unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), path, None, None);
        prefs.value = serde_json::to_value(AppSettings::default()).unwrap();
        prefs.load_error = None;
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let frame = |prefs: &mut Preferences| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 500.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    prefs.ui(ui, &tokens, true);
                },
            );
            output.textures_delta.clear();
        };
        for target in [
            PreferenceTarget::IncludeRecordingControlsInCaptures,
            PreferenceTarget::AutoStartOnSelection,
        ] {
            prefs.open_target(target);
            frame(&mut prefs);
            assert_eq!(prefs.highlighted_target(), Some(target));
            assert!(
                prefs
                    .highlight
                    .as_ref()
                    .is_some_and(|highlight| highlight.scrolled),
                "the linked row must be found and revealed"
            );
            prefs.highlight.as_mut().unwrap().until = Instant::now();
            frame(&mut prefs);
            assert_eq!(prefs.highlighted_target(), None, "highlight expires");
        }
    }

    #[test]
    fn cards_find_and_whole_row_switches_follow_shipping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        captures_settings::save(&path, &AppSettings::default()).unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), path, None, None);
        prefs.value = serde_json::to_value(AppSettings::default()).unwrap();
        prefs.load_error = None;
        let tokens = crate::tokens::load()["light-mustard"].clone();
        let frame = |prefs: &mut Preferences, events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(820., 6000.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    prefs.ui(ui, &tokens, false);
                },
            );
            output.textures_delta.clear();
        };
        frame(&mut prefs, vec![]);
        assert_eq!(prefs.card_tops.len(), preferences::SECTIONS.len());
        assert!(prefs.card_tops.windows(2).all(|pair| pair[0] < pair[1]));
        for section in preferences::SECTIONS {
            assert!(
                prefs
                    .find_rows
                    .iter()
                    .any(|row| row.text.starts_with(section.title)),
                "{} card header is a find target",
                section.title
            );
        }

        prefs.find_open = true;
        prefs.query = "freeze screen when".into();
        frame(&mut prefs, vec![]);
        assert_eq!(prefs.matches.len(), 1);
        assert_eq!(
            preferences::find_count_label(&prefs.query, prefs.matches.len(), prefs.match_index),
            "1 of 1"
        );

        // Clicking the copy (not just the switch) toggles, like the shipping label.
        let row = prefs
            .find_rows
            .iter()
            .find(|row| row.text.starts_with("Freeze screen when capturing"))
            .unwrap()
            .rect;
        let point = egui::pos2(row.left() + 20., row.center().y);
        assert!(prefs.value["freeze_screen"].as_bool().unwrap());
        frame(&mut prefs, vec![egui::Event::PointerMoved(point)]);
        let button = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut prefs, vec![button(true)]);
        frame(&mut prefs, vec![button(false)]);
        assert!(!prefs.value["freeze_screen"].as_bool().unwrap());
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
        assert!(matches!(rx.recv().unwrap(), Message::Onboarding(Ok(_))));
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
        assert!(matches!(rx.recv().unwrap(), Message::Onboarding(Err(_))));
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

    #[test]
    fn motion_refresh_coalesces_and_retains_reduction_when_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), path.clone(), None, None);
        prefs.io.flush();
        prefs.receive(&ctx);
        let (tx, rx) = mpsc::channel();
        prefs.io.tx = tx;
        assert!(!prefs.reduced_motion(false));
        assert!(
            prefs.reduced_motion(true),
            "explicit override wins over default"
        );
        prefs.refresh_motion_preference();
        prefs.refresh_motion_preference();
        assert!(matches!(rx.try_recv(), Ok(Command::MotionPreference)));
        assert!(rx.try_recv().is_err(), "only one request may be pending");
        for (reported, expected) in [(Some(true), true), (None, true), (Some(false), false)] {
            prefs.out.send(Message::MotionPreference(reported)).unwrap();
            prefs.receive(&ctx);
            assert!(!prefs.motion_pending);
            assert_eq!(prefs.reduced_motion(false), expected);
            assert!(
                prefs.reduced_motion(true),
                "OS response cannot clear explicit override"
            );
            prefs.refresh_motion_preference();
            assert!(matches!(rx.try_recv(), Ok(Command::MotionPreference)));
        }
        assert!(!path.exists(), "OS reads cannot save application settings");
    }

    #[test]
    fn onboarding_is_serialized_with_saves_and_fixture_paths_are_isolated() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_path = first.path().join("settings.json");
        let second_path = second.path().join("settings.json");
        let (tx, rx) = mpsc::channel();
        let mut io = SettingsIo::start(first_path.clone(), tx, || {});
        assert!(matches!(rx.recv().unwrap(), Message::Loaded(Ok(_))));
        assert!(matches!(rx.recv().unwrap(), Message::Onboarding(Ok(_))));

        let settings = AppSettings {
            screenshot_countdown_seconds: 6,
            ..Default::default()
        };
        io.tx.send(Command::Save(1, Box::new(settings))).unwrap();
        io.tx
            .send(Command::Onboarding(
                captures_app::onboarding::Action::Complete,
            ))
            .unwrap();
        io.flush();

        assert!(matches!(rx.recv().unwrap(), Message::Saved(1, Ok(_))));
        assert!(matches!(rx.recv().unwrap(), Message::Onboarding(Ok(_))));
        let saved = captures_settings::load(&first_path).unwrap();
        assert!(saved.onboarding_completed);
        assert_eq!(saved.screenshot_countdown_seconds, 6);
        let untouched = captures_settings::load(&second_path).unwrap();
        assert!(!untouched.onboarding_completed);
        assert!(!second_path.exists());
    }

    #[test]
    fn onboarding_loading_busy_and_error_states_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), path, None, None);
        assert!(prefs.onboarding_pending());
        assert!(!prefs.onboarding_complete());
        prefs.io.flush();
        prefs.receive(&ctx);
        assert!(!prefs.onboarding_pending());
        let (tx, rx) = mpsc::channel();
        prefs.io.tx = tx;
        prefs.complete_onboarding();
        prefs.complete_onboarding(); // A busy action is deliberately ignored.
        assert!(prefs.onboarding_pending());
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Onboarding(
                captures_app::onboarding::Action::Complete
            ))
        ));
        assert!(
            rx.try_recv().is_err(),
            "busy completion cannot enqueue twice"
        );
        let mut state = prefs.onboarding.take().unwrap();
        state.onboarding_completed = true;
        prefs.out.send(Message::Onboarding(Ok(state))).unwrap();
        prefs.receive(&ctx);
        assert!(prefs.onboarding_complete());

        prefs
            .out
            .send(Message::Onboarding(Err("malformed settings".into())))
            .unwrap();
        prefs.receive(&ctx);
        assert!(!prefs.onboarding_complete());
        assert_eq!(prefs.onboarding_error(), Some("malformed settings"));
    }

    #[test]
    fn permission_recovery_error_preserves_completed_setup_and_done_never_completes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut prefs = Preferences::new(ctx.clone(), dir.path().join("settings.json"), None, None);
        prefs.io.flush();
        prefs.receive(&ctx);
        prefs.onboarding.as_mut().unwrap().onboarding_completed = true;

        let (tx, rx) = mpsc::channel();
        prefs.io.tx = tx;
        prefs.open_permission_recovery();
        prefs.close_permission_recovery();
        assert!(
            prefs.permission_recovery_open(),
            "In-flight checks retain the dialog"
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::PermissionRecovery(
                captures_app::onboarding::Action::Check
            ))
        ));
        prefs
            .out
            .send(Message::PermissionRecovery(Err("check failed".into())))
            .unwrap();
        prefs.receive(&ctx);
        assert!(prefs.onboarding_complete());
        assert_eq!(prefs.permission_recovery_error(), Some("check failed"));
        assert!(!prefs.permission_recovery_busy());

        prefs.close_permission_recovery();
        assert!(!prefs.permission_recovery_open());
        assert!(rx.try_recv().is_err(), "Done must not enqueue Complete");

        prefs.permission_recovery_open = true;
        prefs.permission_recovery = Some(captures_app::onboarding::State {
            platform: "test".into(),
            onboarding_completed: true,
            screen_recording_required: true,
            screen_recording_granted: false,
            screen_recording_can_request: false,
            screen_recording_requested_this_launch: true,
            microphone_granted: false,
            microphone_can_request: false,
            microphone_requested_this_launch: false,
        });
        prefs.close_permission_recovery();
        assert!(!prefs.permission_recovery_open(), "Done works while denied");
        assert!(rx.try_recv().is_err(), "Done must remain a local close");
    }
}
