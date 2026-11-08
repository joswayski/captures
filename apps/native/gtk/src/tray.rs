//! The live app's tray: shipping `build_tray_menu` as a StatusNotifierItem
//! (`tray-icon` with `ksni`, as the wgpu host; no GTK 3). Menu events arrive
//! on ksni's thread and run on the GTK main loop. Rows the GTK host cannot
//! run yet stay in shipping's order but are disabled.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, accelerator::Accelerator};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::{Host, emit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    NewCapture,
    CaptureRegion,
    CaptureWindow,
    CaptureDisplay,
    RecordRegion,
    RecordWindow,
    RecordDisplay,
    History,
    OpenOutputFolder,
    Preferences,
    SendFeedback,
    CheckUpdates,
    Quit,
}

#[derive(Clone, Copy)]
enum Shortcut {
    NewCapture,
    Region,
    Window,
    Display,
    RecordRegion,
    RecordWindow,
    RecordDisplay,
}

enum Row {
    Item(&'static str, &'static str, Action, Option<Shortcut>),
    Updates,
    Separator,
}

/// Shipping order (`build_tray_menu`), without the update row that only
/// appears while an update is available.
const MENU: [Row; 15] = [
    Row::Item(
        "new-capture",
        "New Capture…",
        Action::NewCapture,
        Some(Shortcut::NewCapture),
    ),
    Row::Item(
        "capture-region",
        "Screenshot Region",
        Action::CaptureRegion,
        Some(Shortcut::Region),
    ),
    Row::Item(
        "capture-window",
        "Screenshot Window",
        Action::CaptureWindow,
        Some(Shortcut::Window),
    ),
    Row::Item(
        "capture-display",
        "Screenshot Display",
        Action::CaptureDisplay,
        Some(Shortcut::Display),
    ),
    Row::Item(
        "record-region",
        "Record Region",
        Action::RecordRegion,
        Some(Shortcut::RecordRegion),
    ),
    Row::Item(
        "record-window",
        "Record Window",
        Action::RecordWindow,
        Some(Shortcut::RecordWindow),
    ),
    Row::Item(
        "record-display",
        "Record Display",
        Action::RecordDisplay,
        Some(Shortcut::RecordDisplay),
    ),
    Row::Separator,
    Row::Item("history", "Capture History…", Action::History, None),
    Row::Item(
        "output",
        "Open Save Location",
        Action::OpenOutputFolder,
        None,
    ),
    Row::Item("preferences", "Preferences", Action::Preferences, None),
    Row::Item(
        "send-feedback",
        "Send Feedback…",
        Action::SendFeedback,
        None,
    ),
    Row::Updates,
    Row::Separator,
    Row::Item("quit", "Quit Captures", Action::Quit, None),
];

/// The captures the GTK host can run so far.
fn available(action: Action) -> bool {
    !matches!(
        action,
        Action::RecordRegion | Action::RecordWindow | Action::RecordDisplay
    ) || gtk::gdk::Display::default()
        .is_some_and(|display| display.type_().name() == "GdkX11Display")
}

fn shortcut(settings: &captures_settings::AppSettings, shortcut: Shortcut) -> &str {
    match shortcut {
        Shortcut::NewCapture => &settings.new_capture_shortcut,
        Shortcut::Region => &settings.region_shortcut,
        Shortcut::Window => &settings.window_shortcut,
        Shortcut::Display => &settings.display_shortcut,
        Shortcut::RecordRegion => &settings.recording.video_shortcut,
        Shortcut::RecordWindow => &settings.recording.window_shortcut,
        Shortcut::RecordDisplay => &settings.recording.display_shortcut,
    }
}

