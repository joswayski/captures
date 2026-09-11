//! GPUI 0.2.2 incorrectly forces server decorations for transparent popups when
//! the WM lacks _GTK_FRAME_EXTENTS. Repair only this process's notification
//! windows; ordinary editor/preferences windows retain normal WM behavior.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};

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
