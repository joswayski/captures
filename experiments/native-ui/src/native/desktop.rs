//! X11 hotkeys and AppIndicator menu, polled on GTK's owning thread.
use crate::settings::Settings;
use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};
use std::{collections::HashSet, path::PathBuf};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuItem},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Capture(u32, u32),
    Preferences,
    History,
    Open,
    Canvas,
    Quit,
}

pub struct Desktop {
    manager: GlobalHotKeyManager,
    keys: Vec<(HotKey, Action)>,
    _tray: TrayIcon,
    menu: Vec<(MenuItem, Action)>,
}

impl Desktop {
    pub fn new() -> Result<Self, String> {
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
            return Err(
                "Global shortcuts require X11; XWayland grabs are not compositor-wide.".into(),
            );
        }
        let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
        let menu = Menu::new();
        let actions = [
            ("New capture", Action::Capture(0, 0)),
            ("Region screenshot", Action::Capture(0, 0)),
            ("Window screenshot", Action::Capture(0, 1)),
            ("Full screen screenshot", Action::Capture(0, 2)),
            ("Record video", Action::Capture(1, 0)),
            ("Record GIF", Action::Capture(2, 0)),
            ("Capture history", Action::History),
            ("Image / video editor…", Action::Open),
            ("Create an asset…", Action::Canvas),
            ("Preferences", Action::Preferences),
            ("Quit Captures", Action::Quit),
        ];
        let mut items = Vec::new();
        for (label, action) in actions {
            let item = MenuItem::new(label, true, None);
            menu.append(&item).map_err(|e| e.to_string())?;
            items.push((item, action));
        }
        let mut rgba = vec![0; 24 * 24 * 4];
        for y in 3..21 {
            for x in 3..21 {
                if !(6..18).contains(&x) || !(6..18).contains(&y) {
                    let i = (y * 24 + x) * 4;
                    rgba[i..i + 4].copy_from_slice(&[246, 246, 248, 255]);
                }
            }
        }
        let icon = Icon::from_rgba(rgba, 24, 24).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("Captures — Linux native preview")
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            manager,
            keys: vec![],
            _tray: tray,
            menu: items,
        })
    }

    pub fn replace_shortcuts(&mut self, settings: &Settings) -> Result<(), String> {
        let desired = shortcuts(settings)?;
        let mut added = Vec::new();
        for (key, _) in &desired {
            if self.keys.iter().any(|(old, _)| old == key) {
                continue;
            }
            if let Err(error) = self.manager.register(*key) {
                for key in added {
                    let _ = self.manager.unregister(key);
                }
                return Err(format!(
                    "Cannot register shortcut {key:?}: {error}. Existing shortcuts are unchanged."
                ));
            }
            added.push(*key);
        }
        let mut removed = Vec::new();
        for (key, _) in &self.keys {
            if desired.iter().any(|(new, _)| new == key) {
                continue;
            }
            if let Err(error) = self.manager.unregister(*key) {
                for key in added {
                    let _ = self.manager.unregister(key);
                }
                for key in removed {
                    let _ = self.manager.register(key);
                }
                return Err(error.to_string());
            }
            removed.push(*key);
        }
        self.keys = desired;
        Ok(())
    }

    pub fn events(&self) -> Vec<Action> {
        let mut actions = vec![];
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if let Some((_, action)) = self.menu.iter().find(|(item, _)| *item.id() == event.id) {
                actions.push(*action);
            }
        }
        while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
            if event.state == global_hotkey::HotKeyState::Pressed
                && let Some((_, action)) = self.keys.iter().find(|(key, _)| key.id() == event.id)
            {
                actions.push(*action);
            }
        }
        actions
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        for (key, _) in &self.keys {
            let _ = self.manager.unregister(*key);
        }
    }
}

pub fn shortcuts(settings: &Settings) -> Result<Vec<(HotKey, Action)>, String> {
    let r = &settings.recording;
    let mut seen = HashSet::new();
    [
        (&settings.new_capture_shortcut, Action::Capture(0, 0)),
        (&settings.region_shortcut, Action::Capture(0, 0)),
        (&settings.window_shortcut, Action::Capture(0, 1)),
        (&settings.display_shortcut, Action::Capture(0, 2)),
        (&r.video_shortcut, Action::Capture(1, 0)),
        (&r.window_shortcut, Action::Capture(1, 1)),
        (&r.display_shortcut, Action::Capture(1, 2)),
        (&r.gif_shortcut, Action::Capture(2, 0)),
    ]
    .into_iter()
    .filter(|(text, _)| !text.trim().is_empty())
    .map(|(text, action)| {
        let key = text
            .parse::<HotKey>()
            .map_err(|e| format!("Invalid shortcut {text}: {e}"))?;
        if !seen.insert(key.id()) {
            return Err(format!("Shortcut {text} is assigned more than once."));
        }
        Ok((key, action))
    })
    .collect()
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    let path = config.join("autostart/captures-linux-native.desktop");
    if !enabled {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    } else {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let executable = executable.to_str().ok_or("Executable path must be UTF-8")?;
        if executable.contains(['\n', '\r']) {
            return Err("Invalid executable path".into());
        }
        let quoted = executable
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%");
        crate::settings::atomic_write(&path,format!("[Desktop Entry]\nType=Application\nName=Captures Linux Native\nExec=\"{quoted}\" --background\nTerminal=false\nX-GNOME-Autostart-enabled=true\n").as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_shortcuts_are_valid_and_duplicate_bindings_are_rejected() {
        let mut s = Settings::default();
        assert_eq!(shortcuts(&s).unwrap().len(), 8);
        s.recording.gif_shortcut = s.window_shortcut.clone();
        assert!(shortcuts(&s).is_err());
        s.recording.gif_shortcut.clear();
        assert_eq!(shortcuts(&s).unwrap().len(), 7);
    }
}
