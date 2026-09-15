//! Outbound XDND v3–5 source used by the GPUI X11 backend.
//!
//! This intentionally uses its own X connection and an otherwise invisible
//! source window.  In particular it neither asks GPUI for a window handle nor
//! replaces GPUI's implicit pointer grab.  `QueryPointer` still reports the
//! server's real pointer/button state while another client owns that grab.

use anyhow::{Context as _, Result, bail};
use std::{
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _,
            CreateWindowAux, EventMask, KeyButMask, PropMode, SelectionNotifyEvent,
            SelectionRequestEvent, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const VERSION: u32 = 5;
const POLL: Duration = Duration::from_millis(12);
const STATUS_WAIT: Duration = Duration::from_millis(800);
const FINISH_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct Atoms {
    aware: Atom,
    proxy: Atom,
    enter: Atom,
    position: Atom,
    status: Atom,
    leave: Atom,
    drop: Atom,
    finished: Atom,
    selection: Atom,
    action_copy: Atom,
    targets: Atom,
    uri_list: Atom,
    pid: Atom,
    atom: Atom,
}

impl Atoms {
    fn new(c: &RustConnection) -> Result<Self> {
        let names = [
            "XdndAware",
            "XdndProxy",
            "XdndEnter",
            "XdndPosition",
            "XdndStatus",
            "XdndLeave",
            "XdndDrop",
            "XdndFinished",
            "XdndSelection",
            "XdndActionCopy",
            "TARGETS",
            "text/uri-list",
            "_NET_WM_PID",
            "ATOM",
        ];
        let mut values = Vec::with_capacity(names.len());
        for name in names {
            values.push(c.intern_atom(false, name.as_bytes())?.reply()?.atom);
        }
        Ok(Self {
            aware: values[0],
            proxy: values[1],
            enter: values[2],
            position: values[3],
            status: values[4],
            leave: values[5],
            drop: values[6],
            finished: values[7],
            selection: values[8],
            action_copy: values[9],
            targets: values[10],
            uri_list: values[11],
            pid: values[12],
            atom: values[13],
        })
    }
}

struct Source {
    c: RustConnection,
    root: Window,
    window: Window,
    atoms: Atoms,
    uris: Vec<u8>,
    own_pid: u32,
    icon: Option<Window>,
}

/// Runs a synchronous, Copy-only native file drag.
/// Call this on a background thread after the GPUI drag gesture has begun.
/// Returns `true` only after the target accepts Copy and acknowledges
/// the drop with `XdndFinished`; all rejection and cancellation paths return
/// `false`.
pub fn run(paths: Vec<PathBuf>, icon_png: Vec<u8>) -> Result<bool> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        bail!("native XDND is unavailable on a native Wayland session");
    }
    if paths.is_empty() {
        bail!("XDND requires at least one source path");
    }
    let uris = uri_list(&paths)?;
    let (c, screen_num) = x11rb::connect(None).context("cannot connect to the X11 display")?;
    let root = c.setup().roots[screen_num].root;
    let window = c.generate_id()?;
    c.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        -1,
        -1,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?
    .check()?;
    let atoms = Atoms::new(&c)?;
    c.change_property32(
        PropMode::REPLACE,
        window,
        atoms.aware,
        AtomEnum::ATOM,
        &[VERSION],
    )?;
    c.change_property32(
        PropMode::REPLACE,
        window,
        atoms.pid,
        AtomEnum::CARDINAL,
        &[std::process::id()],
    )?;
    c.set_selection_owner(window, atoms.selection, CURRENT_TIME)?
        .check()?;
    c.flush()?;
    if c.get_selection_owner(atoms.selection)?.reply()?.owner != window {
        bail!("could not own XdndSelection");
    }

    let icon = drag_icon(&c, screen_num, &icon_png)?;
    let mut source = Source {
        c,
        root,
        window,
        atoms,
        uris,
        own_pid: std::process::id(),
        icon,
    };
    let result = source.drag();
    // Destroying the owner releases only XdndSelection (never CLIPBOARD).
    let _ = source.c.destroy_window(source.window);
    let _ = source.c.flush();
    result
}

