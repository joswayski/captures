//! Linux desktop integration without a GTK event loop or webview.
#[path = "desktop/windows.rs"]
mod windows;
use crate::settings::Settings;
use captures_capture::CaptureMode;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use ksni::blocking::TrayMethods;
use std::{
    collections::HashSet,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::mpsc,
};
pub use windows::position_guide;
use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};

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

pub struct Desktop {
    manager: GlobalHotKeyManager,
    shortcuts: Vec<(HotKey, Action)>,
    escape: HotKey,
    escape_registered: bool,
    events: mpsc::Receiver<Action>,
    _tray: Option<ksni::blocking::Handle<Tray>>,
    _popup_decorations: windows::PopupDecorations,
}

struct Tray(mpsc::Sender<Action>);
impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "captures-gpui".into()
    }
    fn title(&self) -> String {
        "Captures".into()
    }
    fn icon_name(&self) -> String {
        "applets-screenshooter".into()
    }
    fn activate(&mut self, _: i32, _: i32) {
        let _ = self.0.send(Action::Capture(CaptureMode::Region, 0));
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        [
            ("New Capture", Action::Capture(CaptureMode::Region, 0)),
            ("Region screenshot", Action::Capture(CaptureMode::Region, 0)),
            ("Window screenshot", Action::Capture(CaptureMode::Window, 0)),
            (
                "Full screen screenshot",
                Action::Capture(CaptureMode::Display, 0),
            ),
            ("Record video", Action::Capture(CaptureMode::Region, 1)),
            ("Record GIF", Action::Capture(CaptureMode::Region, 2)),
            ("Show recording controls", Action::RestoreControls),
            ("Show mini previews", Action::Previews),
            ("Capture History", Action::History),
            ("Open image or recording…", Action::Open),
            ("Preferences", Action::Preferences),
            ("Quit Captures", Action::Quit),
        ]
        .into_iter()
        .map(|(label, action)| {
            ksni::menu::StandardItem {
                label: label.into(),
                activate: Box::new(move |tray: &mut Self| {
                    let _ = tray.0.send(action.clone());
                }),
                ..Default::default()
            }
            .into()
        })
        .collect()
    }
}

pub struct Instance {
    commands: mpsc::Receiver<Action>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    path: PathBuf,
    _lock: std::fs::File,
}
impl Instance {
    pub fn acquire(args: &[String]) -> anyhow::Result<Option<Self>> {
        Self::acquire_at(crate::settings::data_dir(), args)
    }