/// Shipping `tray_accelerator`: `KeyA`/`Digit1` codes become their key.
fn accelerator(shortcut: &str) -> Option<Accelerator> {
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        return None;
    }
    shortcut
        .split('+')
        .map(|token| {
            let token = token.trim();
            let lower = token.to_ascii_lowercase();
            if (lower.starts_with("digit") && token.len() == 6)
                || (lower.starts_with("key") && token.len() == 4)
            {
                token[token.len() - 1..].to_owned()
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
        .parse()
        .ok()
}

/// The menu's accelerators, so a rebuild happens only when they change.
fn accelerators(settings: &captures_settings::AppSettings) -> Vec<String> {
    [
        Shortcut::NewCapture,
        Shortcut::Region,
        Shortcut::Window,
        Shortcut::Display,
        Shortcut::RecordRegion,
        Shortcut::RecordWindow,
        Shortcut::RecordDisplay,
    ]
    .map(|which| shortcut(settings, which).to_owned())
    .to_vec()
}

fn menu(settings: &captures_settings::AppSettings) -> Result<Menu, String> {
    let menu = Menu::new();
    // Check for Updates works only in a Preview build with update metadata.
    let updates = captures_app::preview_channel::version().is_some();
    for row in &MENU {
        let result = match *row {
            Row::Item(id, label, action, keys) => menu.append(&MenuItem::with_id(
                id,
                label,
                available(action),
                keys.and_then(|keys| accelerator(shortcut(settings, keys))),
            )),
            Row::Updates => menu.append(&MenuItem::with_id(
                "check-updates",
                "Check for Updates…",
                updates,
                None,
            )),
            Row::Separator => menu.append(&PredefinedMenuItem::separator()),
        };
        result.map_err(|error| error.to_string())?;
    }
    Ok(menu)
}

pub struct Tray {
    icon: TrayIcon,
    accelerators: Vec<String>,
}

impl Tray {
    /// Refresh the menu's accelerators after the saved shortcuts change.
    pub fn update(&mut self, settings: &captures_settings::AppSettings) {
        let accelerators = accelerators(settings);
        if accelerators == self.accelerators {
            return;
        }
        match menu(settings) {
            Ok(menu) => {
                self.icon.set_menu(Some(Box::new(menu)));
                self.accelerators = accelerators;
            }
            Err(error) => eprintln!("Could not refresh tray shortcuts: {error}"),
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    }
}

/// Show the tray if the desktop has a StatusNotifier host. `None` when it
/// has none, with the reason reported.
pub fn start(settings: &captures_settings::AppSettings) -> Option<Tray> {
    if let Err(error) = host_available() {
        emit("tray-unavailable", serde_json::json!({"error": error}));
        return None;
    }
    let built = (|| {
        let image =
            image::load_from_memory(include_bytes!("../../../desktop/src-tauri/icons/32x32.png"))
                .map_err(|error| format!("Could not decode tray icon: {error}"))?
                .into_rgba8();
        let (width, height) = image.dimensions();
        let icon = Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|error| format!("Could not create tray icon: {error}"))?;
        TrayIconBuilder::new()
            .with_tooltip("Captures")
            .with_icon(icon)
            .with_menu(Box::new(menu(settings)?))
            .build()
            .map_err(|error| format!("Tray is unavailable: {error}"))
    })();
    let icon = match built {
        Ok(icon) => icon,
        Err(error) => {
            emit("tray-unavailable", serde_json::json!({"error": error}));
            return None;
        }
    };
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let Some(action) = MENU.iter().find_map(|row| match row {
            Row::Item(id, _, action, _) if *id == event.id.0 => Some(*action),
            Row::Updates if event.id.0 == "check-updates" => Some(Action::CheckUpdates),
            _ => None,
        }) else {
            return;
        };
        glib::MainContext::default().invoke(move || crate::live::tray_action(action));
    }));
    emit("tray", serde_json::json!({}));
    Some(Tray {
        icon,
        accelerators: accelerators(settings),
    })
}

/// Shipping's Linux tray needs a StatusNotifier watcher with a registered
/// host; without one the icon would never appear.
fn host_available() -> Result<(), String> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|error| format!("Could not connect to the desktop session bus: {error}"))?;
    let watcher = gio::DBusProxy::new_sync(
        &bus,
        gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        Some("org.kde.StatusNotifierWatcher"),
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
        gio::Cancellable::NONE,
    )
    .map_err(|error| format!("Could not query the system tray host: {error}"))?;
    if watcher.name_owner().is_none() {
        return Err("No StatusNotifier watcher is running on this desktop.".into());
    }
    watcher
        .cached_property("IsStatusNotifierHostRegistered")
        .and_then(|value| value.get::<bool>())
        .filter(|registered| *registered)
        .map(|_| ())
        .ok_or_else(|| "The StatusNotifier watcher has no registered tray host.".into())
}

/// Run one tray action on the main loop.
pub fn run(host: &Rc<Host>, action: Action) {
    emit(
        "tray-action",
        serde_json::json!({"action": format!("{action:?}")}),
    );
    match action {
        Action::CaptureDisplay => host.capture_display(),
        Action::CaptureRegion => host.capture_region(),
        Action::CaptureWindow => host.capture_window(),
        Action::NewCapture => host.new_capture(),
        Action::History => {
            host.show_history();
        }
        Action::Preferences | Action::CheckUpdates => {
            host.show_preferences();
        }
        Action::SendFeedback => {
            host.show_feedback();
        }
        Action::OpenOutputFolder => {
            let settings = captures_settings::load(&host.settings_file).unwrap_or_default();
            let path = std::path::PathBuf::from(settings.output_directory);
            let opened = std::fs::create_dir_all(&path)
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    gio::AppInfo::launch_default_for_uri(
                        &gio::File::for_path(&path).uri(),
                        gio::AppLaunchContext::NONE,
                    )
                    .map_err(|error| error.to_string())
                });
            if let Err(error) = opened {
                host.show_error(&format!("Could not open the save location: {error}"));
            }
        }
        Action::Quit => host.app.quit(),
        Action::RecordRegion => host.record_menu(crate::capture_bar::Target::Region),
        Action::RecordWindow => host.record_menu(crate::capture_bar::Target::Window),
        Action::RecordDisplay => host.record_menu(crate::capture_bar::Target::Display),
    }
}