/// A server-backed image with an empty input shape: it follows the pointer
/// without ever becoming the drop target or taking a pointer grab.
fn drag_icon(c: &RustConnection, screen: usize, png: &[u8]) -> Result<Option<Window>> {
    use x11rb::protocol::{
        shape::{ConnectionExt as _, SK, SO},
        xproto::{ClipOrdering, CreateGCAux, ImageFormat, ImageOrder, Rectangle},
    };
    if png.is_empty() {
        return Ok(None);
    }
    let screen = &c.setup().roots[screen];
    let format = c
        .setup()
        .pixmap_formats
        .iter()
        .find(|format| format.depth == screen.root_depth);
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|depth| &depth.visuals)
        .find(|visual| visual.visual_id == screen.root_visual);
    let Some(visual) = visual.filter(|_| format.is_some_and(|format| format.bits_per_pixel == 32))
    else {
        return Ok(None); // uncommon indexed/16-bit X servers still support the file drop
    };
    let image = image::load_from_memory(png)?
        .resize_exact(142, 80, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let component = |value: u8, mask: u32| -> u32 {
        if mask == 0 {
            0
        } else {
            (u32::from(value) * (mask >> mask.trailing_zeros()) / 255) << mask.trailing_zeros()
        }
    };
    let mut pixels = Vec::with_capacity(142 * 80 * 4);
    for pixel in image.pixels() {
        let value = component(pixel[0], visual.red_mask)
            | component(pixel[1], visual.green_mask)
            | component(pixel[2], visual.blue_mask);
        pixels.extend_from_slice(&if c.setup().image_byte_order == ImageOrder::LSB_FIRST {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    }
    let pixmap = c.generate_id()?;
    let gc = c.generate_id()?;
    c.create_pixmap(screen.root_depth, pixmap, screen.root, 142, 80)?
        .check()?;
    c.create_gc(gc, pixmap, &CreateGCAux::new())?.check()?;
    c.put_image(
        ImageFormat::Z_PIXMAP,
        pixmap,
        gc,
        142,
        80,
        0,
        0,
        0,
        screen.root_depth,
        &pixels,
    )?
    .check()?;
    let window = c.generate_id()?;
    c.create_window(
        screen.root_depth,
        window,
        screen.root,
        -160,
        -100,
        142,
        80,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixmap(pixmap),
    )?
    .check()?;
    c.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"Captures drag",
    )?;
    let opacity = c
        .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")?
        .reply()?
        .atom;
    c.change_property32(
        PropMode::REPLACE,
        window,
        opacity,
        AtomEnum::CARDINAL,
        &[(f64::from(u32::MAX) * 0.82) as u32],
    )?;
    c.shape_rectangles(
        SO::SET,
        SK::INPUT,
        ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &[],
    )?
    .check()?;
    let rounded = (0..80)
        .map(|y: i16| {
            let dy = (7.5 - f32::from(y)).max(f32::from(y) - 71.5).max(0.);
            let inset = (8. - (64. - dy * dy).sqrt()).ceil() as i16;
            Rectangle {
                x: inset,
                y,
                width: (142 - 2 * inset) as u16,
                height: 1,
            }
        })
        .collect::<Vec<_>>();
    c.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        ClipOrdering::Y_SORTED,
        window,
        0,
        0,
        &rounded,
    )?
    .check()?;
    c.map_window(window)?.check()?;
    c.free_gc(gc)?;
    c.free_pixmap(pixmap)?; // background retains its server-side reference
    c.flush()?;
    Ok(Some(window))
}

