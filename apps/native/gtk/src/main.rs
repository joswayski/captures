//! Captures' Linux host on GTK4: OS text, input and AT-SPI with Captures'
//! own token-drawn surfaces and controls. Runs beside the wgpu workbench
//! until it passes the feature, screenshot, performance and accessibility
//! gates in `docs/native-rewrite.md`.
mod editor;
mod feedback;
mod history;
mod live;
mod preferences;
mod previews;
mod recording;
mod style;
mod switch;
mod system_look;
mod tokens;
mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib, graphene};

const USAGE: &str = "captures-gtk [--scene preferences|history|editor|recording|preview|feedback] [--artifact ID] [--appearance light|dark]\n\
  [--settings-file PATH] [--history-root PATH] [--size WxH] [--feedback-endpoint URL]\n\
  [--screenshot OUT.png] [--find QUERY] [--count N] [--exercise] [--quit-after SECONDS]\n\
  [--open-media PATH]... [-- FILE...]\n\
  --live [--captures-autostart] [--history-root PATH] [--settings-file PATH] [--open-media PATH]... [-- FILE...]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    Preferences,
    History,
    Editor,
    Recording,
    Preview,
    Feedback,
}

impl Scene {
    fn name(self) -> &'static str {
        match self {
            Scene::Preferences => "preferences",
            Scene::History => "history",
            Scene::Editor => "editor",
            Scene::Recording => "recording",
            Scene::Preview => "preview",
            Scene::Feedback => "feedback",
        }
    }
}

struct Options {
    scene: Scene,
    appearance: Option<String>,
    settings_file: PathBuf,
    history_root: Option<PathBuf>,
    artifact: Option<String>,
    screenshot: Option<PathBuf>,
    /// Preferences opens Find with this query.
    find: Option<String>,
    /// Where Send feedback posts; without it the form is a fixture.
    feedback_endpoint: Option<String>,
    size: Option<(i32, i32)>,
    /// How many of the newest entries `--scene preview` restores.
    count: usize,
    exercise: bool,
    quit_after: Option<Duration>,
    /// Open With: files to import into History and edit.
    open: Vec<PathBuf>,
    /// `--scene` was given; without it, files open only their editors.
    scene_given: bool,
    settings_given: bool,
    /// Run as the resident app (`live.rs`).
    live: bool,
    /// `--captures-autostart`: a quiet launch that opens no window.
    quiet: bool,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        scene: Scene::Preferences,
        appearance: None,
        settings_file: captures_settings::default_native_settings_path(),
        history_root: None,
        artifact: None,
        screenshot: None,
        find: None,
        feedback_endpoint: None,
        size: None,
        count: 2,
        exercise: false,
        quit_after: None,
        open: Vec::new(),
        scene_given: false,
        settings_given: false,
        live: false,
        quiet: false,
    };
    let mut args = args.skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--scene" => {
                options.scene_given = true;
                options.scene = match value()?.as_str() {
                    "preferences" => Scene::Preferences,
                    "history" => Scene::History,
                    "editor" => Scene::Editor,
                    "recording" => Scene::Recording,
                    "preview" => Scene::Preview,
                    "feedback" => Scene::Feedback,
                    other => return Err(format!("unsupported scene {other}")),
                }
            }
            "--artifact" => options.artifact = Some(value()?),
            "--appearance" => options.appearance = Some(value()?),
            "--settings-file" => {
                options.settings_given = true;
                options.settings_file = value()?.into();
            }
            "--live" => options.live = true,
            "--captures-autostart" => options.quiet = true,
            "--history-root" => options.history_root = Some(value()?.into()),
            "--screenshot" => options.screenshot = Some(value()?.into()),
            "--find" => options.find = Some(value()?),
            "--feedback-endpoint" => options.feedback_endpoint = Some(value()?),
            "--open-media" => options.open.push(value()?.into()),
            // Everything after `--` is a local path (Open With).
            "--" => options.open.extend(args.by_ref().map(PathBuf::from)),
            "--count" => options.count = value()?.parse().map_err(|_| "bad --count")?,
            "--exercise" => options.exercise = true,
            "--quit-after" => {
                let seconds: f64 = value()?.parse().map_err(|_| "bad --quit-after")?;
                if seconds.is_nan() || seconds <= 0. {
                    return Err("--quit-after must be positive".into());
                }
                options.quit_after = Some(Duration::from_secs_f64(seconds));
            }
            "--size" => {
                let size = value()?;
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                options.size = Some((
                    w.parse().map_err(|_| "bad width")?,
                    h.parse().map_err(|_| "bad height")?,
                ));
            }
            "--help" => return Err(USAGE.into()),
            _ => return Err(format!("unknown argument {flag}\n{USAGE}")),
        }
    }
    if options.live
        && (options.scene_given
            || options.screenshot.is_some()
            || options.exercise
            || options.artifact.is_some())
    {
        return Err(
            "--live opens windows itself; drop --scene, --artifact, --screenshot and --exercise"
                .into(),
        );
    }
    if options.quiet && !options.live {
        return Err("--captures-autostart needs --live".into());
    }
    if matches!(options.scene, Scene::Editor | Scene::Recording) && options.artifact.is_none() {
        return Err(format!(
            "--scene {} needs --artifact ID",
            options.scene.name()
        ));
    }
    Ok(options)
}

