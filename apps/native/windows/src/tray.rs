//! The resident app's notification-area icon and menu, in shipping order
//! (`build_tray_menu`). Menus are the native Win32 menus shipping's Tauri
//! tray uses (`tray-icon`/`muda`); their window procedures run on this
//! thread inside the host loop's `DispatchMessageW`.

use std::sync::mpsc::{Receiver, Sender, channel};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, accelerator::Accelerator};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

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
    /// Shipping's left click opens Preferences once no notice is up.
    LeftClick,
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

/// What the Win32 host can run so far. The screenshot rows open the capture
/// bar; the recording and feedback rows stay visible but disabled.
fn available(action: Action) -> bool {
    !matches!(
        action,
        Action::RecordRegion | Action::RecordWindow | Action::RecordDisplay | Action::SendFeedback
    )
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

fn accelerators(settings: &captures_settings::AppSettings) -> Vec<String> {
    MENU.iter()
        .filter_map(|row| match row {
            Row::Item(_, _, _, Some(keys)) => Some(shortcut(settings, *keys).to_owned()),
            _ => None,
        })
        .collect()
}

fn menu(settings: &captures_settings::AppSettings) -> Result<Menu, String> {
    let menu = Menu::new();
    // Check for Updates works only in a Preview build with update metadata.
    let updates = captures_app::preview_channel::version().is_some();
    for row in &MENU {
        let result = match *row {
            // Shipping `tray_menu_item`: a row whose accelerator the menu
            // rejects (Windows menus cannot show Super) keeps no shortcut.
            Row::Item(id, label, action, keys) => keys
                .and_then(|keys| accelerator(shortcut(settings, keys)))
                .and_then(|keys| {
                    menu.append(&MenuItem::with_id(id, label, available(action), Some(keys)))
                        .ok()
                })
                .map_or_else(
                    || menu.append(&MenuItem::with_id(id, label, available(action), None)),
                    Ok,
                ),
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

fn action(id: &str) -> Option<Action> {
    MENU.iter().find_map(|row| match row {
        Row::Item(row, _, action, _) if *row == id => Some(*action),
        Row::Updates if id == "check-updates" => Some(Action::CheckUpdates),
        _ => None,
    })
}

pub struct Tray {
    icon: TrayIcon,
    actions: Receiver<Action>,
    accelerators: Vec<String>,
}

impl Tray {
    /// `wake` runs on whichever thread delivered the event.
    pub fn new(
        settings: &captures_settings::AppSettings,
        wake: impl Fn() + Send + Sync + Clone + 'static,
    ) -> Result<Self, String> {
        let image =
            image::load_from_memory(include_bytes!("../../../desktop/src-tauri/icons/32x32.png"))
                .map_err(|error| format!("Could not decode tray icon: {error}"))?
                .into_rgba8();
        let (width, height) = image.dimensions();
        let icon = Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|error| format!("Could not create tray icon: {error}"))?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Captures")
            .with_icon(icon)
            .with_menu(Box::new(menu(settings)?))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| format!("Tray is unavailable: {error}"))?;
        let (sender, actions) = channel();
        let send = |sender: &Sender<Action>, wake: &dyn Fn(), action| {
            let _ = sender.send(action);
            wake();
        };
        let (menu_sender, menu_wake) = (sender.clone(), wake.clone());
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = action(&event.id.0) {
                send(&menu_sender, &menu_wake, action);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                send(&sender, &wake, Action::LeftClick);
            }
        }));
        Ok(Self {
            icon,
            actions,
            accelerators: accelerators(settings),
        })
    }

    pub fn next_action(&self) -> Option<Action> {
        self.actions.try_recv().ok()
    }

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
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_route_to_their_rows_and_settings_drive_accelerators() {
        assert_eq!(action("capture-window"), Some(Action::CaptureWindow));
        assert_eq!(action("check-updates"), Some(Action::CheckUpdates));
        assert_eq!(action("quit"), Some(Action::Quit));
        assert_eq!(action("unknown"), None);
        let settings = captures_settings::AppSettings {
            region_shortcut: "Ctrl+Shift+KeyR".into(),
            ..Default::default()
        };
        assert_eq!(accelerators(&settings)[1], "Ctrl+Shift+KeyR");
        assert!(accelerator("Ctrl+Shift+KeyR").is_some());
        assert!(accelerator("").is_none());
    }
}
