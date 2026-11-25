//! The live app's tray: shipping `build_tray_menu` as a StatusNotifierItem
//! (`tray-icon` with `ksni`, as the wgpu host; no GTK 3). Menu events arrive
//! on ksni's thread and run on the GTK main loop. Rows the GTK host cannot
//! run yet stay in shipping's order but are disabled.

use std::rc::Rc;

use captures_app::shortcuts::{
    CaptureShortcut, ShortcutPlatform, conflict_menu_label, conflict_tooltip, shortcut_text,
};
use captures_app::update_notice::{TrayUpdateItem, UpdateStatus, tray_tooltip, tray_update_item};
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
    /// A "… is used by another app — Change Shortcut…" row.
    ChangeShortcut,
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
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

/// Shipping order (`build_tray_menu`); an available or installing update
/// moves the update row above New Capture.
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
    Row::Item("history", "Capture History", Action::History, None),
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

impl Shortcut {
    fn capture(self) -> CaptureShortcut {
        match self {
            Self::NewCapture => CaptureShortcut::NewCapture,
            Self::Region => CaptureShortcut::Region,
            Self::Window => CaptureShortcut::Window,
            Self::Display => CaptureShortcut::Display,
            Self::RecordRegion => CaptureShortcut::RecordRegion,
            Self::RecordWindow => CaptureShortcut::RecordWindow,
            Self::RecordDisplay => CaptureShortcut::RecordDisplay,
        }
    }
}

fn shortcut(settings: &captures_settings::AppSettings, shortcut: Shortcut) -> &str {
    shortcut_text(settings, shortcut.capture())
}

/// The id of a conflict row, which opens Preferences at Shortcuts.
const CHANGE_SHORTCUT: &str = "change-shortcut-";

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

fn menu(
    settings: &captures_settings::AppSettings,
    update: &TrayUpdateItem,
    conflicts: &[CaptureShortcut],
) -> Result<Menu, String> {
    let menu = Menu::new();
    // Check for Updates works only in a Preview build with update metadata.
    let updates = crate::updates::enabled();
    let update_row = || {
        MenuItem::with_id(
            "check-updates",
            &update.label,
            updates && update.enabled,
            None,
        )
    };
    let append = |item: &dyn tray_icon::menu::IsMenuItem| {
        menu.append(item).map_err(|error| error.to_string())
    };
    // Shipping pins an available or installing update above New Capture.
    if update.pin_first {
        append(&update_row())?;
        append(&PredefinedMenuItem::separator())?;
    }
    // A shortcut another app holds is pinned first, each naming its action,
    // so the menu says which one never fires before anything else.
    for action in conflicts {
        let keys = shortcut_text(settings, *action);
        let label = conflict_menu_label(*action, keys, ShortcutPlatform::Linux);
        let id = format!("{CHANGE_SHORTCUT}{}", action.id());
        append(&MenuItem::with_id(id, label, true, None))?;
    }
    if !conflicts.is_empty() {
        append(&PredefinedMenuItem::separator())?;
    }
    for row in &MENU {
        match *row {
            Row::Item(id, label, _, keys) => append(&MenuItem::with_id(
                id,
                label,
                // Every row runs on X11 and Wayland; Wayland recording
                // refuses at start where there is no ScreenCast portal.
                true,
                // A chord another app holds never fires, so it shows no hint.
                keys.filter(|keys| !conflicts.contains(&keys.capture()))
                    .and_then(|keys| accelerator(shortcut(settings, keys))),
            ))?,
            Row::Updates if !update.pin_first => append(&update_row())?,
            Row::Updates => {}
            Row::Separator => append(&PredefinedMenuItem::separator())?,
        }
    }
    Ok(menu)
}

pub struct Tray {
    icon: TrayIcon,
    accelerators: Vec<String>,
    settings: captures_settings::AppSettings,
    update: TrayUpdateItem,
    update_tooltip: &'static str,
    conflicts: Vec<CaptureShortcut>,
}

impl Tray {
    /// Follow the shortcuts another app holds: their rows, hints and the
    /// tooltip, which outranks the update state's while any is held.
    pub fn set_conflicts(&mut self, conflicts: Vec<CaptureShortcut>) {
        if conflicts == self.conflicts {
            return;
        }
        self.conflicts = conflicts;
        self.refresh_tooltip();
        self.rebuild();
    }