/// One JSON line on stdout for `apps/native/profile.py`.
fn emit(event: &str, mut fields: serde_json::Value) {
    fields["event"] = event.into();
    println!("{fields}");
}

/// Windows shared across the app: one Preferences and one History.
struct Host {
    app: gtk::Application,
    /// The current look; Preferences replaces it when Appearance changes.
    tokens: RefCell<Rc<tokens::Tokens>>,
    /// `--appearance`, until the user picks one in Preferences.
    forced_appearance: RefCell<Option<String>>,
    provider: gtk::CssProvider,
    settings_file: PathBuf,
    /// The live profile's History, when Captures runs as the app.
    live_history_root: Option<PathBuf>,
    history_root: PathBuf,
    preferences: RefCell<Option<gtk::ApplicationWindow>>,
    history: RefCell<Option<Rc<history::History>>>,
    /// Open screenshot editors, one per artifact.
    editors: RefCell<Vec<(String, Rc<editor::Editor>)>>,
    /// Open recording editors, one per artifact.
    recordings: RefCell<Vec<(String, Rc<recording::RecordingEditor>)>>,
    previews: RefCell<Option<Rc<previews::Previews>>>,
    /// `--find`, for the first Preferences window.
    find: RefCell<Option<String>>,
    feedback: RefCell<Option<gtk::ApplicationWindow>>,
    feedback_endpoint: Option<String>,
    /// Holds the app while Open With imports run before any window opens.
    opening: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// Open With files waiting for the import worker, in arrival order.
    pending: RefCell<VecDeque<PathBuf>>,
    importing: Cell<bool>,
    /// `--live`: the profile election and the hold that keeps the resident
    /// app running with no window.
    instance: RefCell<Option<captures_app::instance::Instance>>,
    resident: RefCell<Option<gio::ApplicationHoldGuard>>,
}

