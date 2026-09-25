use std::{
    path::Path,
    process::Command,
    sync::mpsc::{self, Receiver},
};

use eframe::egui;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
#[cfg(target_os = "windows")]
use tray_icon::{MouseButton, MouseButtonState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    NewCapture,
    ShowRecordingControls,
    CaptureDisplay,
    CaptureRegion,
    CaptureWindow,
    History,
    Preferences,
    OpenOutputFolder,
    #[cfg(target_os = "linux")]
    Unavailable,
    Quit,
}

/// Shipping tray labels and order (`build_tray_menu`), plus the native-only
/// Show Recording Controls item. Separators are deferred: the X11 smokes
/// address rows by equal-height index.
const MENU_ITEMS: [(&str, &str); 9] = [
    ("new-capture", "New Capture…"),
    ("show-recording-controls", "Show Recording Controls"),
    ("capture-region", "Screenshot Region"),
    ("capture-window", "Screenshot Window"),
    ("capture-display", "Screenshot Display"),
    ("history", "Capture History…"),
    ("output", "Open Save Location"),
    ("preferences", "Preferences"),
    ("quit", "Quit Captures"),
];

pub struct Tray {
    _icon: TrayIcon,
    actions: Receiver<Action>,
}

impl Tray {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        let menu = Menu::new();
        for (id, label) in MENU_ITEMS {
            menu.append(&MenuItem::with_id(id, label, true, None))
                .map_err(|error| error.to_string())?;
        }

        let image =
            image::load_from_memory(include_bytes!("../../../desktop/src-tauri/icons/32x32.png"))
                .map_err(|error| format!("Could not decode tray icon: {error}"))?
                .into_rgba8();
        let (width, height) = image.dimensions();
        let icon = Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|error| format!("Could not create tray icon: {error}"))?;
        let builder = TrayIconBuilder::new()
            .with_tooltip("Captures")
            .with_icon(icon)
            .with_menu(Box::new(menu));
        #[cfg(target_os = "windows")]
        let builder = builder.with_menu_on_left_click(false);
        let icon = builder
            .build()
            .map_err(|error| format!("Tray is unavailable: {error}"))?;
        let (actions, receiver) = mpsc::channel();
        #[cfg(target_os = "linux")]
        monitor_backend(actions.clone(), ctx.clone())?;
        let menu_actions = actions.clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id.0.as_str() {
                "new-capture" => Some(Action::NewCapture),
                "show-recording-controls" => Some(Action::ShowRecordingControls),
                "capture-display" => Some(Action::CaptureDisplay),
                "capture-region" => Some(Action::CaptureRegion),
                "capture-window" => Some(Action::CaptureWindow),
                "history" => Some(Action::History),
                "preferences" => Some(Action::Preferences),
                "output" => Some(Action::OpenOutputFolder),
                "quit" => Some(Action::Quit),
                _ => None,
            };
            if let Some(action) = action {
                send_action(&menu_actions, &menu_ctx, action);
            }
        }));

        let tray_actions = actions;
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            #[cfg(target_os = "windows")]
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                send_action(&tray_actions, &ctx, Action::Preferences);
            }
            #[cfg(not(target_os = "windows"))]
            let _ = (&tray_actions, &ctx, event);
        }));
        Ok(Self {
            _icon: icon,
            actions: receiver,
        })
    }

    pub fn try_recv(&self) -> Option<Action> {
        self.actions.try_recv().ok()
    }
}

fn send_action(actions: &mpsc::Sender<Action>, ctx: &egui::Context, action: Action) {
    let _ = actions.send(action);
    // Context clones share the active viewport. A tray callback can run during
    // a preview pass, but only root logic drains this queue (including while
    // hidden). Wake root explicitly instead of repainting/coalescing the child.
    ctx.request_repaint_of(egui::ViewportId::ROOT);
}

#[cfg(target_os = "linux")]
fn backend_available(connection: &dbus::blocking::Connection) -> Result<(), String> {
    use std::time::Duration;

    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;

    let proxy = connection.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_secs(2),
    );
    let (available,): (bool,) = proxy
        .method_call(
            "org.freedesktop.DBus",
            "NameHasOwner",
            ("org.kde.StatusNotifierWatcher",),
        )
        .map_err(|error| format!("Could not query the system tray host: {error}"))?;
    available
        .then_some(())
        .ok_or_else(|| "No StatusNotifier watcher is running on this desktop.".to_owned())?;
    let watcher = connection.with_proxy(
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        Duration::from_secs(2),
    );
    watcher
        .get::<bool>(
            "org.kde.StatusNotifierWatcher",
            "IsStatusNotifierHostRegistered",
        )
        .map_err(|error| format!("Could not query the system tray host: {error}"))?
        .then_some(())
        .ok_or_else(|| "The StatusNotifier watcher has no registered tray host.".into())
}

