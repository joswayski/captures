use crate::{capture, editor, preview::Preview, recording, ui};
use captures_capture::CaptureMode;
use captures_recording::{
    AudioOptions, GifOptions, MaxResolution, RecordingKind, RecordingOptions,
};
use gtk::{gdk, glib, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    appearance: String,
    output: PathBuf,
    fps: u16,
    countdown: u8,
    cursor: bool,
    corner: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: "system".into(),
            output: data_dir().join("captures"),
            fps: 30,
            countdown: 0,
            cursor: true,
            corner: 0,
        }
    }
}
fn data_dir() -> PathBuf {
    std::env::var_os("CAPTURES_NATIVE_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
                })
                .join("captures-linux-native")
        })
}

pub fn run() {
    if let Err(error) = gtk::init() {
        eprintln!("A Linux graphical session is required: {error}");
        return;
    }
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("DISPLAY").is_none()
    {
        let w = gtk::Window::new(gtk::WindowType::Toplevel);
        ui::error(
            &w,
            "This native experiment currently requires an X11 session. Wayland targeting and overlay placement have not been implemented. The shipping Tauri app remains available.",
        );
        return;
    }
    let settings_path = data_dir().join("settings.json");
    let settings: Settings = match std::fs::read(&settings_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            eprintln!("Cannot read native settings; using defaults: {e}");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    let system_dark =
        gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme());
    ui::install_theme(match settings.appearance.as_str() {
        "dark" => true,
        "light" => false,
        _ => system_dark,
    });
    let settings = Rc::new(RefCell::new(settings));
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures — Linux native");
    window.set_default_size(590, 440);
    window.set_position(gtk::WindowPosition::Center);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
    root.set_border_width(28);
    root.style_context().add_class("glass");
    root.pack_start(&ui::label("New capture", "title"), false, false, 0);
    root.pack_start(
        &ui::label(
            "Linux native · Experimental · Separate capture library",
            "muted",
        ),
        false,
        false,
        0,
    );
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let kind = gtk::ComboBoxText::new();
    for text in ["Screenshot", "Video", "GIF"] {
        kind.append_text(text);
    }
    kind.set_active(Some(0));
    let target = gtk::ComboBoxText::new();
    for text in ["Region", "Window", "Full display"] {
        target.append_text(text);
    }
    target.set_active(Some(0));
    row.pack_start(&kind, true, true, 0);
    row.pack_start(&target, true, true, 0);
    root.pack_start(&row, false, false, 0);
    let audio = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let system = gtk::CheckButton::with_label("Desktop audio");
    let mic = gtk::CheckButton::with_label("Default microphone");
    audio.pack_start(&system, false, false, 0);
    audio.pack_start(&mic, false, false, 0);
    system.set_sensitive(false);
    mic.set_sensitive(false);
    {
        let (system, mic) = (system.clone(), mic.clone());
        kind.connect_changed(move |kind| {
            let video = kind.active() == Some(1);
            system.set_sensitive(video);
            mic.set_sensitive(video);
        });
    }
    root.pack_start(&audio, false, false, 0);
    let capture_button = ui::button("Choose target");
    capture_button.style_context().add_class("primary");
    root.pack_start(&capture_button, false, false, 0);
    let nav = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let history = ui::button("History");
    let prefs = ui::button("Preferences");
    let open = ui::button("Open file…");
    let blank = ui::button("New canvas…");
    for b in [&history, &prefs, &open, &blank] {
        nav.pack_start(b, true, true, 0);
    }
    root.pack_start(&nav, false, false, 0);
    let status = ui::label(
        "Ready · Ctrl+N to capture · Esc to cancel selection",
        "muted",
    );
    status.set_line_wrap(true);
    root.pack_start(&status, true, true, 0);
    let saved: Rc<dyn Fn(PathBuf)> = Rc::new({
        let status = status.clone();
        let window = window.clone();
        move |path| {
            status.set_text(&format!("Saved {}", path.display()));
            eprintln!("Saved {}", path.display());
            window.show_all();
        }
    });
    let preview = Preview::new(Rc::new({
        let settings = settings.clone();
        let saved = saved.clone();
        move |path| open_path(path, settings.borrow().output.clone(), saved.clone())
    }));
    {
        let (window, settings, preview, saved) = (
            window.clone(),
            settings.clone(),
            preview.clone(),
            saved.clone(),
        );
        capture_button.connect_clicked(move |_| {
            let mode = match target.active() {
                Some(1) => CaptureMode::Window,
                Some(2) => CaptureMode::Display,
                _ => CaptureMode::Region,
            };
            let kind = kind.active().unwrap_or(0);
            let config = settings.borrow().clone();
            let audio = AudioOptions {
                capture_system_audio: kind == 1 && system.is_active(),
                microphone_device_id: (kind == 1 && mic.is_active()).then(|| "default".into()),
                ..AudioOptions::default()
            };
            let visible: Vec<gtk::Window> = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .filter(|w| w.is_visible())
                .collect();
            for w in &visible {
                w.hide();
            }
            preview.hide();
            let window = window.clone();
            let preview = preview.clone();
            let saved = saved.clone();
            // Let X11 composite the hidden windows before acquiring the frozen frame.
            glib::timeout_add_local_once(Duration::from_millis(180), move || {
                let cancel_windows = visible.clone();
                let cancelled_preview = preview.clone();
                capture::select(
                    mode,
                    Rc::new(move |selection| {
                        let config = config.clone();
                        let saved = saved.clone();
                        let preview = preview.clone();
                        let window = window.clone();
                        let audio = audio.clone();
                        let image = selection.image.clone();
                        let display = selection.display.clone();
                        let target = selection.target.clone();
                        let cancel_window = window.clone();
                        countdown(
                            config.countdown,
                            Rc::new(move || {
                                if kind == 0 {
                                    let image = image.clone();
                                    let output = config.output.clone();
                                    let preview = preview.clone();
                                    let saved = saved.clone();
                                    let window = window.clone();
                                    let corner = config.corner;
                                    ui::job(
                                        move || {
                                            if !captures_session::capture_session_available() {
                                                return Err("Desktop session unavailable".into());
                                            }
                                            let path = ui::save_png(&image, &output)?;
                                            Ok((image, path))
                                        },
                                        move |result| match result {
                                            Ok((image, path)) => {
                                                saved(path.clone());
                                                preview.add(image, path);
                                                preview.home(corner % 2 == 1, corner >= 2);
                                            }
                                            Err(error) => {
                                                window.show_all();
                                                ui::error(&window, &error);
                                            }
                                        },
                                    );
                                } else {
                                    let options = RecordingOptions {
                                        kind: if kind == 2 {
                                            RecordingKind::Gif
                                        } else {
                                            RecordingKind::Video
                                        },
                                        target: target.clone(),
                                        frames_per_second: config.fps.min(if kind == 2 {
                                            30
                                        } else {
                                            60
                                        }),
                                        max_resolution: MaxResolution::Original,
                                        countdown_seconds: 0,
                                        show_cursor: config.cursor,
                                        highlight_clicks: false,
                                        show_keystrokes: false,
                                        audio: audio.clone(),
                                        gif: GifOptions::default(),
                                    };
                                    let dir = config.output.clone();
                                    let on_saved = saved.clone();
                                    recording::start(
                                        options,
                                        display.clone(),
                                        dir.clone(),
                                        Rc::new(move |path| {
                                            on_saved(path.clone());
                                            recording::open_editor(
                                                path,
                                                dir.clone(),
                                                on_saved.clone(),
                                            );
                                        }),
                                    );
                                }
                            }),
                            Rc::new(move || cancel_window.show_all()),
                        );
                    }),
                    Rc::new(move || {
                        for w in &cancel_windows {
                            w.show_all();
                        }
                        cancelled_preview.show();
                    }),
                );
            });
        });
    }
    {
        let (window, settings, saved) = (window.clone(), settings.clone(), saved.clone());
        open.connect_clicked(move |_| {
            if let Some(path) = ui::open_file(&window) {
                open_path(path, settings.borrow().output.clone(), saved.clone());
            }
        });
    }
    {
        let (window, settings, saved) = (window.clone(), settings.clone(), saved.clone());
        blank.connect_clicked(move |_| {
            new_canvas(&window, settings.borrow().output.clone(), saved.clone())
        });
    }
    {
        let (settings, saved) = (settings.clone(), saved.clone());
        history.connect_clicked(move |_| {
            open_history(settings.borrow().output.clone(), saved.clone())
        });
    }
    {
        let (settings, preview) = (settings.clone(), preview.clone());
        prefs.connect_clicked(move |_| {
            preferences(
                settings.clone(),
                settings_path.clone(),
                preview.clone(),
                system_dark,
            )
        });
    }
    window.connect_key_press_event(move |_, event| {
        if event.state().contains(gdk::ModifierType::CONTROL_MASK)
            && event.keyval() == gdk::keys::constants::n
        {
            capture_button.clicked();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    // GTK destroys all native windows when the process exits; recorder Drop joins its workers.
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        glib::Propagation::Proceed
    });
    let ready = Cell::new(false);
    window.connect_draw(move |_, _| {
        if !ready.replace(true) {
            glib::idle_add_local_once(|| {
                let _ = captures_ui_probe::ready();
            });
        }
        glib::Propagation::Proceed
    });
    window.add(&root);
    window.show_all();
    // Real file opening also supplies deterministic content for comparison runs.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--open") => {
            if let Some(path) = args.get(1) {
                open_path(path.into(), settings.borrow().output.clone(), saved.clone());
            }
        }
        Some("--previews") => {
            for path in args.iter().skip(1) {
                if let Ok(image) = image::open(path) {
                    preview.add(image.to_rgba8(), path.into());
                }
            }
        }
        Some("--preferences") => preferences(
            settings.clone(),
            data_dir().join("settings.json"),
            preview.clone(),
            system_dark,
        ),
        Some("--history") => open_history(settings.borrow().output.clone(), saved.clone()),
        _ => {}
    }
    gtk::main();
}