impl Source {
    fn drag(&mut self) -> Result<bool> {
        let initial = self.pointer()?;
        if !initial.2.contains(KeyButMask::BUTTON1) {
            return Ok(false); // released before the background worker started
        }
        let escape = self.escape_keycode()?;
        let mut current: Option<Target> = None;
        let mut requested_position = None;
        let mut pending: Option<Instant> = None;
        let mut accepted = false;
        let mut released_position = None;

        loop {
            let (x, y, buttons) = self.pointer()?;
            let first_release =
                released_position.is_none() && !buttons.contains(KeyButMask::BUTTON1);
            if first_release {
                released_position = Some((x, y));
            }
            let (x, y) = released_position.unwrap_or((x, y));
            if let Some(icon) = self.icon {
                self.c.configure_window(
                    icon,
                    &x11rb::protocol::xproto::ConfigureWindowAux::new()
                        .x(i32::from(x) + 12)
                        .y(i32::from(y) + 12),
                )?;
                self.c.flush()?;
            }
            if self.escape_down(escape)? {
                if let Some(target) = current {
                    self.send(target.send_to, self.atoms.leave, [self.window, 0, 0, 0, 0])?;
                }
                return Ok(false);
            }
            // Mouse-up fixes the destination even if a delayed status arrives
            // after the user moves the pointer somewhere else.
            let next = if released_position.is_some() && !first_release {
                current
            } else {
                self.target_at()?
            };
            if next != current {
                if let Some(old) = current {
                    self.send(old.send_to, self.atoms.leave, [self.window, 0, 0, 0, 0])?;
                }
                current = next;
                accepted = false;
                requested_position = None;
                pending = None;
                if let Some(target) = current {
                    // URI is in field 2, so the type-list property bit is clear.
                    self.send(
                        target.send_to,
                        self.atoms.enter,
                        [self.window, target.version << 24, self.atoms.uri_list, 0, 0],
                    )?;
                }
            }
            if let Some(target) = current {
                if let Some(status) = self.read_status(target)? {
                    accepted = status;
                    pending = None;
                }
                // XDND has no per-position response serial. Keep just one
                // outstanding request, and never drop using an acceptance for
                // a previous coordinate (including the last mouse-up move).
                if requested_position != Some((x, y)) && pending.is_none() {
                    self.send(
                        target.send_to,
                        self.atoms.position,
                        [
                            self.window,
                            0,
                            pack_xy(x, y),
                            CURRENT_TIME,
                            self.atoms.action_copy,
                        ],
                    )?;
                    requested_position = Some((x, y));
                    pending = Some(Instant::now());
                    accepted = false;
                }
                if released_position.is_some() {
                    if pending.is_some_and(|sent| sent.elapsed() < STATUS_WAIT) {
                        thread::sleep(POLL);
                        continue;
                    }
                    if pending.is_some() || !accepted {
                        self.send(target.send_to, self.atoms.leave, [self.window, 0, 0, 0, 0])?;
                        return Ok(false);
                    }
                    self.send(
                        target.send_to,
                        self.atoms.drop,
                        [self.window, 0, CURRENT_TIME, 0, 0],
                    )?;
                    return self.wait_finished(target);
                }
            } else if released_position.is_some() {
                return Ok(false);
            }
            thread::sleep(POLL);
        }
    }

    fn pointer(&self) -> Result<(i16, i16, KeyButMask)> {
        let p = self.c.query_pointer(self.root)?.reply()?;
        Ok((p.root_x, p.root_y, p.mask))
    }

    fn escape_keycode(&self) -> Result<Option<u8>> {
        let setup = self.c.setup();
        let first = setup.min_keycode;
        let count = setup.max_keycode - first + 1;
        let map = self.c.get_keyboard_mapping(first, count)?.reply()?;
        Ok(map
            .keysyms
            .chunks(map.keysyms_per_keycode as usize)
            .position(|syms| syms.contains(&0xff1b))
            .map(|index| first + index as u8))
    }

    fn escape_down(&self, keycode: Option<u8>) -> Result<bool> {
        let Some(k) = keycode else { return Ok(false) };
        let keys = self.c.query_keymap()?.reply()?.keys;
        Ok(keys[(k / 8) as usize] & (1 << (k % 8)) != 0)
    }

