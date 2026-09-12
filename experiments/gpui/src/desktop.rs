//! Native desktop integration shared by the GPUI frontend.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod tray;
#[cfg(unix)]
mod unix;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

use crate::settings::Settings;
use captures_capture::CaptureMode;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Mutex, OnceLock, mpsc},
};

pub use platform::Instance;

#[derive(Clone, Debug)]
pub enum Action {
    Capture(CaptureMode, u32),
    Preferences,
    History,
    Open,
    Previews,
    RestoreControls,
    Quit,
    Cancel,
    Arguments(Vec<String>),
}

static NATIVE_EVENTS: OnceLock<Mutex<Vec<Action>>> = OnceLock::new();

pub fn open_urls(urls: Vec<String>) {
    let paths: Vec<_> = urls
        .into_iter()
        .filter_map(|value| url::Url::parse(&value).ok()?.to_file_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    if paths.is_empty() {
        return;
    }
    let mut args = vec!["--open".to_owned()];
    args.extend(paths);
    if let Ok(mut events) = NATIVE_EVENTS.get_or_init(Default::default).lock() {
        events.push(Action::Arguments(args));
    }
}

pub(super) fn take_native_events() -> Vec<Action> {
    NATIVE_EVENTS
        .get_or_init(Default::default)
        .lock()
        .map(|mut events| events.drain(..).collect())
        .unwrap_or_default()
}

pub struct Desktop {
    manager: GlobalHotKeyManager,
    shortcuts: Vec<(HotKey, Action)>,
    escape: HotKey,
    escape_registered: bool,
    events: mpsc::Receiver<Action>,
    _platform: platform::Integration,
}

impl Desktop {
    pub fn new() -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let (sender, events) = mpsc::channel();
        let integration = platform::Integration::new(sender)?;
        Ok(Self {
            manager,
            shortcuts: vec![],
            escape: "Escape".parse()?,
            escape_registered: false,
            events,
            _platform: integration,
        })
    }

    pub fn replace_shortcuts(&mut self, settings: &Settings) -> Result<(), String> {
        let bindings = [
            (
                &settings.new_capture_shortcut,
                Action::Capture(CaptureMode::Region, 0),
            ),
            (
                &settings.region_shortcut,
                Action::Capture(CaptureMode::Region, 0),
            ),
            (
                &settings.window_shortcut,
                Action::Capture(CaptureMode::Window, 0),
            ),
            (
                &settings.display_shortcut,
                Action::Capture(CaptureMode::Display, 0),
            ),
            (
                &settings.recording.video_shortcut,
                Action::Capture(CaptureMode::Region, 1),
            ),
            (
                &settings.recording.window_shortcut,
                Action::Capture(CaptureMode::Window, 1),
            ),
            (
                &settings.recording.display_shortcut,
                Action::Capture(CaptureMode::Display, 1),
            ),
            (
                &settings.recording.gif_shortcut,
                Action::Capture(CaptureMode::Region, 2),
            ),
        ];
        let mut desired = vec![];
        let mut ids = HashSet::new();
        for (value, action) in bindings {
            if value.trim().is_empty() {
                continue;
            }
            let key: HotKey = value
                .parse()
                .map_err(|error| format!("Invalid shortcut {value}: {error}"))?;
            if !ids.insert(key.id()) {
                return Err(format!("Shortcut {value} is assigned more than once"));
            }
            desired.push((key, action));
        }

        let mut added = vec![];
        for (key, _) in &desired {
            if self.shortcuts.iter().any(|(old, _)| old == key) {
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
        for (key, _) in &self.shortcuts {
            if !desired.iter().any(|(new, _)| new == key) {
                self.manager.unregister(*key).map_err(|e| e.to_string())?;
            }
        }
        self.shortcuts = desired;
        Ok(())
    }

    pub fn escape(&mut self, enabled: bool) {
        if enabled == self.escape_registered {
            return;
        }
        let result = if enabled {
            self.manager.register(self.escape)
        } else {
            self.manager.unregister(self.escape)
        };
        match result {
            Ok(()) => self.escape_registered = enabled,
            Err(error) => eprintln!("Global Escape: {error}"),
        }
    }

    pub fn events(&self) -> Vec<Action> {
        self._platform.poll();
        let mut actions: Vec<_> = self.events.try_iter().collect();
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if self.escape_registered && event.id == self.escape.id() {
                actions.push(Action::Cancel);
            } else if let Some((_, action)) =
                self.shortcuts.iter().find(|(key, _)| key.id() == event.id)
            {
                actions.push(action.clone());
            }
        }
        actions
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        for (key, _) in &self.shortcuts {
            let _ = self.manager.unregister(*key);
        }
        if self.escape_registered {
            let _ = self.manager.unregister(self.escape);
        }
    }
}

pub fn ensure_supported_session() -> anyhow::Result<()> {
    platform::ensure_supported_session()
}

pub fn pointer() -> (i32, i32) {
    platform::pointer()
}

pub fn login(enabled: bool) -> Result<(), String> {
    platform::login(enabled)
}

pub fn position_guide(title: &str, x: i32, y: i32) -> anyhow::Result<()> {
    platform::position_guide(title, x, y)
}

pub fn copy_file(path: &Path) -> Result<(), String> {
    platform::copy_file(path)
}

pub fn reveal(path: &Path) -> Result<(), String> {
    platform::reveal(path)
}

#[allow(dead_code)]
pub fn private_directory(path: &Path) -> std::io::Result<()> {
    platform::private_directory(path)
}

#[allow(dead_code)]
pub fn private_file(path: &Path) -> std::io::Result<()> {
    platform::private_file(path)
}

pub fn exclude_from_capture(title: &str, excluded: bool) -> Result<(), String> {
    platform::exclude_from_capture(title, excluded)
}

pub fn set_preview_input_region(
    rectangles: &[(f32, f32, f32, f32)],
    scale: f32,
    initial_origin: (f32, f32),
) -> Result<(), String> {
    platform::set_preview_input_region(rectangles, scale, initial_origin)
}

pub fn clear_preview_input_region() {
    platform::clear_preview_input_region();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_open_events_accept_file_urls_and_reject_other_schemes() {
        let _ = take_native_events();
        let path = std::env::temp_dir().join("a file.png");
        open_urls(vec![
            "https://captur.es/not-a-local-file.png".into(),
            url::Url::from_file_path(&path).unwrap().into(),
        ]);
        let events = take_native_events();
        assert!(
            matches!(events.as_slice(), [Action::Arguments(args)] if args == &["--open", &path.to_string_lossy()])
        );
    }
}
