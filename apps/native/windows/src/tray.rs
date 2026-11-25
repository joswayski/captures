//! The resident app's notification-area icon and menu, in shipping order
//! (`build_tray_menu`). Menus are the native Win32 menus shipping's Tauri
//! tray uses (`tray-icon`/`muda`); their window procedures run on this
//! thread inside the host loop's `DispatchMessageW`.

use std::sync::mpsc::{Receiver, Sender, channel};

use captures_app::shortcuts::{
    CaptureShortcut, ShortcutPlatform, conflict_menu_label, conflict_tooltip, shortcut_text,
};
use captures_app::update_notice::{TrayUpdateItem, UpdateStatus, tray_tooltip, tray_update_item};
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
    /// The icon was pressed; its menu opens on release. Check the
    /// conflicting shortcuts again so that menu is current.
    Pressed,
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

fn accelerators(settings: &captures_settings::AppSettings) -> Vec<String> {
    MENU.iter()
        .filter_map(|row| match row {
            Row::Item(_, _, _, Some(keys)) => Some(shortcut(settings, *keys).to_owned()),
            _ => None,
        })
        .collect()
}

fn menu(
    settings: &captures_settings::AppSettings,
    update: &TrayUpdateItem,
    conflicts: &[CaptureShortcut],
) -> Result<Menu, String> {
    let menu = Menu::new();
    // Check for Updates works only in a Preview build with update metadata.
    let updates = captures_app::preview_channel::version().is_some();
    let update_row = || {
        MenuItem::with_id(
            "check-updates",
            &update.label,
            updates && update.enabled,
            None,
        )
    };
    // Shipping pins an available or installing update above New Capture.
    if update.pin_first {
        menu.append(&update_row())
            .and_then(|()| menu.append(&PredefinedMenuItem::separator()))
            .map_err(|error| error.to_string())?;
    }
    for row in &MENU {
        let result = match *row {
            // Shipping `tray_menu_item`: a row whose accelerator the menu
            // rejects (Windows menus cannot show Super) keeps no shortcut.
            // A chord another app holds never fires, so it shows no hint.
            Row::Item(id, label, _, keys) => keys
                .filter(|keys| !conflicts.contains(&keys.capture()))
                .and_then(|keys| accelerator(shortcut(settings, keys)))
                .and_then(|keys| {
                    menu.append(&MenuItem::with_id(id, label, true, Some(keys)))
                        .ok()
                })
                .map_or_else(
                    || menu.append(&MenuItem::with_id(id, label, true, None)),
                    Ok,
                ),
            Row::Updates if !update.pin_first => menu.append(&update_row()),
            Row::Updates => Ok(()),
            Row::Separator => menu.append(&PredefinedMenuItem::separator()),
        };
        result.map_err(|error| error.to_string())?;
        // Conflict rows follow Record Display, before the first separator.
        if matches!(row, Row::Item(_, _, _, Some(Shortcut::RecordDisplay))) {
            for action in conflicts {
                let label = conflict_menu_label(
                    shortcut_text(settings, *action),
                    ShortcutPlatform::Windows,
                );
                let id = format!("{CHANGE_SHORTCUT}{}", action.id());
                menu.append(&MenuItem::with_id(id, label, true, None))
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(menu)
}

fn action(id: &str) -> Option<Action> {
    if id.starts_with(CHANGE_SHORTCUT) {
        return Some(Action::ChangeShortcut);
    }
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
    settings: captures_settings::AppSettings,
    update: TrayUpdateItem,
    update_tooltip: &'static str,
    conflicts: Vec<CaptureShortcut>,
}

impl Tray {
    /// `wake` runs on whichever thread delivered the event.
    pub fn new(
        settings: &captures_settings::AppSettings,
        wake: impl Fn() + Send + Sync + Clone + 'static,
    ) -> Result<Self, String> {
        let image = image::load_from_memory(include_bytes!("../../bundle/icons/32x32.png"))
            .map_err(|error| format!("Could not decode tray icon: {error}"))?
            .into_rgba8();
        let (width, height) = image.dimensions();
        let icon = Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|error| format!("Could not create tray icon: {error}"))?;
        let icon = TrayIconBuilder::new()
            .with_tooltip(captures_app::preview_channel::TRAY_NAME)
            .with_icon(icon)
            .with_menu(Box::new(menu(settings, &tray_update_item(None), &[])?))
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
            // Every row opens a window: create the device while the menu is up.
            if matches!(
                event,
                TrayIconEvent::Click {
                    button_state: MouseButtonState::Down,
                    ..
                }
            ) {
                crate::gfx::prewarm();
                send(&sender, &wake, Action::Pressed);
            }
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
            settings: settings.clone(),
            update: tray_update_item(None),
            update_tooltip: tray_tooltip(None),
            conflicts: Vec::new(),
        })
    }

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
        let tooltip = conflict_tooltip(self.update_tooltip.to_owned(), self.conflicts.len());
        if let Err(error) = self.icon.set_tooltip(Some(tooltip)) {
            eprintln!("Could not refresh the tray tooltip: {error}");
        }
    }

    /// The icon's physical screen rect, once the shell has laid it out.
    pub fn rect(&self) -> Option<windows::Win32::Foundation::RECT> {
        let rect = self.icon.rect()?;
        let (x, y) = (
            rect.position.x.round() as i32,
            rect.position.y.round() as i32,
        );
        (rect.size.width > 0 && rect.size.height > 0).then(|| windows::Win32::Foundation::RECT {
            left: x,
            top: y,
            right: x + rect.size.width as i32,
            bottom: y + rect.size.height as i32,
        })
    }

    pub fn next_action(&self) -> Option<Action> {
        self.actions.try_recv().ok()
    }

    /// Refresh the menu's accelerators after the saved shortcuts change.
    pub fn update(&mut self, settings: &captures_settings::AppSettings) {
        if accelerators(settings) == self.accelerators {
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

    #[test]
    fn a_held_shortcut_loses_its_hint_and_gets_a_change_row() {
        let settings = captures_settings::AppSettings {
            new_capture_shortcut: "Ctrl+Shift+Space".into(),
            ..Default::default()
        };
        let conflicted = menu(
            &settings,
            &tray_update_item(None),
            &[CaptureShortcut::NewCapture],
        )
        .unwrap();
        let items = conflicted.items();
        assert_eq!(items[0].id().0, "new-capture");
        let row = items[7].as_menuitem().unwrap();
        assert_eq!(
            row.text(),
            "Ctrl+Shift+Space is used by another app — Change Shortcut…"
        );
        assert_eq!(action(&row.id().0), Some(Action::ChangeShortcut));
        assert!(matches!(
            items[8],
            tray_icon::menu::MenuItemKind::Predefined(_)
        ));
        let resting = menu(&settings, &tray_update_item(None), &[]).unwrap();
        assert_eq!(items.len(), resting.items().len() + 1);
    }

    #[test]
    fn an_available_update_moves_its_row_above_new_capture() {
        use tray_icon::menu::MenuItemKind;
        let settings = captures_settings::AppSettings::default();
        let ids = |menu: &Menu| -> Vec<String> {
            menu.items()
                .iter()
                .map(|item| item.id().0.clone())
                .collect()
        };
        let resting = menu(&settings, &tray_update_item(None), &[]).unwrap();
        assert_eq!(ids(&resting)[0], "new-capture");
        let pinned = TrayUpdateItem {
            label: "Update Available — 2 changes".into(),
            enabled: true,
            pin_first: true,
        };
        let pinned = menu(&settings, &pinned, &[]).unwrap();
        let items = pinned.items();
        assert_eq!(
            items[0].as_menuitem().map(|item| item.text()).as_deref(),
            Some("Update Available — 2 changes")
        );
        assert!(matches!(items[1], MenuItemKind::Predefined(_)));
        assert_eq!(ids(&pinned)[2], "new-capture");
        // The row moves rather than appearing twice.
        assert_eq!(
            ids(&pinned)
                .iter()
                .filter(|id| *id == "check-updates")
                .count(),
            1
        );
        assert_eq!(items.len(), resting.items().len() + 1);
    }
}
