//! X11 affordances GPUI 0.2.2 does not expose yet.
//!
//! Transparent popup windows are rectangular to X11 unless their Shape input
//! region is set explicitly. Updating that region makes every transparent gap
//! pass through to the desktop while cards and controls remain interactive.

use super::geometry::Rect;
use anyhow::{Context as _, Result};
use std::sync::{Mutex, OnceLock};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        shape::{ConnectionExt as _, SK, SO},
        xproto::{
            Atom, AtomEnum, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux,
            EventMask, PropMode, Rectangle, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
            SelectionRequestEvent, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const TITLE: &str = "Captures GPUI Previews";

struct ShapeConnection {
    connection: RustConnection,
    root: Window,
    window: Option<Window>,
    positioned: bool,
}

static SHAPE: OnceLock<Mutex<Option<ShapeConnection>>> = OnceLock::new();

fn connection() -> &'static Mutex<Option<ShapeConnection>> {
    SHAPE.get_or_init(|| {
        let value = x11rb::connect(None).ok().map(|(connection, screen)| {
            let root = connection.setup().roots[screen].root;
            ShapeConnection {
                connection,
                root,
                window: None,
                positioned: false,
            }
        });
        Mutex::new(value)
    })
}

fn property_string(connection: &RustConnection, window: Window, atom: u32) -> Option<String> {
    let reply = connection
        .get_property(false, window, atom, AtomEnum::ANY, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    if reply.type_ == 0 || reply.value.is_empty() {
        return None;
    }
    String::from_utf8(reply.value)
        .ok()
        .map(|value| value.trim_end_matches('\0').to_owned())
}

fn find_titled_window(connection: &RustConnection, root: Window) -> Option<Window> {
    let net_name = connection
        .intern_atom(false, b"_NET_WM_NAME")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let wm_name = AtomEnum::WM_NAME.into();
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        let Ok(cookie) = connection.query_tree(parent) else {
            continue;
        };
        let Ok(tree) = cookie.reply() else {
            continue;
        };
        for child in tree.children.into_iter().rev() {
            let title = property_string(connection, child, net_name)
                .or_else(|| property_string(connection, child, wm_name));
            if title.as_deref() == Some(TITLE) {
                return Some(child);
            }
            pending.push(child);
        }
    }
    None
}

fn rectangle(rect: Rect, scale: f32) -> Rectangle {
    let clamp_i16 = |value: f32| value.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    let clamp_u16 = |value: f32| value.round().clamp(0.0, u16::MAX as f32) as u16;
    Rectangle {
        x: clamp_i16(rect.x * scale),
        y: clamp_i16(rect.y * scale),
        width: clamp_u16(rect.width * scale),
        height: clamp_u16(rect.height * scale),
    }
}

pub fn set_input_region(rectangles: &[Rect], scale: f32, initial_origin: (f32, f32)) -> Result<()> {
    if std::env::var_os("DISPLAY").is_none() {
        return Ok(());
    }
    let mut guard = connection().lock().expect("X11 shape connection poisoned");
    let Some(state) = guard.as_mut() else {
        return Ok(());
    };
    if state.window.is_none() {
        state.window = find_titled_window(&state.connection, state.root);
    }
    let Some(window) = state.window else {
        return Ok(());
    };
    let scale = scale.max(0.25);
    if !state.positioned {
        let motif_hints = state
            .connection
            .intern_atom(false, b"_MOTIF_WM_HINTS")?
            .reply()?
            .atom;
        // GPUI 0.2.2 falls back to server decorations on some X11 compositors.
        // The preview is intentionally chrome-free, so repeat the standard
        // Motif no-decoration hint directly on the client window.
        state.connection.change_property32(
            PropMode::REPLACE,
            window,
            motif_hints,
            motif_hints,
            &[2, 0, 0, 0, 0],
        )?;
        state.connection.configure_window(
            window,
            &ConfigureWindowAux::new()
                .x((initial_origin.0 * scale).round() as i32)
                .y((initial_origin.1 * scale).round() as i32),
        )?;
        state.positioned = true;
    }
    let rectangles: Vec<_> = rectangles
        .iter()
        .copied()
        .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
        .map(|rect| rectangle(rect, scale))
        .collect();
    let parent = state
        .connection
        .query_tree(window)?
        .reply()
        .map(|tree| tree.parent)
        .unwrap_or(state.root);
    for shaped_window in [window, parent]
        .into_iter()
        .filter(|candidate| *candidate != state.root)
    {
        state
            .connection
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                shaped_window,
                0,
                0,
                &rectangles,
            )
            .context("set preview X11 input shape")?;
    }
    state
        .connection
        .flush()
        .context("flush preview X11 input shape")?;
    Ok(())
}

pub fn clear_cached_window() {
    if let Ok(mut guard) = connection().lock()
        && let Some(state) = guard.as_mut()
    {
        state.window = None;
        state.positioned = false;
    }
}