    fn target_at(&self) -> Result<Option<Target>> {
        let mut window = self.c.query_pointer(self.root)?.reply()?.child;
        if window == 0 || window == self.window {
            return Ok(None);
        }
        loop {
            if let Some(version) = self.cardinal(window, self.atoms.aware)?
                && version >= 3
                && self.window_pid(window)? != Some(self.own_pid)
            {
                let send_to = self.valid_proxy(window)?.unwrap_or(window);
                return Ok(Some(Target {
                    logical: window,
                    send_to,
                    version: version.min(VERSION),
                }));
            }
            let p = match self.c.query_pointer(window)?.reply() {
                Ok(p) => p,
                Err(_) => return Ok(None),
            };
            if p.child == 0 || p.child == window {
                return Ok(None);
            }
            window = p.child;
        }
    }

    fn valid_proxy(&self, window: Window) -> Result<Option<Window>> {
        let Some(proxy) = self.cardinal(window, self.atoms.proxy)? else {
            return Ok(None);
        };
        if proxy == 0 {
            return Ok(None);
        }
        Ok((self.cardinal(proxy, self.atoms.proxy)? == Some(proxy)).then_some(proxy))
    }

    fn window_pid(&self, mut window: Window) -> Result<Option<u32>> {
        loop {
            if let Some(pid) = self.cardinal(window, self.atoms.pid)? {
                return Ok(Some(pid));
            }
            let tree = match self.c.query_tree(window)?.reply() {
                Ok(t) => t,
                Err(_) => return Ok(None),
            };
            if tree.parent == 0 || tree.parent == window || tree.parent == self.root {
                return Ok(None);
            }
            window = tree.parent;
        }
    }

    fn cardinal(&self, window: Window, property: Atom) -> Result<Option<u32>> {
        let reply = match self
            .c
            .get_property(false, window, property, AtomEnum::ANY, 0, 1)?
            .reply()
        {
            Ok(reply) => reply,
            Err(_) => return Ok(None),
        };
        Ok(reply.value32().and_then(|mut values| values.next()))
    }

    fn send(&self, target: Window, kind: Atom, data: [u32; 5]) -> Result<()> {
        let event = ClientMessageEvent::new(32, target, kind, ClientMessageData::from(data));
        self.c
            .send_event(false, target, EventMask::NO_EVENT, event)?
            .check()?;
        self.c.flush()?;
        Ok(())
    }

    fn read_status(&mut self, target: Target) -> Result<Option<bool>> {
        let mut accepted = None;
        while let Some(event) = self.c.poll_for_event()? {
            match event {
                Event::ClientMessage(e) if e.type_ == self.atoms.status => {
                    let d = e.data.as_data32();
                    // Reject stale/foreign replies and anything but Copy.
                    if d[0] == target.logical || d[0] == target.send_to {
                        accepted = Some(status_accepts(d, target, self.atoms.action_copy));
                    }
                }
                Event::SelectionRequest(e) => self.selection_request(e)?,
                _ => {}
            }
        }
        Ok(accepted)
    }

    fn selection_request(&self, e: SelectionRequestEvent) -> Result<()> {
        let mut property = AtomEnum::NONE.into();
        if e.selection == self.atoms.selection {
            let destination = if e.property == u32::from(AtomEnum::NONE) {
                e.target
            } else {
                e.property
            };
            if e.target == self.atoms.targets {
                self.c.change_property32(
                    PropMode::REPLACE,
                    e.requestor,
                    destination,
                    self.atoms.atom,
                    &[self.atoms.targets, self.atoms.uri_list],
                )?;
                property = destination;
            } else if e.target == self.atoms.uri_list {
                self.c.change_property8(
                    PropMode::REPLACE,
                    e.requestor,
                    destination,
                    self.atoms.uri_list,
                    &self.uris,
                )?;
                property = destination;
            }
        }
        let notify = SelectionNotifyEvent {
            response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property,
        };
        self.c
            .send_event(false, e.requestor, EventMask::NO_EVENT, notify)?
            .check()?;
        self.c.flush()?;
        Ok(())
    }