    fn refresh_tooltip(&self) {
        let tooltip = conflict_tooltip(self.update_tooltip.to_owned(), &self.conflicts);
        if let Err(error) = self.icon.set_tooltip(Some(tooltip)) {
            eprintln!("Could not refresh the tray tooltip: {error}");
        }
    }

    /// Refresh the menu's accelerators after the saved shortcuts change.
    pub fn update(&mut self, settings: &captures_settings::AppSettings) {
        let accelerators = accelerators(settings);
        if accelerators == self.accelerators {
            return;
        }
        self.settings = settings.clone();
        self.rebuild();
    }

    /// Shipping's `refresh_tray_update_state`: the update row and tooltip
    /// follow the update status ("Update Available — N changes", pinned
    /// first; "Installing Update…"; "Checking for Updates…").
    pub fn set_update(&mut self, status: Option<&UpdateStatus>) {
        let update = tray_update_item(status);
        if update == self.update {
            return;
        }
        self.update = update;
        self.update_tooltip = tray_tooltip(status);
        self.refresh_tooltip();
        self.rebuild();
    }

    fn rebuild(&mut self) {
        match menu(&self.settings, &self.update, &self.conflicts) {
            Ok(menu) => {
                self.icon.set_menu(Some(Box::new(menu)));
                self.accelerators = accelerators(&self.settings);
            }
            Err(error) => eprintln!("Could not refresh the tray menu: {error}"),
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
        let image = image::load_from_memory(include_bytes!("../../bundle/icons/32x32.png"))
            .map_err(|error| format!("Could not decode tray icon: {error}"))?
            .into_rgba8();
        let (width, height) = image.dimensions();
        let icon = Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|error| format!("Could not create tray icon: {error}"))?;
        TrayIconBuilder::new()
            .with_tooltip(captures_app::preview_channel::TRAY_NAME)
            .with_icon(icon)
            .with_menu(Box::new(menu(settings, &tray_update_item(None), &[])?))
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
        let change = event
            .id
            .0
            .starts_with(CHANGE_SHORTCUT)
            .then_some(Action::ChangeShortcut);
        let Some(action) = change.or_else(|| {
            MENU.iter().find_map(|row| match row {
                Row::Item(id, _, action, _) if *id == event.id.0 => Some(*action),
                Row::Updates if event.id.0 == "check-updates" => Some(Action::CheckUpdates),
                _ => None,
            })
        }) else {
            return;
        };
        glib::MainContext::default().invoke(move || crate::live::tray_action(action));
    }));
    emit("tray", serde_json::json!({}));
    Some(Tray {
        icon,
        accelerators: accelerators(settings),
        settings: settings.clone(),
        update: tray_update_item(None),
        update_tooltip: tray_tooltip(None),
        conflicts: Vec::new(),
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
    use captures_app::capture_error::{Action as Route, Target};
    emit(
        "tray-action",
        serde_json::json!({"action": format!("{action:?}")}),
    );
    match action {
        Action::CaptureDisplay => host.capture_action(Route::Screenshot(Target::Display)),
        Action::CaptureRegion => host.capture_action(Route::Screenshot(Target::Region)),
        Action::CaptureWindow => host.capture_action(Route::Screenshot(Target::Window)),
        Action::NewCapture => host.capture_action(Route::NewCapture),
        Action::History => {
            host.show_history();
        }
        Action::Preferences => {
            host.show_preferences();
            crate::live::recheck_shortcut_conflicts(host);
        }
        Action::ChangeShortcut => {
            host.show_preferences();
            crate::live::recheck_shortcut_conflicts(host);
            crate::preferences::show_section("shortcuts");
        }
        // Shipping's `handle_tray_action`: an update in hand shows the
        // notice; otherwise check now, then show the notice for an update
        // or say up to date or why the check failed.
        Action::CheckUpdates => crate::updates::check_from_tray(),
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
        Action::RecordRegion => host.capture_action(Route::Record(Target::Region)),
        Action::RecordWindow => host.capture_action(Route::Record(Target::Window)),
        Action::RecordDisplay => host.capture_action(Route::Record(Target::Display)),
    }
}