impl Host {
    fn show_preferences(self: &Rc<Self>) -> gtk::ApplicationWindow {
        if let Some(window) = self.preferences.borrow().as_ref() {
            window.present();
            return window.clone();
        }
        let window = preferences::build(
            &self.app,
            self.tokens.borrow().clone(),
            preferences::Options {
                settings_file: self.settings_file.clone(),
                history_root: self.live_history_root.clone(),
                on_look: Box::new({
                    let host = Rc::downgrade(self);
                    move |settings, appearance_changed| {
                        let host = host.upgrade().expect("host outlives Preferences");
                        host.apply_look(settings, appearance_changed)
                    }
                }),
                find: self.find.borrow_mut().take(),
                open_feedback: Box::new({
                    let host = Rc::downgrade(self);
                    move || {
                        if let Some(host) = host.upgrade() {
                            host.show_feedback();
                        }
                    }
                }),
            },
            {
                let host = Rc::downgrade(self);
                move || {
                    if let Some(host) = host.upgrade() {
                        host.show_history();
                    }
                }
            },
        );
        // Forget windows on close, not destroy: GTK 4 emits `destroy` only
        // once the last reference drops, and these handles are references,
        // so a closed window would be re-presented without its toplevel
        // entry (gone from AT-SPI) and with its stale state.
        let host = Rc::downgrade(self);
        window.connect_close_request(move |_| {
            if let Some(host) = host.upgrade() {
                host.preferences.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        *self.preferences.borrow_mut() = Some(window.clone());
        window.present();
        window
    }

    /// Send Feedback: one window, reused while open.
    fn show_feedback(self: &Rc<Self>) -> gtk::ApplicationWindow {
        if let Some(window) = self.feedback.borrow().as_ref() {
            window.present();
            return window.clone();
        }
        let window = feedback::build(
            &self.app,
            &self.tokens.borrow(),
            self.feedback_endpoint.clone(),
        );
        let host = Rc::downgrade(self);
        window.connect_close_request(move |_| {
            if let Some(host) = host.upgrade() {
                host.feedback.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        *self.feedback.borrow_mut() = Some(window.clone());
        window.present();
        window
    }

    /// Restyle every window for the look `settings` asks for. Widgets that
    /// draw from tokens pick the new look up when they are next built.
    fn apply_look(
        &self,
        settings: &captures_settings::AppSettings,
        appearance_changed: bool,
    ) -> Rc<tokens::Tokens> {
        if appearance_changed {
            self.forced_appearance.borrow_mut().take();
        }
        let forced = self.forced_appearance.borrow().clone();
        let (_, tokens) = tokens::Tokens::for_settings(settings, forced.as_deref());
        let tokens = Rc::new(tokens);
        self.provider.load_from_data(&style::css(&tokens));
        *self.tokens.borrow_mut() = tokens.clone();
        tokens
    }

    /// The desktop switched light and dark: a System look follows it.
    fn follow_system(&self) {
        if self.forced_appearance.borrow().is_some() {
            return;
        }
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        if settings.appearance != captures_settings::Appearance::System {
            return;
        }
        let (appearance, _) = tokens::Tokens::for_settings(&settings, None);
        // Dark tokens have light text.
        let current = if self.tokens.borrow().rgba("text").red() < 0.5 {
            "light"
        } else {
            "dark"
        };
        if appearance != current {
            preferences::restyle(self.apply_look(&settings, false));
        }
    }

    fn show_history(self: &Rc<Self>) -> Rc<history::History> {
        if let Some(history) = self.history.borrow().as_ref() {
            history.window.present();
            history.reload();
            return history.clone();
        }
        let host = Rc::downgrade(self);
        let history = Rc::new(history::build(
            &self.app,
            self.tokens.borrow().clone(),
            self.history_root.clone(),
            history::Actions {
                open_preferences: Box::new(move || {
                    if let Some(host) = host.upgrade() {
                        host.show_preferences();
                    }
                }),
                edit: Box::new({
                    let host = Rc::downgrade(self);
                    move |entry| {
                        if let Some(host) = host.upgrade() {
                            host.edit(entry);
                        }
                    }
                }),
                restore: Box::new({
                    let host = Rc::downgrade(self);
                    move |entry| {
                        if let Some(host) = host.upgrade() {
                            host.show_preview(entry.clone());
                        }
                    }
                }),
            },
        ));
        let host = Rc::downgrade(self);
        history.window.connect_close_request(move |_| {
            if let Some(host) = host.upgrade() {
                host.history.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        *self.history.borrow_mut() = Some(history.clone());
        history.window.present();
        history
    }
}

impl Host {
    /// The floating preview stack, created on first Restore.
    fn show_preview(
        self: &Rc<Self>,
        entry: captures_history::HistoryEntry,
    ) -> Rc<previews::Previews> {
        let existing = self.previews.borrow().clone();
        let previews = existing.unwrap_or_else(|| {
            let edit = Rc::downgrade(self);
            let changed = Rc::downgrade(self);
            let editing = Rc::downgrade(self);
            let placement = captures_settings::load(&self.settings_file)
                .unwrap_or_default()
                .mini_preview_placement;
            let previews = Rc::new(previews::build(
                &self.app,
                &self.tokens.borrow(),
                placement,
                self.history_root.clone(),
                previews::Actions {
                    edit: Box::new(move |entry| {
                        if let Some(host) = edit.upgrade() {
                            host.edit(entry);
                        }
                    }),
                    changed: Box::new(move || {
                        if let Some(host) = changed.upgrade()
                            && let Some(history) = host.history.borrow().as_ref()
                        {
                            history.reload();
                        }
                    }),
                    editing: Box::new(move |id| {
                        editing.upgrade().is_some_and(|host| {
                            host.editors.borrow().iter().any(|(open, _)| open == id)
                                || host.recordings.borrow().iter().any(|(open, _)| open == id)
                        })
                    }),
                },
            ));
            *self.previews.borrow_mut() = Some(previews.clone());
            previews
        });
        previews.restore(entry);
        previews
    }

    fn sync_preview_editing(&self) {
        if let Some(previews) = self.previews.borrow().as_ref() {
            previews.sync_editing();
        }
    }

    /// Open With: import each file into History off the UI thread, as
    /// shipping does, then open its editor. A file already in History opens
    /// that entry; one already being edited focuses its window. Files
    /// import one at a time in arrival order, including forwarded ones.
    fn open_media(self: &Rc<Self>, paths: Vec<PathBuf>) {
        self.pending.borrow_mut().extend(paths);
        if !self.importing.get() {
            self.import_next();
        }
    }

    fn import_next(self: &Rc<Self>) {
        let Some(path) = self.pending.borrow_mut().pop_front() else {
            self.importing.set(false);
            self.opening.borrow_mut().take();
            return;
        };
        self.importing.set(true);
        let root = self.history_root.clone();
        let open_artifact_ids = self
            .editors
            .borrow()
            .iter()
            .map(|(id, _)| id)
            .chain(self.recordings.borrow().iter().map(|(id, _)| id))
            .cloned()
            .collect();
        let host = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (ffmpeg, ffprobe) = recording::media_tools();
            let request = captures_app::Request::OpenMedia {
                root,
                path: path.clone(),
                open_artifact_ids,
                ffmpeg: Some(ffmpeg),
                ffprobe: Some(ffprobe),
            };
            let result = gio::spawn_blocking(move || captures_app::execute(request))
                .await
                .map_err(|_| "the import worker stopped".to_owned())
                .and_then(|result| result.map_err(|error| error.to_string()));
            let Some(host) = host.upgrade() else { return };
            match result {
                Ok(captures_app::Response::OpenedMedia { artifact, .. }) => {
                    emit("media-opened", serde_json::json!({"id": artifact.entry.id}));
                    if let Some(history) = host.history.borrow().as_ref() {
                        history.reload();
                    }
                    host.edit(&artifact.entry);
                }
                Ok(_) => unreachable!("OpenMedia returns OpenedMedia"),
                Err(error) => {
                    emit(
                        "media-open-failed",
                        serde_json::json!({"path": path, "error": error}),
                    );
                    host.show_error(&error);
                }
            }
            host.import_next();
        });
    }

    /// Shipping's open-media error: the system's "Captures" message dialog
    /// with an OK button.
    #[allow(deprecated)] // GtkAlertDialog needs GTK 4.10.
    fn show_error(&self, error: &str) {
        let dialog = gtk::MessageDialog::builder()
            .title("Captures")
            .text(error)
            .message_type(gtk::MessageType::Error)
            .buttons(gtk::ButtonsType::Ok)
            .modal(true)
            .build();
        if let Some(parent) = self.app.active_window() {
            dialog.set_transient_for(Some(&parent));
        }
        dialog.set_application(Some(&self.app));
        dialog.connect_response(|dialog, _| dialog.destroy());
        dialog.present();
    }

    /// Edit opens the editor that matches the artifact's kind.
    fn edit(self: &Rc<Self>, entry: &captures_history::HistoryEntry) {
        if entry.kind.is_recording() {
            self.show_recording(&entry.id);
        } else {
            self.show_editor(&entry.id, entry.mode, &entry.created_at);
        }
    }

    /// The recording editor for one History recording; reopening focuses it.
    fn show_recording(self: &Rc<Self>, artifact_id: &str) -> Rc<recording::RecordingEditor> {
        if let Some((_, editor)) = self
            .recordings
            .borrow()
            .iter()
            .find(|(id, _)| id == artifact_id)
        {
            editor.present();
            return editor.clone();
        }
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        let editor = Rc::new(recording::build(
            &self.app,
            self.tokens.borrow().clone(),
            recording::Options {
                history_root: self.history_root.clone(),
                artifact_id: artifact_id.to_owned(),
                output_directory: settings.output_directory.into(),
                preferences: settings.recording,
            },
        ));
        let host = Rc::downgrade(self);
        let id = artifact_id.to_owned();
        editor.window.connect_close_request(move |_| {
            if let Some(host) = host.upgrade() {
                host.recordings.borrow_mut().retain(|(open, _)| *open != id);
                host.sync_preview_editing();
            }
            glib::Propagation::Proceed
        });
        self.recordings
            .borrow_mut()
            .push((artifact_id.to_owned(), editor.clone()));
        self.sync_preview_editing();
        editor.present();
        editor
    }

    /// The screenshot editor for one History artifact; reopening focuses it.
    fn show_editor(
        self: &Rc<Self>,
        artifact_id: &str,
        mode: Option<captures_capture::CaptureMode>,
        created_at: &str,
    ) -> Rc<editor::Editor> {
        if let Some((_, editor)) = self
            .editors
            .borrow()
            .iter()
            .find(|(id, _)| id == artifact_id)
        {
            editor.present();
            return editor.clone();
        }
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        let editor = Rc::new(editor::build(
            &self.app,
            self.tokens.borrow().clone(),
            editor::Options {
                history_root: self.history_root.clone(),
                artifact_id: artifact_id.to_owned(),
                output_directory: settings.output_directory.into(),
                created_at: created_at.to_owned(),
            },
            mode.unwrap_or(captures_capture::CaptureMode::Region),
        ));
        let host = Rc::downgrade(self);
        let id = artifact_id.to_owned();
        editor.window.connect_close_request(move |_| {
            if let Some(host) = host.upgrade() {
                host.editors.borrow_mut().retain(|(open, _)| *open != id);
                host.sync_preview_editing();
            }
            glib::Propagation::Proceed
        });
        self.editors
            .borrow_mut()
            .push((artifact_id.to_owned(), editor.clone()));
        self.sync_preview_editing();
        editor.present();
        editor
    }
}

fn main() -> glib::ExitCode {
    let mut options = match parse(std::env::args()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::from(2);
        }
    };
    let started = Instant::now();
    // Election precedes GTK, workers and windows; fixtures never take part.
    let instance = if options.live {
        if options.history_root.is_none() && !options.settings_given {
            match captures_app::preview_channel::startup() {
                Ok(Some(startup)) => {
                    options.history_root = Some(startup.history_root);
                    options.settings_file = startup.settings_file;
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("Captures could not start: {error}");
                    return glib::ExitCode::FAILURE;
                }
            }
        }
        let root = options
            .history_root
            .clone()
            .unwrap_or_else(captures_app::default_history_root);
        match live::elect(&root, &options.open) {
            Ok(Some(instance)) => Some(instance),
            Ok(None) => {
                emit(
                    "forwarded",
                    serde_json::json!({"paths": options.open.len()}),
                );
                return glib::ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("Captures could not start: {error}");
                return glib::ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let instance = RefCell::new(instance);
    editor::register_fonts();
    let app = gtk::Application::builder()
        .application_id("es.captur.Captures.Gtk")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let options = Rc::new(RefCell::new(Some(options)));
    let hosts: Rc<RefCell<Vec<Rc<Host>>>> = Rc::default();
    let shutdown = hosts.clone();
    app.connect_activate(move |app| {
        let Some(options) = options.borrow_mut().take() else {
            return;
        };
        if options.appearance.is_none() {
            system_look::start();
        }
        let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
        let (appearance, tokens) =
            tokens::Tokens::for_settings(&settings, options.appearance.as_deref());
        let tokens = Rc::new(tokens);
        let provider = style::install(&tokens);
        let host = Rc::new(Host {
            app: app.clone(),
            tokens: RefCell::new(tokens),
            forced_appearance: RefCell::new(options.appearance.clone()),
            provider,
            settings_file: options.settings_file.clone(),
            live_history_root: options.history_root.clone(),
            history_root: options
                .history_root
                .clone()
                .unwrap_or_else(captures_app::default_history_root),
            preferences: RefCell::default(),
            history: RefCell::default(),
            editors: RefCell::default(),
            recordings: RefCell::default(),
            previews: RefCell::default(),
            find: RefCell::new(options.find.clone()),
            feedback: RefCell::default(),
            feedback_endpoint: options.feedback_endpoint.clone(),
            opening: RefCell::default(),
            pending: RefCell::default(),
            importing: Cell::new(false),
            instance: RefCell::default(),
            resident: RefCell::default(),
        });
        hosts.borrow_mut().push(host.clone());
        let weak = Rc::downgrade(&host);
        system_look::watch(move || {
            if let Some(host) = weak.upgrade() {
                host.follow_system();
            }
        });
        if let Some(instance) = instance.borrow_mut().take() {
            live::start(&host, instance, options.quiet, !options.open.is_empty());
            host.open_media(options.open.clone());
            emit(
                "ready",
                serde_json::json!({
                    "scene": "live", "renderer": "gtk4",
                    "gsk": std::env::var("GSK_RENDERER").ok(),
                    "os": std::env::consts::OS, "appearance": appearance,
                    "launchToReadyMs": started.elapsed().as_secs_f64() * 1000.,
                    "windows": app.windows().len(),
                }),
            );
            if let Some(after) = options.quit_after {
                let app = app.clone();
                glib::timeout_add_local_once(after, move || app.quit());
            }
            return;
        }
        // Open With alone opens only the files' editors, as shipping.
        if !options.open.is_empty() {
            host.open_media(options.open.clone());
            if !options.scene_given {
                if let Some(after) = options.quit_after {
                    let app = app.clone();
                    glib::timeout_add_local_once(after, move || app.quit());
                }
                // Keep running while the imports have no window yet.
                *host.opening.borrow_mut() = Some(app.hold());
                return;
            }
        }
        let window: gtk::ApplicationWindow = match options.scene {
            Scene::Preferences => host.show_preferences(),
            Scene::Feedback => host.show_feedback(),
            Scene::History => host.show_history().window.clone(),
            Scene::Editor => {
                let id = options.artifact.clone().expect("checked in parse");
                let entry = captures_history::load(&host.history_root, chrono::Utc::now())
                    .ok()
                    .and_then(|entries| entries.into_iter().find(|e| e.id == id));
                let (mode, created_at) = entry.map(|e| (e.mode, e.created_at)).unwrap_or_default();
                host.show_editor(&id, mode, &created_at).window.clone()
            }
            Scene::Recording => {
                let id = options.artifact.clone().expect("checked in parse");
                host.show_recording(&id).window.clone()
            }
            // `--artifact ID` restores one entry; otherwise the `--count` newest.
            Scene::Preview => {
                let entries = captures_history::load(&host.history_root, chrono::Utc::now())
                    .unwrap_or_default();
                let mut chosen: Vec<_> = match &options.artifact {
                    Some(id) => entries.into_iter().filter(|e| e.id == *id).collect(),
                    None => entries.into_iter().take(options.count).collect(),
                };
                chosen.reverse();
                let mut shown = None;
                for entry in chosen {
                    shown = Some(host.show_preview(entry));
                }
                match shown {
                    Some(previews) => previews.window.clone(),
                    None => {
                        eprintln!("no History entries to preview");
                        app.quit();
                        return;
                    }
                }
            }
        };
        if let Some((w, h)) = options.size {
            window.set_default_size(w, h);
        }
        let scene = options.scene;
        let ready = move || {
            emit(
                "ready",
                serde_json::json!({
                    "scene": scene.name(), "renderer": "gtk4",
                    "gsk": std::env::var("GSK_RENDERER").ok(),
                    "os": std::env::consts::OS, "appearance": appearance,
                    "launchToMapMs": started.elapsed().as_secs_f64() * 1000.,
                    "readiness": "window mapped; not first presentation",
                    "displayEnvironment": {
                        "wayland": std::env::var("WAYLAND_DISPLAY").ok(),
                        "x11": std::env::var("DISPLAY").ok()
                    }
                }),
            );
        };
        if window.is_mapped() {
            ready();
        } else {
            let ready = RefCell::new(Some(ready));
            window.connect_map(move |_| {
                if let Some(ready) = ready.borrow_mut().take() {
                    ready();
                }
            });
        }
        if options.exercise {
            exercise(&host, scene, started);
        }
        if let Some(after) = options.quit_after {
            let app = app.clone();
            glib::timeout_add_local_once(after, move || app.quit());
        }
        if let Some(path) = options.screenshot {
            let window = window.clone();
            glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                if let Err(error) = screenshot(&window, &path) {
                    eprintln!("screenshot failed: {error}");
                }
                window.application().expect("app").quit();
            });
        }
    });
    // A resident app quits on SIGINT/SIGTERM through the normal shutdown,
    // which releases the profile election.
    for signal in [2, 15] {
        let app = app.clone();
        glib::unix_signal_add_local(signal, move || {
            app.quit();
            glib::ControlFlow::Continue
        });
    }
    app.connect_shutdown(move |_| {
        for host in shutdown.borrow().iter() {
            live::stop(host);
        }
    });
    app.run_with_args::<&str>(&[])
}

/// `profile.py --exercise`: six scripted mutations at 2, 6, … 22 s, like the
/// wgpu workbench's `exercise_at`, each reported as `scripted-action`.
fn exercise(host: &Rc<Host>, scene: Scene, started: Instant) {
    for cycle in 0..6u64 {
        let host = host.clone();
        let at = Duration::from_secs(2 + cycle * 4).saturating_sub(started.elapsed());
        glib::timeout_add_local_once(at, move || {
            let start = Instant::now();
            match scene {
                // Shipping-equivalent of the workbench's appearance flip: the
                // stylesheet is rebuilt and every widget restyles.
                Scene::Preferences => {
                    let appearance = if cycle % 2 == 0 { "dark" } else { "light" };
                    let tokens = tokens::Tokens::variant(appearance, "mustard");
                    host.provider.load_from_data(&style::css(&tokens));
                }
                Scene::Preview => {
                    if let Some(previews) = host.previews.borrow().as_ref() {
                        previews.toggle_collapsed();
                    }
                }
                Scene::Editor => {
                    if let Some((_, editor)) = host.editors.borrow().first() {
                        editor.set_zoom(if cycle % 2 == 0 { 200. } else { 50. });
                    }
                }
                // Trim nudges re-stage the edit like the workbench's trim exercise.
                Scene::Recording => {
                    if let Some((_, editor)) = host.recordings.borrow().first() {
                        editor.nudge_trim(cycle % 2 == 0);
                    }
                }
                Scene::History => {
                    if let Some(history) = host.history.borrow().as_ref() {
                        history.scroll_to_end(cycle % 2 == 0);
                    }
                }
                // A static form: nothing to exercise.
                Scene::Feedback => {}
            }
            emit(
                "scripted-action",
                serde_json::json!({"scene": scene.name(), "cycle": cycle,
                "milliseconds": start.elapsed().as_secs_f64() * 1000.,
                "note": "CPU mutation, not presentation or hardware input latency"}),
            );
        });
    }
}

/// Render the window's content (without decorations) to a PNG.
fn screenshot(window: &gtk::ApplicationWindow, path: &std::path::Path) -> Result<(), String> {
    let child = window.child().ok_or("no content")?;
    let (w, h) = (child.width() as f32, child.height() as f32);
    let paintable = gtk::WidgetPaintable::new(Some(&child));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("empty frame")?;
    let renderer = window.renderer().ok_or("no renderer")?;
    let texture = renderer.render_texture(&node, Some(&graphene::Rect::new(0., 0., w, h)));
    texture.save_to_png(path).map_err(|e| e.to_string())
}
