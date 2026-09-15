//! GPUI 0.2.2 incorrectly forces server decorations for transparent popups when
//! the WM lacks _GTK_FRAME_EXTENTS. Repair only this process's notification
//! windows; ordinary editor/preferences windows retain normal WM behavior.
use super::Action;
use captures_capture::CaptureMode;
use ksni::blocking::TrayMethods;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};

pub use super::unix::Instance;
pub use super::unix::{private_directory, private_file};

pub struct Integration {
    _tray: Option<ksni::blocking::Handle<Tray>>,
    _popup_decorations: PopupDecorations,
}

impl Integration {
    pub fn new(sender: mpsc::Sender<Action>) -> anyhow::Result<Self> {
        let tray = match Tray(sender).spawn() {
            Ok(tray) => Some(tray),
            Err(error) => {
                eprintln!("Tray host unavailable: {error}");
                None
            }
        };
        Ok(Self {
            _tray: tray,
            _popup_decorations: PopupDecorations::watch()?,
        })
    }

    pub fn poll(&self) {}
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
        tray_actions()
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

fn tray_actions() -> [(&'static str, Action); 12] {
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
}

pub fn ensure_supported_session() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var_os("DISPLAY").is_some()
            && std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland"),
        "GPUI capture integration requires an X11 session; Wayland capture is not yet supported"
    );
    Ok(())
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        };
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable = executable
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    std::fs::create_dir_all(path.parent().expect("autostart path has a parent"))
        .map_err(|error| error.to_string())?;
    std::fs::write(
        path,
        format!(
            "[Desktop Entry]\nType=Application\nName=Captures\nExec=\"{executable}\" --background\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        ),
    )
    .map_err(|error| error.to_string())
}

pub fn copy_file(path: &Path) -> Result<(), String> {
    crate::preview::x11::copy_file_uri(crate::preview::file_uri(path))
        .map_err(|error| error.to_string())
}

pub fn reveal(path: &Path) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::process::Command::new("xdg-open")
        .arg(directory)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("show {}: {error}", path.display()))
}

pub fn exclude_from_capture(_: &str, _: bool) -> Result<(), String> {
    // X11 has no standard per-window capture exclusion. Capture paths must
    // explicitly omit app windows when the backend supports that operation.
    Ok(())
}

pub fn set_preview_input_region(
    rectangles: &[(f32, f32, f32, f32)],
    scale: f32,
    initial_origin: (f32, f32),
) -> Result<(), String> {
    let rectangles: Vec<_> = rectangles
        .iter()
        .map(|&(x, y, width, height)| crate::preview::geometry::Rect {
            x,
            y,
            width,
            height,
        })
        .collect();
    crate::preview::x11::set_input_region(&rectangles, scale, initial_origin)
        .map_err(|error| error.to_string())
}

pub fn clear_preview_input_region() {
    crate::preview::x11::clear_cached_window();
}

pub struct PopupDecorations(Arc<AtomicBool>);

impl PopupDecorations {
    pub fn watch() -> anyhow::Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let atom = |name: &[u8]| -> anyhow::Result<u32> {
            Ok(connection.intern_atom(false, name)?.reply()?.atom)
        };
        let pid = atom(b"_NET_WM_PID")?;
        let kind = atom(b"_NET_WM_WINDOW_TYPE")?;
        let popup = atom(b"_NET_WM_WINDOW_TYPE_NOTIFICATION")?;
        let hints = atom(b"_MOTIF_WM_HINTS")?;
        connection
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
            )?
            .check()?;
        connection.flush()?;
        let running = Arc::new(AtomicBool::new(true));
        let worker = running.clone();
        std::thread::spawn(move || {
            while worker.load(Ordering::Relaxed) {
                let window = match connection.poll_for_event() {
                    Ok(Some(Event::CreateNotify(event))) => {
                        let _ = connection.change_window_attributes(
                            event.window,
                            &ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE),
                        );
                        Some(event.window)
                    }
                    Ok(Some(Event::PropertyNotify(event)))
                        if [pid, kind, hints].contains(&event.atom) =>
                    {
                        Some(event.window)
                    }
                    Ok(Some(_)) => None,
                    Ok(None) => {
                        let _ = connection.flush();
                        std::thread::sleep(Duration::from_millis(10));
                        None
                    }
                    Err(_) => break,
                };
                if let Some(window) = window {
                    let values = |property| -> Option<Vec<u32>> {
                        connection
                            .get_property(false, window, property, AtomEnum::ANY, 0, 8)
                            .ok()?
                            .reply()
                            .ok()?
                            .value32()
                            .map(Iterator::collect)
                    };
                    if values(pid).as_deref() == Some(&[std::process::id()])
                        && values(kind).is_some_and(|types| types.contains(&popup))
                        && values(hints).is_some_and(|data| data.get(2) != Some(&0))
                    {
                        let _ = connection.change_property32(
                            PropMode::REPLACE,
                            window,
                            hints,
                            hints,
                            &[2, 0, 0, 0, 0],
                        );
                    }
                }
            }
        });
        Ok(Self(running))
    }
}

impl Drop for PopupDecorations {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// GPUI's initial bounds are only a WM placement hint. Position thin recording
/// guides after mapping, and keep their input regions empty.
pub fn position_guide(title: &str, x: i32, y: i32) -> anyhow::Result<()> {
    use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let name = connection
        .intern_atom(false, b"_NET_WM_NAME")?
        .reply()?
        .atom;
    let pid = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        let Ok(tree) = connection.query_tree(parent)?.reply() else {
            continue;
        };
        for window in tree.children {
            let Ok(property) = connection
                .get_property(false, window, name, AtomEnum::ANY, 0, 128)?
                .reply()
            else {
                continue;
            };
            if property.value == title.as_bytes()
                && connection
                    .get_property(false, window, pid, AtomEnum::CARDINAL, 0, 1)?
                    .reply()?
                    .value32()
                    .and_then(|mut values| values.next())
                    == Some(std::process::id())
            {
                connection.configure_window(window, &ConfigureWindowAux::new().x(x).y(y))?;
                connection.shape_rectangles(
                    SO::SET,
                    SK::INPUT,
                    ClipOrdering::UNSORTED,
                    window,
                    0,
                    0,
                    &[],
                )?;
                if parent != root {
                    connection.shape_rectangles(
                        SO::SET,
                        SK::INPUT,
                        ClipOrdering::UNSORTED,
                        parent,
                        0,
                        0,
                        &[],
                    )?;
                }
                connection.flush()?;
                return Ok(());
            }
            pending.push(window);
        }
    }
    // The recording may have been cancelled before the deferred placement ran.
    Ok(())
}