    fn acquire_at(directory: PathBuf, args: &[String]) -> anyhow::Result<Option<Self>> {
        std::fs::create_dir_all(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let path = directory.join("instance.sock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("instance.lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // The owning process may still be between locking and binding.
                for _ in 0..100 {
                    if let Ok(mut stream) = UnixStream::connect(&path) {
                        stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
                        stream.write_all(&serde_json::to_vec(args)?)?;
                        return Ok(None);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                anyhow::bail!("Captures is already running but its command channel is unavailable");
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let (send, commands) = mpsc::sync_channel(64);
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker = running.clone();
        std::thread::spawn(move || {
            while worker.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ =
                            stream.set_read_timeout(Some(std::time::Duration::from_millis(250)));
                        let mut bytes = vec![];
                        if Read::by_ref(&mut stream)
                            .take(64 * 1024 + 1)
                            .read_to_end(&mut bytes)
                            .is_ok()
                            && bytes.len() <= 64 * 1024
                            && let Ok(args) = serde_json::from_slice(&bytes)
                            && send.send(Action::Arguments(args)).is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Some(Self {
            commands,
            running,
            path,
            _lock: lock,
        }))
    }
    pub fn commands(&self) -> Vec<Action> {
        self.commands.try_iter().collect()
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Desktop {
    pub fn new() -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let popup_decorations = windows::PopupDecorations::watch()?;
        let (tx, events) = mpsc::channel();
        let tray = match Tray(tx).spawn() {
            Ok(tray) => Some(tray),
            Err(e) => {
                eprintln!("Tray host unavailable: {e}");
                None
            }
        };
        Ok(Self {
            manager,
            shortcuts: vec![],
            escape: "Escape".parse()?,
            escape_registered: false,
            events,
            _tray: tray,
            _popup_decorations: popup_decorations,
        })
    }
    pub fn replace_shortcuts(&mut self, s: &Settings) -> Result<(), String> {
        let bindings = [
            (
                &s.new_capture_shortcut,
                Action::Capture(CaptureMode::Region, 0),
            ),
            (&s.region_shortcut, Action::Capture(CaptureMode::Region, 0)),
            (&s.window_shortcut, Action::Capture(CaptureMode::Window, 0)),
            (
                &s.display_shortcut,
                Action::Capture(CaptureMode::Display, 0),
            ),
            (
                &s.recording.video_shortcut,
                Action::Capture(CaptureMode::Region, 1),
            ),
            (
                &s.recording.window_shortcut,
                Action::Capture(CaptureMode::Window, 1),
            ),
            (
                &s.recording.display_shortcut,
                Action::Capture(CaptureMode::Display, 1),
            ),
            (
                &s.recording.gif_shortcut,
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
                .map_err(|e| format!("Invalid shortcut {value}: {e}"))?;
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
            if let Err(e) = self.manager.register(*key) {
                for key in added {
                    let _ = self.manager.unregister(key);
                }
                return Err(format!(
                    "Cannot register shortcut {key:?}: {e}. Existing shortcuts are unchanged."
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
            Err(e) => eprintln!("Global Escape: {e}"),
        }
    }
    pub fn events(&self) -> Vec<Action> {
        let mut actions: Vec<_> = self.events.try_iter().collect();
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if self.escape_registered && event.id == self.escape.id() {
                actions.push(Action::Cancel);
                continue;
            }
            if let Some((_, action)) = self.shortcuts.iter().find(|(key, _)| key.id() == event.id) {
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

pub fn pointer() -> (i32, i32) {
    let Ok((connection, screen)) = x11rb::connect(None) else {
        return (0, 0);
    };
    connection
        .query_pointer(connection.setup().roots[screen].root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| (i32::from(reply.root_x), i32::from(reply.root_y)))
        .unwrap_or((0, 0))
}

pub fn login(enabled: bool) -> Result<(), String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    let path = config.join("autostart/captures-gpui.desktop");
    if !enabled {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let executable = executable
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(path,format!("[Desktop Entry]\nType=Application\nName=Captures GPUI\nExec=\"{executable}\" --background\nTerminal=false\nX-GNOME-Autostart-enabled=true\n")).map_err(|e|e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_forwards_exact_arguments_and_releases_lock() {
        let directory = tempfile::tempdir().unwrap();
        let first = Instance::acquire_at(directory.path().into(), &[])
            .unwrap()
            .unwrap();
        let args = vec!["--open".into(), "/tmp/a file with spaces.png".into()];
        assert!(
            Instance::acquire_at(directory.path().into(), &args)
                .unwrap()
                .is_none()
        );
        let received = first
            .commands
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(matches!(received,Action::Arguments(values) if values==args));
        drop(first);
        assert!(!directory.path().join("instance.sock").exists());
        assert!(
            Instance::acquire_at(directory.path().into(), &[])
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn partial_ipc_message_is_reassembled_without_reading_on_ui_thread() {
        let directory = tempfile::tempdir().unwrap();
        let instance = Instance::acquire_at(directory.path().into(), &[])
            .unwrap()
            .unwrap();
        let mut stream = UnixStream::connect(directory.path().join("instance.sock")).unwrap();
        stream.write_all(b"[\"--open\",\"/tmp/").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(instance.commands().is_empty());
        stream.write_all(b"asymmetric.png\"]").unwrap();
        drop(stream);
        let received = instance
            .commands
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(
            matches!(received,Action::Arguments(values) if values==["--open","/tmp/asymmetric.png"])
        );
    }
}