fn open_path(path: PathBuf, directory: PathBuf, saved: Rc<dyn Fn(PathBuf)>) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["mp4", "webm", "gif"].contains(&ext.as_str()) {
        recording::open_editor(path, directory, saved);
    } else {
        ui::job(
            move || {
                image::open(path)
                    .map(|i| i.to_rgba8())
                    .map_err(|e| e.to_string())
            },
            move |result| match result {
                Ok(image) => editor::open(image, directory.clone(), saved.clone()),
                Err(error) => ui::error(&gtk::Window::new(gtk::WindowType::Toplevel), &error),
            },
        );
    }
}

fn countdown(seconds: u8, done: Rc<dyn Fn()>, cancelled: Rc<dyn Fn()>) {
    if seconds == 0 {
        done();
        return;
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures countdown");
    window.set_keep_above(true);
    let remaining = Rc::new(Cell::new(seconds));
    let closed = Rc::new(Cell::new(false));
    let label = ui::label(&seconds.to_string(), "title");
    label.set_margin_top(24);
    label.set_margin_bottom(24);
    label.set_margin_start(48);
    label.set_margin_end(48);
    window.add(&label);
    window.show_all();
    {
        let closed = closed.clone();
        window.connect_delete_event(move |_, _| {
            if !closed.replace(true) {
                cancelled();
            }
            glib::Propagation::Proceed
        });
    }
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
            done();
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
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_border_width(16);
    let width = gtk::SpinButton::with_range(1., 8192., 1.);
    width.set_value(960.);
    let height = gtk::SpinButton::with_range(1., 8192., 1.);
    height.set_value(540.);
    let transparent = gtk::CheckButton::with_label("Transparent");
    row.pack_start(&ui::label("Width", ""), false, false, 0);
    row.pack_start(&width, false, false, 0);
    row.pack_start(&ui::label("Height", ""), false, false, 0);
    row.pack_start(&height, false, false, 0);
    row.pack_start(&transparent, false, false, 0);
    dialog.content_area().add(&row);
    dialog.show_all();
    if dialog.run() == gtk::ResponseType::Accept {
        let image = image::RgbaImage::from_pixel(
            width.value_as_int() as u32,
            height.value_as_int() as u32,
            image::Rgba([255, 255, 255, if transparent.is_active() { 0 } else { 255 }]),
        );
        editor::open(image, directory, saved);
    }
    dialog.close();
}

fn preferences(
    settings: Rc<RefCell<Settings>>,
    path: PathBuf,
    preview: Preview,
    system_dark: bool,
) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures — Preferences");
    window.set_default_size(630, 480);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
    root.set_border_width(28);
    root.pack_start(&ui::label("Preferences", "title"), false, false, 0);
    root.pack_start(
        &ui::label(
            "Native experiment settings · Does not modify Tauri preferences",
            "muted",
        ),
        false,
        false,
        0,
    );
    let grid = gtk::Grid::new();
    grid.set_column_spacing(24);
    grid.set_row_spacing(16);
    let appearance = gtk::ComboBoxText::new();
    for name in ["system", "light", "dark"] {
        appearance.append(Some(name), name);
    }
    appearance.set_active_id(Some(&settings.borrow().appearance));
    let output = gtk::Entry::new();
    output.set_text(&settings.borrow().output.to_string_lossy());
    output.set_hexpand(true);
    let fps = gtk::ComboBoxText::new();
    for n in ["15", "30", "60"] {
        fps.append(Some(n), n);
    }
    fps.set_active_id(Some(&settings.borrow().fps.to_string()));
    let countdown = gtk::SpinButton::with_range(0., 10., 1.);
    countdown.set_value(settings.borrow().countdown as f64);
    let cursor = gtk::CheckButton::with_label("Show pointer in recordings");
    cursor.set_active(settings.borrow().cursor);
    let corner = gtk::ComboBoxText::new();
    for name in ["Bottom left", "Bottom right", "Top left", "Top right"] {
        corner.append_text(name);
    }
    corner.set_active(Some(settings.borrow().corner));
    for (i, (name, widget)) in [
        ("Appearance", appearance.upcast_ref::<gtk::Widget>()),
        ("Save directory", output.upcast_ref()),
        ("Recording FPS", fps.upcast_ref()),
        ("Countdown (seconds)", countdown.upcast_ref()),
        ("Mini previews", corner.upcast_ref()),
        ("Pointer", cursor.upcast_ref()),
    ]
    .into_iter()
    .enumerate()
    {
        grid.attach(&ui::label(name, ""), 0, i as i32, 1, 1);
        grid.attach(widget, 1, i as i32, 1, 1);
    }
    root.pack_start(&grid, true, true, 0);
    let save = ui::button("Apply preferences");
    save.style_context().add_class("primary");
    root.pack_start(&save, false, false, 0);
    let w = window.clone();
    save.connect_clicked(move |_| {
        let config = Settings {
            appearance: appearance.active_id().unwrap().to_string(),
            output: PathBuf::from(output.text().as_str()),
            fps: fps.active_id().unwrap().parse().unwrap(),
            countdown: countdown.value_as_int() as u8,
            cursor: cursor.is_active(),
            corner: corner.active().unwrap_or(0),
        };
        if !config.output.is_absolute() {
            ui::error(&w, "Choose an absolute save directory.");
            return;
        }
        let result = (|| -> Result<(), String> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let temp = path.with_extension("json.tmp");
            std::fs::write(
                &temp,
                serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            std::fs::rename(temp, &path).map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            ui::error(&w, &error);
            return;
        }
        ui::install_theme(match config.appearance.as_str() {
            "dark" => true,
            "light" => false,
            _ => system_dark,
        });
        preview.home(config.corner % 2 == 1, config.corner >= 2);
        *settings.borrow_mut() = config;
    });
    window.add(&root);
    window.show_all();
}

fn open_history(directory: PathBuf, saved: Rc<dyn Fn(PathBuf)>) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures — History");
    window.set_default_size(800, 600);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
    root.set_border_width(24);
    root.pack_start(&ui::label("Capture history", "title"), false, false, 0);
    root.pack_start(
        &ui::label(
            "Native library · Dismissing a mini preview keeps its saved file",
            "muted",
        ),
        false,
        false,
        0,
    );
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    let list = gtk::ListBox::new();
    scroll.add(&list);
    root.pack_start(&scroll, true, true, 0);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_media(p))
        .collect();
    files.sort();
    files.reverse();
    if files.is_empty() {
        list.add(&ui::label(
            "No captures yet. Choose a target to get started.",
            "muted",
        ));
    }
    for path in files {
        let button = ui::button(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_ref(),
        );
        let dir = directory.clone();
        let saved = saved.clone();
        button.connect_clicked(move |_| open_path(path.clone(), dir.clone(), saved.clone()));
        list.add(&button);
    }
    window.add(&root);
    window.show_all();
}
fn is_media(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        ["png", "jpg", "jpeg", "webp", "mp4", "gif", "webm"]
            .contains(&s.to_ascii_lowercase().as_str())
    }) && !path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .starts_with("native-")
}
