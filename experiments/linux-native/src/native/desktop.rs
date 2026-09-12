//! Native hotkeys and tray menu, polled on GTK's owning thread.
use crate::settings::Settings;
use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};
use ksni::blocking::TrayMethods;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
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
    _tray: ksni::blocking::Handle<CapturesTray>,
    tray_events: Receiver<Action>,
}

struct CapturesTray {
    events: Sender<Action>,
}

impl ksni::Tray for CapturesTray {
    fn id(&self) -> String {
        "captures-linux-native".into()
    }

    fn title(&self) -> String {
        "Captures".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let size = 24usize;
        let mut data = vec![0; size * size * 4];
        for y in 3..21 {
            for x in 3..21 {
                if !(6..18).contains(&x) || !(6..18).contains(&y) {
                    let offset = (y * size + x) * 4;
                    data[offset..offset + 4].copy_from_slice(&[255, 246, 246, 248]);
                }
            }
        }
        vec![ksni::Icon {
            width: size as i32,
            height: size as i32,
            data,
        }]
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        [
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
        ]
        .into_iter()
        .map(|(label, action)| {
            MenuItem::Standard(StandardItem {
                label: label.into(),
                activate: Box::new(move |tray: &mut Self| {
                    let _ = tray.events.send(action);
                }),
                ..Default::default()
            })
        })
        .collect()
    }
}

impl Desktop {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
            return Err(
                "Global shortcuts require X11; XWayland grabs are not compositor-wide.".into(),
            );
        }
        let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
        let (sender, tray_events) = mpsc::channel();
        let tray = CapturesTray { events: sender }
            .spawn()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            manager,
            keys: vec![],
            _tray: tray,
            tray_events,
        })
    }

    pub fn replace_shortcuts(&mut self, settings: &Settings) -> Result<(), String> {
        let desired = shortcuts(settings)?;
        #[cfg(target_os = "windows")]
        if desired.iter().any(|(key, _)| intercepted(key)) {
            captures_session::ensure_win_shift_s_takeover()?;
            captures_session::set_win_shift_s_handler(Some(|phase| {
                if phase == captures_session::WinShiftSPhase::Released {
                    WIN_SHIFT_S.store(true, std::sync::atomic::Ordering::Release);
                }
            }));
        }
        let mut added = Vec::new();
        for (key, _) in &desired {
            if intercepted(key) || self.keys.iter().any(|(old, _)| old == key) {
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
            if intercepted(key) || desired.iter().any(|(new, _)| new == key) {
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
        #[cfg(target_os = "windows")]
        captures_session::set_win_shift_s_takeover_enabled(
            desired.iter().any(|(key, _)| intercepted(key)),
        );
        self.keys = desired;
        Ok(())
    }

    pub fn events(&self) -> Vec<Action> {
        let mut actions = vec![];
        #[cfg(target_os = "windows")]
        if WIN_SHIFT_S.swap(false, std::sync::atomic::Ordering::AcqRel)
            && let Some((_, action)) = self.keys.iter().find(|(key, _)| intercepted(key))
        {
            actions.push(*action);
        }
        while let Ok(action) = self.tray_events.try_recv() {
            actions.push(action);
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
        #[cfg(target_os = "windows")]
        captures_session::set_win_shift_s_takeover_enabled(false);
        for (key, _) in &self.keys {
            if !intercepted(key) {
                let _ = self.manager.unregister(*key);
            }
        }
    }
}

#[cfg(target_os = "windows")]
static WIN_SHIFT_S: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn intercepted(key: &HotKey) -> bool {
    cfg!(target_os = "windows") && *key == "Super+Shift+S".parse::<HotKey>().unwrap()
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

#[cfg(target_os = "windows")]
pub use crate::windows::set_autostart;

#[cfg(target_os = "linux")]
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