    fn wait_finished(&mut self, target: Target) -> Result<bool> {
        let deadline = Instant::now() + FINISH_WAIT;
        while Instant::now() < deadline {
            while let Some(event) = self.c.poll_for_event()? {
                match event {
                    Event::SelectionRequest(e) => self.selection_request(e)?,
                    Event::ClientMessage(e) if e.type_ == self.atoms.finished => {
                        let d = e.data.as_data32();
                        if let Some(success) = finished_result(d, target, self.atoms.action_copy) {
                            return Ok(success);
                        }
                    }
                    _ => {}
                }
            }
            thread::sleep(POLL);
        }
        Ok(false)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Target {
    logical: Window,
    send_to: Window,
    version: u32,
}

fn finished_result(data: [u32; 5], target: Target, copy: Atom) -> Option<bool> {
    (data[0] == target.logical || data[0] == target.send_to)
        .then_some(target.version < 5 || (data[1] & 1 != 0 && data[2] == copy))
}

fn pack_xy(x: i16, y: i16) -> u32 {
    ((x as u16 as u32) << 16) | y as u16 as u32
}

fn status_accepts(data: [u32; 5], target: Target, copy: Atom) -> bool {
    (data[0] == target.logical || data[0] == target.send_to) && data[1] & 1 != 0 && data[4] == copy
}

fn uri_list(paths: &[PathBuf]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for path in paths {
        let path = path
            .canonicalize()
            .with_context(|| format!("drag source does not exist: {}", path.display()))?;
        if !path.is_file() {
            bail!("drag source is not a file: {}", path.display());
        }
        output.extend_from_slice(b"file://");
        for &byte in path.as_os_str().as_bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
                output.push(byte);
            } else {
                output.extend_from_slice(format!("%{byte:02X}").as_bytes());
            }
        }
        output.extend_from_slice(b"\r\n");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_encoding_handles_spaces_hash_and_unicode_without_touching_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("space # café.png");
        std::fs::write(&path, b"preserved").unwrap();
        let list = String::from_utf8(uri_list(std::slice::from_ref(&path)).unwrap()).unwrap();
        assert!(list.starts_with("file:///"));
        assert!(list.ends_with("space%20%23%20caf%C3%A9.png\r\n"));
        assert_eq!(std::fs::read(path).unwrap(), b"preserved");
    }

    #[test]
    fn coordinates_preserve_signed_root_bit_patterns() {
        assert_eq!(pack_xy(-2, 513), 0xfffe_0201);
    }

    #[test]
    fn target_identity_distinguishes_proxy_and_logical_window() {
        let target = Target {
            logical: 7,
            send_to: 9,
            version: 5,
        };
        assert_ne!(target.logical, target.send_to);
    }

    #[test]
    fn status_rejects_foreign_stale_and_move_responses() {
        let target = Target {
            logical: 7,
            send_to: 9,
            version: 5,
        };
        let copy = 42;
        assert!(status_accepts([7, 1, 0, 0, copy], target, copy));
        assert!(status_accepts([9, 1, 0, 0, copy], target, copy));
        assert!(!status_accepts([8, 1, 0, 0, copy], target, copy));
        assert!(!status_accepts([7, 0, 0, 0, copy], target, copy));
        assert!(!status_accepts([7, 1, 0, 0, 43], target, copy));
    }

    #[test]
    fn finished_rejection_is_terminal_and_old_targets_have_no_success_flags() {
        let target = Target {
            logical: 7,
            send_to: 9,
            version: 5,
        };
        assert_eq!(finished_result([7, 1, 42, 0, 0], target, 42), Some(true));
        assert_eq!(finished_result([7, 0, 0, 0, 0], target, 42), Some(false));
        assert_eq!(finished_result([9, 1, 43, 0, 0], target, 42), Some(false));
        assert_eq!(finished_result([8, 1, 42, 0, 0], target, 42), None);
        assert_eq!(
            finished_result(
                [7, 0, 0, 0, 0],
                Target {
                    version: 3,
                    ..target
                },
                42
            ),
            Some(true)
        );
    }
}