#[cfg(target_os = "linux")]
fn monitor_backend(actions: mpsc::Sender<Action>, ctx: egui::Context) -> Result<(), String> {
    use std::{thread, time::Duration};

    use dbus::{arg::PropMap, blocking::Connection, message::MatchRule};

    let connection = Connection::new_session()
        .map_err(|error| format!("Could not connect to the desktop session bus: {error}"))?;
    let (recheck, rechecks) = mpsc::channel();
    let owner_actions = actions.clone();
    let owner_ctx = ctx.clone();
    let mut owner_rule = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged");
    owner_rule.sender = Some("org.freedesktop.DBus".into());
    owner_rule.path = Some("/org/freedesktop/DBus".into());
    connection
        .add_match(
            owner_rule,
            move |(name, old_owner, new_owner): (String, String, String), _, _| {
                if name == "org.kde.StatusNotifierWatcher"
                    && !old_owner.is_empty()
                    && old_owner != new_owner
                {
                    send_action(&owner_actions, &owner_ctx, Action::Unavailable);
                }
                true
            },
        )
        .map_err(|error| format!("Could not monitor the system tray watcher: {error}"))?;
    let property_recheck = recheck.clone();
    let host_rule = MatchRule::new_signal("org.freedesktop.DBus.Properties", "PropertiesChanged")
        .with_path("/StatusNotifierWatcher");
    connection
        .add_match(
            host_rule,
            move |(interface, changed, invalidated): (String, PropMap, Vec<String>), _, _| {
                if interface == "org.kde.StatusNotifierWatcher"
                    && (changed.contains_key("IsStatusNotifierHostRegistered")
                        || invalidated
                            .iter()
                            .any(|name| name == "IsStatusNotifierHostRegistered"))
                {
                    let _ = property_recheck.send(());
                }
                true
            },
        )
        .map_err(|error| format!("Could not monitor system tray host state: {error}"))?;
    for member in [
        "StatusNotifierHostRegistered",
        "StatusNotifierHostUnregistered",
    ] {
        let signal_recheck = recheck.clone();
        connection
            .add_match(
                MatchRule::new_signal("org.kde.StatusNotifierWatcher", member)
                    .with_path("/StatusNotifierWatcher"),
                move |(): (), _, _| {
                    let _ = signal_recheck.send(());
                    true
                },
            )
            .map_err(|error| format!("Could not monitor system tray host state: {error}"))?;
    }

    // Subscribe before taking the state snapshot so watcher/host loss cannot
    // fall between the availability check and the event-driven monitor.
    backend_available(&connection)?;
    thread::spawn(move || {
        loop {
            if connection.process(Duration::from_secs(86_400)).is_err() {
                send_action(&actions, &ctx, Action::Unavailable);
                return;
            }
            if rechecks.try_iter().next().is_some() && backend_available(&connection).is_err() {
                send_action(&actions, &ctx, Action::Unavailable);
                return;
            }
        }
    });
    Ok(())
}

impl Drop for Tray {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    }
}

pub fn open_directory(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let status = Command::new("explorer.exe").arg(path).status();
    #[cfg(target_os = "linux")]
    let status = Command::new("xdg-open").arg(path).status();
    status
        .map_err(|error| error.to_string())?
        .success()
        .then_some(())
        .ok_or_else(|| format!("Could not open {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn tray_labels_match_shipping_menu_order() {
        assert_eq!(
            MENU_ITEMS.map(|(_, label)| label),
            [
                "New Capture…",
                "Show Recording Controls",
                "Screenshot Region",
                "Screenshot Window",
                "Screenshot Display",
                "Capture History…",
                "Open Save Location",
                "Preferences",
                "Quit Captures",
            ]
        );
    }

    #[test]
    fn tray_actions_wake_root_even_during_a_preview_pass() {
        for action in [
            Action::History,
            Action::Preferences,
            Action::Quit,
            #[cfg(target_os = "linux")]
            Action::Unavailable,
        ] {
            for preview_active in [false, true] {
                let ctx = egui::Context::default();
                let child = egui::ViewportId::from_hash_of("preview");
                let wakes = Arc::new(Mutex::new(Vec::new()));
                let recorded = wakes.clone();
                ctx.set_request_repaint_callback(move |info| {
                    recorded.lock().unwrap().push(info.viewport_id);
                });
                if preview_active {
                    let mut input = egui::RawInput {
                        viewport_id: child,
                        ..Default::default()
                    };
                    input.viewports.insert(
                        child,
                        egui::ViewportInfo {
                            parent: Some(egui::ViewportId::ROOT),
                            ..Default::default()
                        },
                    );
                    ctx.begin_pass(input);
                }
                wakes.lock().unwrap().clear();
                let (actions, receiver) = mpsc::channel();
                let callback_ctx = ctx.clone();
                std::thread::spawn(move || send_action(&actions, &callback_ctx, action))
                    .join()
                    .unwrap();
                assert_eq!(receiver.try_recv().unwrap(), action);
                assert_eq!(
                    *wakes.lock().unwrap(),
                    [egui::ViewportId::ROOT],
                    "{action:?}, preview active: {preview_active}"
                );
                if preview_active {
                    ctx.end_pass().textures_delta.clear();
                }
            }
        }
    }
}