/// Own CLIPBOARD and serve a native `text/uri-list` until another application
/// replaces it. GPUI 0.2.2 only publishes text/plain for ClipboardItem::String.
pub fn copy_file_uri(uri: String) -> Result<()> {
    let (connection, screen) = x11rb::connect(None).context("connect X11 clipboard")?;
    let root = connection.setup().roots[screen].root;
    let window = connection.generate_id()?;
    connection.create_window(
        0,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    let clipboard = atom(&connection, b"CLIPBOARD")?;
    let targets = atom(&connection, b"TARGETS")?;
    let uri_list = atom(&connection, b"text/uri-list")?;
    let utf8 = atom(&connection, b"UTF8_STRING")?;
    let text_plain = atom(&connection, b"text/plain;charset=utf-8")?;
    let gnome_files = atom(&connection, b"x-special/gnome-copied-files")?;
    connection.set_selection_owner(window, clipboard, x11rb::CURRENT_TIME)?;
    connection.flush()?;
    anyhow::ensure!(
        connection.get_selection_owner(clipboard)?.reply()?.owner == window,
        "could not own X11 clipboard"
    );
    std::thread::Builder::new()
        .name("captures-file-clipboard".into())
        .spawn(move || {
            let offered = [targets, uri_list, utf8, text_plain, gnome_files];
            while let Ok(event) = connection.wait_for_event() {
                match event {
                    Event::SelectionRequest(request) if request.selection == clipboard => {
                        let _ = serve_selection(
                            &connection,
                            request,
                            targets,
                            uri_list,
                            utf8,
                            text_plain,
                            gnome_files,
                            &offered,
                            &uri,
                        );
                    }
                    Event::SelectionClear(event) if event.selection == clipboard => break,
                    _ => {}
                }
            }
            let _ = connection.destroy_window(window);
            let _ = connection.flush();
        })
        .context("start X11 clipboard owner")?;
    Ok(())
}

fn atom(connection: &RustConnection, name: &[u8]) -> Result<Atom> {
    Ok(connection.intern_atom(false, name)?.reply()?.atom)
}

#[allow(clippy::too_many_arguments)]
fn serve_selection(
    connection: &RustConnection,
    request: SelectionRequestEvent,
    targets: Atom,
    uri_list: Atom,
    utf8: Atom,
    text_plain: Atom,
    gnome_files: Atom,
    offered: &[Atom],
    uri: &str,
) -> Result<()> {
    let property = if request.property == u32::from(AtomEnum::NONE) {
        request.target
    } else {
        request.property
    };
    let success = if request.target == targets {
        connection.change_property32(
            PropMode::REPLACE,
            request.requestor,
            property,
            AtomEnum::ATOM,
            offered,
        )?;
        true
    } else if matches!(request.target, target if target == uri_list || target == utf8 || target == text_plain)
    {
        connection.change_property8(
            PropMode::REPLACE,
            request.requestor,
            property,
            request.target,
            format!("{uri}\r\n").as_bytes(),
        )?;
        true
    } else if request.target == gnome_files {
        connection.change_property8(
            PropMode::REPLACE,
            request.requestor,
            property,
            request.target,
            format!("copy\n{uri}\n").as_bytes(),
        )?;
        true
    } else {
        false
    };
    connection.send_event(
        false,
        request.requestor,
        EventMask::NO_EVENT,
        SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: request.sequence,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if success {
                property
            } else {
                u32::from(AtomEnum::NONE)
            },
        },
    )?;
    connection.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_and_clamps_input_rectangles() {
        let scaled = rectangle(
            Rect {
                x: 28.25,
                y: 52.5,
                width: 284.0,
                height: 160.0,
            },
            2.0,
        );
        assert_eq!(
            (scaled.x, scaled.y, scaled.width, scaled.height),
            (57, 105, 568, 320)
        );
        let empty = rectangle(
            Rect {
                x: -40_000.0,
                y: 0.0,
                width: -2.0,
                height: 90_000.0,
            },
            1.0,
        );
        assert_eq!(empty.x, i16::MIN);
        assert_eq!(empty.width, 0);
        assert_eq!(empty.height, u16::MAX);
    }

    #[test]
    fn live_shape_test() {
        if std::env::var_os("CAPTURES_TEST_LIVE_X11").is_none() {
            return;
        }
        let (connection, screen) = x11rb::connect(None).expect("connect live X11 test");
        let root = connection.setup().roots[screen].root;
        let window = find_titled_window(&connection, root).expect("find live GPUI preview");
        let reply = connection
            .shape_get_rectangles(window, SK::INPUT)
            .expect("query preview input shape")
            .reply()
            .expect("receive preview input shape");
        assert!(
            (1..=5).contains(&reply.rectangles.len()),
            "expected card/control input islands, got {:?}",
            reply.rectangles
        );
        assert!(reply.rectangles.iter().all(|rect| rect.width < 340));
    }

    #[test]
    fn live_file_clipboard_serves_uri_list() {
        if std::env::var_os("CAPTURES_TEST_LIVE_X11").is_none() {
            return;
        }
        let expected = "file:///tmp/Capture%20One.mp4";
        copy_file_uri(expected.into()).expect("own file clipboard");
        let (connection, screen) = x11rb::connect(None).expect("connect clipboard requester");
        let root = connection.setup().roots[screen].root;
        let requestor = connection.generate_id().unwrap();
        connection
            .create_window(
                0,
                requestor,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap();
        let clipboard = atom(&connection, b"CLIPBOARD").unwrap();
        let uri_list = atom(&connection, b"text/uri-list").unwrap();
        let property = atom(&connection, b"CAPTURES_TEST_URI").unwrap();
        connection
            .convert_selection(
                requestor,
                clipboard,
                uri_list,
                property,
                x11rb::CURRENT_TIME,
            )
            .unwrap();
        connection.flush().unwrap();
        loop {
            if let Event::SelectionNotify(event) = connection.wait_for_event().unwrap() {
                assert_eq!(event.property, property);
                break;
            }
        }
        let reply = connection
            .get_property(false, requestor, property, uri_list, 0, 4_096)
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(
            String::from_utf8(reply.value).unwrap(),
            format!("{expected}\r\n")
        );
    }
}
