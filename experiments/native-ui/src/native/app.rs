use crate::{
    capture,
    desktop::{self, Action, Desktop},
    editor, history, preferences,
    preview::{Preview, PreviewCallbacks, PreviewMetadata},
    recording,
    settings::Settings,
    ui,
};
use captures_capture::CaptureMode;
use captures_recording::{AudioOptions, GifOptions, RecordingKind, RecordingOptions};
use gtk::{gdk, gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

struct App {
    application: gtk::Application,
    _hold: gio::ApplicationHoldGuard,
    window: gtk::Window,
    settings: Rc<RefCell<Settings>>,
    preview: Preview,
    desktop: RefCell<Option<Desktop>>,
    busy: Cell<bool>,
    suspended_windows: RefCell<Vec<gtk::Window>>,
    status: gtk::Label,
    system_dark: Cell<bool>,
}

pub fn run() {
    if let Err(error) = gtk::init() {
        eprintln!("A Linux graphical session is required: {error}");
        return;
    }
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("DISPLAY").is_none()
    {
        ui::error(
            &gtk::Window::new(gtk::WindowType::Toplevel),
            "The native preview currently requires X11. Wayland capture/placement remains a parity blocker; use the shipping Tauri app on Wayland.",
        );
        return;
    }
    let application = gtk::Application::new(
        Some("es.captur.LinuxNativePreview"),
        gio::ApplicationFlags::HANDLES_COMMAND_LINE,
    );
    let state = Rc::new(RefCell::new(None::<Rc<App>>));
    application.connect_command_line(move |application, command| {
        if state.borrow().is_none() {
            *state.borrow_mut() = Some(App::new(application));
        }
        let app = state.borrow().as_ref().unwrap().clone();
        let args: Vec<String> = command
            .arguments()
            .iter()
            .skip(1)
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        match args.first().map(String::as_str) {
            Some("--background") => app.window.hide(),
            Some("--preferences") => app.action(Action::Preferences),
            Some("--history") => app.action(Action::History),
            Some("--capture") => app.action(Action::Capture(0, 0)),
            Some("--record") => app.action(Action::Capture(1, 0)),
            Some("--gif") => app.action(Action::Capture(2, 0)),
            Some("--canvas") => app.action(Action::Canvas),
            Some("--previews") => {
                app.window.hide();
                for path in args.iter().skip(1) {
                    if let Ok(image) = image::open(path) {
                        app.preview.add(image.to_rgba8(), path.into());
                    }
                }
            }
            Some("--open") => {
                for path in args.iter().skip(1) {
                    app.open(path.into());
                }
            }
            Some(path) if !path.starts_with('-') => app.open(path.into()),
            _ => app.action(Action::Preferences),
        }
        0
    });
    application.run();
}

impl App {
    fn new(application: &gtk::Application) -> Rc<Self> {
        let settings = Settings::load().unwrap_or_else(|e| {
            eprintln!("Native settings: {e}");
            Settings::default()
        });
        let system_dark = gtk::Settings::default().is_some_and(|s| {
            s.is_gtk_application_prefer_dark_theme()
                || s.gtk_theme_name()
                    .is_some_and(|n| n.to_ascii_lowercase().contains("dark"))
        });
        apply_theme(&settings, system_dark);
        let settings = Rc::new(RefCell::new(settings));
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        application.add_window(&window);
        window.set_title("Captures — Linux native");
        // A hidden application owner, not an invented launcher screen. New
        // capture opens the same selector used by the tray and global shortcut.
        let status = ui::label("Ready · Choose a capture target", "muted");
        let weak_slot = Rc::new(RefCell::new(std::rc::Weak::<App>::new()));
        let open: Rc<dyn Fn(PathBuf)> = Rc::new({
            let weak = weak_slot.clone();
            move |path| {
                if let Some(app) = weak.borrow().upgrade() {
                    app.open(path);
                }
            }
        });
        let preview = Preview::with_callbacks(PreviewCallbacks {
            open: Rc::new(|path| {
                let _ = gio::AppInfo::launch_default_for_uri(
                    &gio::File::for_path(path).uri(),
                    gio::AppLaunchContext::NONE,
                );
            }),
            edit: open,
            dismiss: Rc::new(|_| {}),
            delete: Rc::new({
                let weak = weak_slot.clone();
                move |path| {
                    if let Some(app) = weak.borrow().upgrade() {
                        let result = std::fs::remove_file(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|_| history::forget(&path));
                        if let Err(error) = result {
                            ui::error(&app.window, &error);
                        }
                    }
                }
            }),
            save: Rc::new(|path| {
                if let Some(parent) = path.parent() {
                    let _ = gio::AppInfo::launch_default_for_uri(
                        &gio::File::for_path(parent).uri(),
                        gio::AppLaunchContext::NONE,
                    );
                }
            }),
            placement: Rc::new({
                let weak = weak_slot.clone();
                move |placement| {
                    if let Some(app) = weak.borrow().upgrade() {
                        let mut config = app.settings.borrow().clone();
                        config.mini_preview_placement = placement as u32;
                        if let Err(error) = config.save() {
                            ui::error(&app.window, &error);
                        } else {
                            *app.settings.borrow_mut() = config;
                        }
                    }
                }
            }),
        });
        let app = Rc::new(Self {
            application: application.clone(),
            _hold: application.hold(),
            window: window.clone(),
            settings: settings.clone(),
            preview,
            desktop: RefCell::new(None),
            busy: Cell::new(false),
            suspended_windows: RefCell::new(Vec::new()),
            status: status.clone(),
            system_dark: Cell::new(system_dark),
        });
        *weak_slot.borrow_mut() = Rc::downgrade(&app);
        if let Some(gtk_settings) = gtk::Settings::default() {
            let weak = Rc::downgrade(&app);
            gtk_settings.connect_gtk_theme_name_notify(move |system| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                let dark = system
                    .gtk_theme_name()
                    .is_some_and(|name| name.to_ascii_lowercase().contains("dark"));
                app.system_dark.set(dark);
                let config = app.settings.borrow();
                if config.appearance == "system" {
                    apply_theme(&config, dark);
                }
            });
        }
        window.connect_delete_event(|w, _| {
            w.hide();
            glib::Propagation::Stop
        });
        {
            let app = app.clone();
            window.connect_key_press_event(move |window, event| {
                if event.keyval() == gdk::keys::constants::Escape {
                    window.hide();
                    return glib::Propagation::Stop;
                }
                if event.state().contains(gdk::ModifierType::CONTROL_MASK)
                    && event.keyval() == gdk::keys::constants::n
                {
                    app.action(Action::Capture(0, 0));
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        match Desktop::new() {
            Ok(mut desktop) => {
                if let Err(error) = desktop.replace_shortcuts(&settings.borrow()) {
                    app.status.set_text(&error);
                }
                *app.desktop.borrow_mut() = Some(desktop);
            }
            Err(error) => app
                .status
                .set_text(&format!("Desktop integration unavailable: {error}")),
        }
        {
            let weak = Rc::downgrade(&app);
            glib::timeout_add_local(Duration::from_millis(30), move || {
                let Some(app) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let actions = app
                    .desktop
                    .borrow()
                    .as_ref()
                    .map(|d| d.events())
                    .unwrap_or_default();
                for action in actions {
                    app.action(action);
                }
                glib::ControlFlow::Continue
            });
        }
        app.preview.home(
            settings.borrow().mini_preview_placement.is_multiple_of(2),
            settings.borrow().mini_preview_placement >= 2,
        );
        {
            let app = app.clone();
            glib::idle_add_local_once(move || app.recover());
        }
        app
    }

    fn action(self: &Rc<Self>, action: Action) {
        match action {
            Action::Capture(kind, mode) => self.capture(kind, mode),
            Action::Open => {
                if let Some(path) = ui::open_file(&self.window) {
                    self.open(path)
                }
            }
            Action::Canvas => new_canvas(
                &self.window,
                self.settings.borrow().output_directory.clone(),
                self.saved(),
            ),
            Action::History => {
                self.window.hide();
                let app = self.clone();
                let restore = self.clone();
                history::open_with_restore(
                    self.settings.borrow().output_directory.clone(),
                    Rc::new(move |path| app.open(path)),
                    Rc::new(move |path| restore.restore_preview(path)),
                );
                self.recover();
            }
            Action::Preferences => {
                self.window.hide();
                if let Some(window) = gtk::Window::list_toplevels()
                    .into_iter()
                    .filter_map(|w| w.downcast::<gtk::Window>().ok())
                    .find(|w| w.title().as_deref() == Some("Captures Preferences"))
                {
                    window.present();
                    return;
                }
                let app = self.clone();
                preferences::open(
                    self.settings.clone(),
                    Rc::new(move |settings| app.apply(settings)),
                );
            }
            Action::Quit => {
                self.desktop.borrow_mut().take();
                self.application.quit();
            }
        }
    }

    fn apply(&self, settings: Settings) -> Result<(), String> {
        settings.validate()?;
        desktop::shortcuts(&settings)?;
        let previous = self.settings.borrow().clone();
        if let Some(desktop) = self.desktop.borrow_mut().as_mut() {
            desktop.replace_shortcuts(&settings)?;
        }
        let result = (|| {
            if settings.launch_at_login != previous.launch_at_login {
                desktop::set_autostart(settings.launch_at_login)?;
            }
            settings.save()
        })();
        if let Err(error) = result {
            if let Some(desktop) = self.desktop.borrow_mut().as_mut() {
                let _ = desktop.replace_shortcuts(&previous);
            }
            if settings.launch_at_login != previous.launch_at_login {
                let _ = desktop::set_autostart(previous.launch_at_login);
            }
            return Err(error);
        }
        apply_theme(&settings, self.system_dark.get());
        self.preview.home(
            settings.mini_preview_placement.is_multiple_of(2),
            settings.mini_preview_placement >= 2,
        );
        if settings.show_mini_previews
            && (!self.busy.get() || settings.include_mini_previews_in_captures)
        {
            self.preview.show();
        } else {
            self.preview.hide();
        }
        *self.settings.borrow_mut() = settings;
        Ok(())
    }

    fn finish_capture(&self) {
        self.busy.set(false);
        for window in self.suspended_windows.take() {
            window.show();
        }
        self.sync_previews();
    }

    fn sync_previews(&self) {
        let settings = self.settings.borrow();
        if settings.show_mini_previews
            && (!self.busy.get() || settings.include_mini_previews_in_captures)
        {
            self.preview.show();
        } else {
            self.preview.hide();
        }
    }

    fn restore_preview(self: &Rc<Self>, path: PathBuf) {
        let app = self.clone();
        ui::job(
            move || {
                let image = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
                Ok((image, path))
            },
            move |result| match result {
                Ok((image, path)) => {
                    app.preview.add(image, path);
                    app.sync_previews();
                }
                Err(error) => ui::error(&app.window, &error),
            },
        );
    }

    fn saved(self: &Rc<Self>) -> Rc<dyn Fn(PathBuf)> {
        let app = self.clone();
        Rc::new(move |path| {
            if let Err(error) = history::record(&path) {
                app.status
                    .set_text(&format!("Saved, but history failed: {error}"));
            } else {
                app.status.set_text(&format!(
                    "Saved {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            if app.settings.borrow().show_mini_previews
                && path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    ["mp4", "webm", "gif"].contains(&e.to_ascii_lowercase().as_str())
                })
            {
                let app = app.clone();
                ui::job(
                    move || {
                        let metadata = captures_media::MediaToolchain::from_command_names()
                            .probe(&path)
                            .map_err(|e| e.to_string())?
                            .metadata;
                        let output = std::process::Command::new("ffmpeg")
                            .args(["-v", "error", "-i"])
                            .arg(&path)
                            .args([
                                "-frames:v",
                                "1",
                                "-vf",
                                "scale=568:320:force_original_aspect_ratio=decrease",
                                "-f",
                                "image2pipe",
                                "-vcodec",
                                "png",
                                "-",
                            ])
                            .output()
                            .map_err(|e| e.to_string())?;
                        let poster = image::load_from_memory(&output.stdout)
                            .map_err(|e| e.to_string())?
                            .to_rgba8();
                        Ok((poster, path, metadata))
                    },
                    move |result| match result {
                        Ok((poster, path, metadata)) => {
                            if app.settings.borrow().show_mini_previews {
                                app.preview.add_with_metadata(
                                    poster,
                                    path,
                                    PreviewMetadata {
                                        width: metadata.width,
                                        height: metadata.height,
                                        size_bytes: metadata.size_bytes,
                                        saved: true,
                                        copied: false,
                                    },
                                );
                            }
                        }
                        Err(error) => app
                            .status
                            .set_text(&format!("Saved; preview unavailable: {error}")),
                    },
                );
            }
        })
    }

    fn recover(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let app = self.clone();
        crate::recovery::open(
            self.settings.borrow().output_directory.clone(),
            Rc::new(move |path| {
                app.saved()(path.clone());
                app.open(path);
            }),
        );
    }

    fn open(self: &Rc<Self>, path: PathBuf) {
        self.window.hide();
        let directory = self.settings.borrow().output_directory.clone();
        let saved = self.saved();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["mp4", "webm", "gif"].contains(&e.to_ascii_lowercase().as_str()))
        {
            recording::open_editor(path, directory, saved);
        } else {
            editor::open_file(path, directory, saved);
        }
    }

    fn capture(self: &Rc<Self>, kind: u32, mode: u32) {
        if self.busy.replace(true) {
            return;
        }
        let config = self.settings.borrow().clone();
        let visible: Vec<gtk::Window> = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast().ok())
            .filter(|w: &gtk::Window| w.is_visible())
            .collect();
        for window in &visible {
            window.hide();
        }
        // Regular editing/settings windows resume after completion, but the
        // launcher stays dismissed and previews follow their own preference.
        *self.suspended_windows.borrow_mut() = visible
            .iter()
            .filter(|w| {
                **w != self.window && w.title().as_deref() != Some("Captures — Mini previews")
            })
            .cloned()
            .collect();
        self.sync_previews();
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(180), move || {
            let cancelled: Rc<dyn Fn()> = Rc::new({
                let app = app.clone();
                move || {
                    app.finish_capture();
                    for w in &visible {
                        if *w == app.window {
                            w.show();
                        }
                    }
                }
            });
            let done = app.clone();
            let cancel_countdown = cancelled.clone();
            capture::select_with_options(
                match mode {
                    1 => CaptureMode::Window,
                    2 => CaptureMode::Display,
                    _ => CaptureMode::Region,
                },
                kind,
                config.clone(),
                false,
                Rc::new(move |selection| {
                    let config = selection.settings.clone();
                    let app = done.clone();
                    let kind = selection.kind;
                    let seconds = if kind == 0 {
                        config.screenshot_countdown_seconds
                    } else {
                        config.recording.countdown_seconds
                    };
                    let selection = Rc::new(selection);
                    countdown(
                        seconds,
                        Rc::new(move || {
                            if kind == 0 {
                                app.save_screenshot(
                                    selection.clone(),
                                    config.clone(),
                                    seconds > 0,
                                    None,
                                );
                            } else {
                                let r = &config.recording;
                                let options = RecordingOptions {
                                    kind: if kind == 2 {
                                        RecordingKind::Gif
                                    } else {
                                        RecordingKind::Video
                                    },
                                    target: selection.target.clone(),
                                    frames_per_second: if kind == 2 {
                                        r.gif_fps
                                    } else {
                                        r.video_fps
                                    },
                                    max_resolution: r.video_max_resolution,
                                    countdown_seconds: 0,
                                    show_cursor: r.show_cursor,
                                    highlight_clicks: r.highlight_clicks,
                                    show_keystrokes: r.show_keystrokes,
                                    audio: if kind == 2 {
                                        AudioOptions::default()
                                    } else {
                                        AudioOptions {
                                            capture_system_audio: r.capture_system_audio,
                                            microphone_device_id: r.microphone_device_id.clone(),
                                            mono_output: r.mono_audio,
                                            ..AudioOptions::default()
                                        }
                                    },
                                    gif: GifOptions {
                                        max_width: r.gif_max_width,
                                        max_colors: r.gif_max_colors,
                                        optimize: true,
                                    },
                                };
                                let app = app.clone();
                                let dir = config.output_directory.clone();
                                let open = r.open_editor_after_recording;
                                let finished = app.clone();
                                let screenshot = app.clone();
                                recording::start_with_actions(
                                    options,
                                    selection.display.clone(),
                                    dir,
                                    Rc::new(move |path| {
                                        app.saved()(path.clone());
                                        if open {
                                            app.open(path);
                                        } else {
                                            app.status.set_text("Recording saved");
                                        }
                                    }),
                                    recording::RecordingActions {
                                        screenshot: Some(Rc::new(move |done| {
                                            screenshot.recording_screenshot(done)
                                        })),
                                        on_finished: Some(Rc::new(move || {
                                            finished.finish_capture();
                                        })),
                                        preferred_video_format: match r.video_format.as_str() {
                                            "webm" => captures_media::ExportFormat::WebM,
                                            "gif" => captures_media::ExportFormat::Gif,
                                            _ => captures_media::ExportFormat::Mp4,
                                        },
                                    },
                                );
                            }
                        }),
                        cancel_countdown.clone(),
                    );
                }),
                cancelled,
            );
        });
    }

    fn recording_screenshot(self: &Rc<Self>, finished: Rc<dyn Fn()>) {
        let config = self.settings.borrow().clone();
        self.preview.hide();
        let finished: Rc<dyn Fn()> = Rc::new({
            let app = self.clone();
            move || {
                finished();
                app.sync_previews();
            }
        });
        let app = self.clone();
        let cancelled = finished.clone();
        capture::select_with_options(
            CaptureMode::Region,
            0,
            config,
            true,
            Rc::new(move |selection| {
                let seconds = selection.settings.screenshot_countdown_seconds;
                let config = selection.settings.clone();
                let selection = Rc::new(selection);
                let (app, finished, cancelled) = (app.clone(), finished.clone(), finished.clone());
                countdown(
                    seconds,
                    Rc::new(move || {
                        app.save_screenshot(
                            selection.clone(),
                            config.clone(),
                            seconds > 0,
                            Some(finished.clone()),
                        )
                    }),
                    cancelled,
                );
            }),
            cancelled,
        );
    }

    fn save_screenshot(
        self: &Rc<Self>,
        selection: Rc<capture::Selection>,
        config: Settings,
        refresh: bool,
        finished: Option<Rc<dyn Fn()>>,
    ) {
        let mut image = selection.image.clone();
        let mut selection = (*selection).clone();
        if refresh {
            selection.pointer = capture::pointer();
        }
        let app = self.clone();
        ui::job(
            move || {
                if !captures_session::capture_session_available() {
                    return Err("Desktop session unavailable".into());
                }
                if refresh {
                    image = capture::refresh(&selection, config.show_cursor_in_screenshots)?;
                }
                let format = match config.screenshot_format.as_str() {
                    "jpeg" => image::ImageFormat::Jpeg,
                    "webp" => image::ImageFormat::WebP,
                    _ => image::ImageFormat::Png,
                };
                let mut bytes = std::io::Cursor::new(Vec::new());
                if format == image::ImageFormat::Jpeg {
                    image::DynamicImage::ImageRgba8(image.clone())
                        .to_rgb8()
                        .write_to(&mut bytes, format)
                } else {
                    image.write_to(&mut bytes, format)
                }
                .map_err(|e| e.to_string())?;
                let path = ui::save_bytes(
                    &config.output_directory,
                    &config.screenshot_format,
                    bytes.get_ref(),
                )?;
                Ok((image, path, config))
            },
            move |result| {
                if finished.is_none() {
                    app.finish_capture();
                }
                match result {
                    Ok((image, path, config)) => {
                        app.saved()(path.clone());
                        if config.auto_copy_to_clipboard {
                            let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
                            clipboard.set_image(&ui::pixbuf(&image));
                            clipboard.store();
                        }
                        if config.show_mini_previews {
                            app.preview.add_with_metadata(
                                image.clone(),
                                path.clone(),
                                PreviewMetadata {
                                    width: image.width(),
                                    height: image.height(),
                                    size_bytes: std::fs::metadata(path)
                                        .map(|m| m.len())
                                        .unwrap_or(0),
                                    saved: true,
                                    copied: config.auto_copy_to_clipboard,
                                },
                            );
                        }
                    }
                    Err(error) => {
                        app.window.show_all();
                        ui::error(&app.window, &error);
                    }
                }
                if let Some(finished) = finished {
                    finished();
                }
            },
        );
    }
}

fn apply_theme(settings: &Settings, system_dark: bool) {
    ui::install_theme_custom(
        match settings.appearance.as_str() {
            "dark" => true,
            "light" => false,
            _ => system_dark,
        },
        &settings.theme,
        &settings.custom_accent,
        &settings.custom_signal,
    );
}

fn countdown(seconds: u8, done: Rc<dyn Fn()>, cancelled: Rc<dyn Fn()>) {
    if seconds == 0 {
        done();
        return;
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures countdown");
    window.set_decorated(false);
    window.set_keep_above(true);
    window.set_position(gtk::WindowPosition::Center);
    let remaining = Rc::new(Cell::new(seconds));
    let closed = Rc::new(Cell::new(false));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.set_border_width(16);
    row.style_context().add_class("glass");
    let label = ui::label(&seconds.to_string(), "title");
    let cancel = ui::button("Cancel · Esc");
    row.add(&label);
    row.add(&cancel);
    window.add(&row);
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    window.connect_key_press_event(|w, e| {
        if e.keyval() == gdk::keys::constants::Escape {
            w.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    {
        let closed = closed.clone();
        window.connect_delete_event(move |_, _| {
            if !closed.replace(true) {
                cancelled();
            }
            glib::Propagation::Proceed
        });
    }
    window.show_all();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        if closed.get() {
            return glib::ControlFlow::Break;
        }
        let n = remaining.get() - 1;
        remaining.set(n);
        label.set_text(&n.to_string());
        if n == 0 {
            closed.set(true);
            window.close();
            let done = done.clone();
            glib::timeout_add_local_once(Duration::from_millis(180), move || done());
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn new_canvas(parent: &gtk::Window, directory: PathBuf, saved: Rc<dyn Fn(PathBuf)>) {
    let dialog = gtk::Dialog::with_buttons(
        Some("New canvas"),
        Some(parent),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Create", gtk::ResponseType::Accept),
        ],
    );
    let grid = gtk::Grid::new();
    grid.set_border_width(24);
    grid.set_row_spacing(12);
    grid.set_column_spacing(16);
    let width = gtk::SpinButton::with_range(1., 8192., 1.);
    width.set_value(960.);
    let height = gtk::SpinButton::with_range(1., 8192., 1.);
    height.set_value(540.);
    let transparent = gtk::CheckButton::with_label("Transparent");
    transparent.set_active(true);
    let color = gtk::ColorButton::new();
    color.set_rgba(&gdk::RGBA::WHITE);
    let presets = gtk::ComboBoxText::new();
    for name in [
        "Custom",
        "Square · 1080 × 1080",
        "Landscape · 1920 × 1080",
        "Portrait · 1080 × 1920",
        "Icon · 512 × 512",
    ] {
        presets.append_text(name);
    }
    presets.set_active(Some(0));
    {
        let width = width.clone();
        let height = height.clone();
        presets.connect_changed(move |p| {
            if let Some((w, h)) = match p.active() {
                Some(1) => Some((1080., 1080.)),
                Some(2) => Some((1920., 1080.)),
                Some(3) => Some((1080., 1920.)),
                Some(4) => Some((512., 512.)),
                _ => None,
            } {
                width.set_value(w);
                height.set_value(h);
            }
        });
    }
    for (row, (name, widget)) in [
        ("Preset", presets.upcast_ref::<gtk::Widget>()),
        ("Width", width.upcast_ref()),
        ("Height", height.upcast_ref()),
        ("Background", color.upcast_ref()),
        ("", transparent.upcast_ref()),
    ]
    .into_iter()
    .enumerate()
    {
        grid.attach(&ui::label(name, ""), 0, row as i32, 1, 1);
        grid.attach(widget, 1, row as i32, 1, 1);
    }
    dialog.content_area().add(&grid);
    dialog.show_all();
    if dialog.run() == gtk::ResponseType::Accept {
        parent.hide();
        let c = color.rgba();
        let image = image::RgbaImage::from_pixel(
            width.value_as_int() as u32,
            height.value_as_int() as u32,
            image::Rgba([
                (c.red() * 255.) as u8,
                (c.green() * 255.) as u8,
                (c.blue() * 255.) as u8,
                if transparent.is_active() { 0 } else { 255 },
            ]),
        );
        editor::open(image, directory, saved);
    }
    dialog.close();
}
