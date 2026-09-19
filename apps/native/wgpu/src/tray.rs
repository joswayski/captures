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

pub struct Tray {
    _icon: TrayIcon,
    actions: Receiver<Action>,
}

impl Tray {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        let capture_display = MenuItem::with_id("capture-display", "Capture display", true, None);
        let capture_region = MenuItem::with_id("capture-region", "Capture region", true, None);
        let capture_window = MenuItem::with_id("capture-window", "Capture window", true, None);
        let history = MenuItem::with_id("history", "History", true, None);
        let preferences = MenuItem::with_id("preferences", "Preferences", true, None);
        let output = MenuItem::with_id("output", "Open output folder", true, None);
        let quit = MenuItem::with_id("quit", "Quit Captures", true, None);
        let menu = Menu::new();
        for item in [
            &capture_display,
            &capture_region,
            &capture_window,
            &history,
            &preferences,
            &output,
            &quit,
        ] {
            menu.append(item).map_err(|error| error.to_string())?;
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
                let _ = menu_actions.send(action);
                menu_ctx.request_repaint();
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
                let _ = tray_actions.send(Action::Preferences);
                ctx.request_repaint();
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
                    let _ = owner_actions.send(Action::Unavailable);
                    owner_ctx.request_repaint();
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
                let _ = actions.send(Action::Unavailable);
                ctx.request_repaint();
                return;
            }
            if rechecks.try_iter().next().is_some() && backend_available(&connection).is_err() {
                let _ = actions.send(Action::Unavailable);
                ctx.request_repaint();
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
