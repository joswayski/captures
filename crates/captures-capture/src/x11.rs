//! X11 window listing and display grabs from one connection.
//!
//! xcap 0.9.8's Linux `Window` opens a new X connection for every atom
//! lookup and property read, and its `z()` lists every window again, so
//! listing N windows opened roughly 20 connections per window before the
//! window selection or New Capture could appear.

use std::convert::Infallible;

use image::RgbaImage;
use x11rb::{
    connection::Connection,
    cookie::Cookie,
    protocol::{
        randr::ConnectionExt as _,
        xproto::{
            Atom, AtomEnum, ConnectionExt, GetGeometryReply, GetPropertyReply, ImageFormat,
            ImageOrder, TranslateCoordinatesReply, Window,
        },
    },
    rust_connection::RustConnection,
};

/// One window as xcap 0.9.8's Linux `Window` reports it. Fields xcap would
/// fail to read are `None` and drop the window, as `windows()` did.
pub(crate) struct WindowRecord {
    id: u32,
    title: Option<String>,
    app_name: Option<String>,
    geometry: Option<(i32, i32, u32, u32)>,
    z: i32,
    minimized: Option<bool>,
}

/// The accessors return `Result` like xcap's, so callers stay unchanged.
impl WindowRecord {
    pub(crate) fn id(&self) -> Result<u32, Infallible> {
        Ok(self.id)
    }
    pub(crate) fn title(&self) -> Result<String, ()> {
        self.title.clone().ok_or(())
    }
    pub(crate) fn app_name(&self) -> Result<String, ()> {
        self.app_name.clone().ok_or(())
    }
    pub(crate) fn x(&self) -> Result<i32, ()> {
        self.geometry.map(|(x, ..)| x).ok_or(())
    }
    pub(crate) fn y(&self) -> Result<i32, ()> {
        self.geometry.map(|(_, y, ..)| y).ok_or(())
    }
    pub(crate) fn width(&self) -> Result<u32, ()> {
        self.geometry.map(|(.., width, _)| width).ok_or(())
    }
    pub(crate) fn height(&self) -> Result<u32, ()> {
        self.geometry.map(|(.., height)| height).ok_or(())
    }
    pub(crate) fn z(&self) -> Result<i32, Infallible> {
        Ok(self.z)
    }
    pub(crate) fn is_minimized(&self) -> Result<bool, ()> {
        self.minimized.ok_or(())
    }
}

/// xcap 0.9.8's `Window::all()` and accessors over one connection, with the
/// requests for every window pipelined: `_NET_CLIENT_LIST_STACKING` of each
/// screen the pointer is on, topmost first; z counted from the bottom of
/// that list; minimized as `_NET_WM_STATE_HIDDEN` among the first 12 state
/// atoms; the root position from `TranslateCoordinates`; the title from
/// `_NET_WM_NAME`, then `WM_NAME` as COMPOUND_TEXT, then the parent's; the
/// app name as `WM_CLASS`'s class part. An atom xcap requires that the
/// server does not know fails the fields that need it, as in xcap.
pub(crate) fn windows() -> Result<Vec<WindowRecord>, String> {
    let (conn, _) = x11rb::connect(None).map_err(|error| error.to_string())?;
    let atom = |name: &[u8]| -> Option<Atom> {
        let atom = conn.intern_atom(true, name).ok()?.reply().ok()?.atom;
        (atom != u32::from(AtomEnum::NONE)).then_some(atom)
    };
    let stacking = atom(b"_NET_CLIENT_LIST_STACKING")
        .ok_or_else(|| "_NET_CLIENT_LIST_STACKING not supported".to_owned())?;
    let state = [
        atom(b"_NET_WM_STATE"),
        atom(b"_NET_WM_STATE_HIDDEN"),
        atom(b"_NET_WM_STATE_MAXIMIZED_VERT"),
        atom(b"_NET_WM_STATE_MAXIMIZED_HORZ"),
    ];
    let (net_wm_name, utf8) = (atom(b"_NET_WM_NAME"), atom(b"UTF8_STRING"));
    let compound_text = atom(b"COMPOUND_TEXT");

    let mut ids: Vec<Window> = Vec::new();
    for screen in &conn.setup().roots {
        let Ok(pointer) = conn
            .query_pointer(screen.root)
            .map_err(drop)
            .and_then(|cookie| cookie.reply().map_err(drop))
        else {
            continue;
        };
        if !pointer.same_screen {
            continue;
        }
        let Ok(list) = property(&conn, screen.root, stacking, AtomEnum::NONE.into(), 1024)
            .and_then(|cookie| cookie.reply().map_err(drop))
        else {
            continue;
        };
        ids.extend(list.value32().into_iter().flatten());
    }
    ids.reverse();

    // Send every window's requests before reading any reply.
    let pending: Vec<_> = ids
        .iter()
        .map(|&window| {
            let geometry = conn.get_geometry(window).ok();
            let wm_state = match state {
                [Some(wm_state), Some(_), Some(_), Some(_)] => {
                    property(&conn, window, wm_state, AtomEnum::ATOM.into(), 12).ok()
                }
                _ => None,
            };
            let name = match (net_wm_name, utf8) {
                (Some(name), Some(utf8)) => property(&conn, window, name, utf8, 1024).ok(),
                _ => None,
            };
            let class = property(
                &conn,
                window,
                AtomEnum::WM_CLASS.into(),
                AtomEnum::STRING.into(),
                1024,
            )
            .ok();
            (geometry, wm_state, name, class)
        })
        .collect();

    // Positions need each geometry reply first; send those requests together too.
    let pending: Vec<_> = pending
        .into_iter()
        .zip(&ids)
        .map(|((geometry, wm_state, name, class), &window)| {
            let geometry = geometry
                .and_then(|cookie| cookie.reply().ok())
                .and_then(|geometry| {
                    let cookie = conn
                        .translate_coordinates(window, geometry.root, geometry.x, geometry.y)
                        .ok()?;
                    Some((geometry, cookie))
                });
            (geometry, wm_state, name, class)
        })
        .collect();

    let count = ids.len();
    let hidden = state[1];
    let records = ids
        .into_iter()
        .zip(pending)
        .enumerate()
        .map(|(index, (window, (geometry, wm_state, name, class)))| {
            let geometry = geometry.and_then(|(geometry, cookie)| {
                let translated = cookie.reply().ok()?;
                Some(root_position(&geometry, &translated))
            });
            let minimized = wm_state
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| {
                    reply
                        .value32()
                        .is_some_and(|mut atoms| atoms.any(|atom| Some(atom) == hidden))
                });
            let title = name
                .and_then(|cookie| cookie.reply().ok())
                .and_then(|reply| {
                    let title = String::from_utf8_lossy(&reply.value).into_owned();
                    if title.is_empty() {
                        fallback_title(&conn, window, compound_text, net_wm_name.zip(utf8))
                    } else {
                        Some(title)
                    }
                });
            let app_name = class.and_then(|cookie| cookie.reply().ok()).map(|reply| {
                String::from_utf8_lossy(&reply.value)
                    .split('\u{0}')
                    .nth(1)
                    .unwrap_or("")
                    .to_owned()
            });
            WindowRecord {
                id: window,
                title,
                app_name,
                geometry,
                z: i32::try_from(count - 1 - index).unwrap_or(i32::MAX),
                minimized,
            }
        })
        .collect();
    Ok(records)
}

fn property(
    conn: &RustConnection,
    window: Window,
    property: Atom,
    kind: Atom,
    long_length: u32,
) -> Result<Cookie<'_, RustConnection, GetPropertyReply>, ()> {
    conn.get_property(false, window, property, kind, 0, long_length)
        .map_err(drop)
}

/// xcap's position: the root coordinates of the window's own geometry
/// origin, minus that origin (the window's border-box offset in its parent).
fn root_position(
    geometry: &GetGeometryReply,
    translated: &TranslateCoordinatesReply,
) -> (i32, i32, u32, u32) {
    (
        i32::from(translated.dst_x) - i32::from(geometry.x),
        i32::from(translated.dst_y) - i32::from(geometry.y),
        u32::from(geometry.width),
        u32::from(geometry.height),
    )
}

/// xcap's title fallback for a window without `_NET_WM_NAME`: `WM_NAME`
/// read as COMPOUND_TEXT, then the parent's title, recursively.
fn fallback_title(
    conn: &RustConnection,
    window: Window,
    compound_text: Option<Atom>,
    net_wm_name: Option<(Atom, Atom)>,
) -> Option<String> {
    let reply = property(conn, window, AtomEnum::WM_NAME.into(), compound_text?, 1024)
        .ok()?
        .reply()
        .ok()?;
    let title = String::from_utf8_lossy(&reply.value).into_owned();
    if !title.is_empty() {
        return Some(title);
    }
    let parent = conn
        .query_tree(window)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|tree| tree.parent);
    match parent {
        Some(parent) if parent != 0 => {
            let (name, utf8) = net_wm_name?;
            let reply = property(conn, parent, name, utf8, 1024)
                .ok()?
                .reply()
                .ok()?;
            let title = String::from_utf8_lossy(&reply.value).into_owned();
            if title.is_empty() {
                fallback_title(conn, parent, compound_text, net_wm_name)
            } else {
                Some(title)
            }
        }
        _ => Some(title),
    }
}

/// The values of one xcap `Monitor`, read once per listing instead of once
/// per window and comparison (each read opened its own X connections).
pub(crate) struct MonitorRecord {
    pub(crate) id: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_factor: f32,
}

impl MonitorRecord {
    /// `None` where any value fails, as the per-window reads then gave no
    /// overlap and no window could land on that monitor.
    pub(crate) fn read(monitor: &xcap::Monitor) -> Option<Self> {
        Some(Self {
            id: monitor.id().ok()?.to_string(),
            x: monitor.x().ok()?,
            y: monitor.y().ok()?,
            width: monitor.width().ok()?,
            height: monitor.height().ok()?,
            scale_factor: monitor.scale_factor().ok()?,
        })
    }
}

/// Grabs RandR outputs' pixels byte-identical to xcap 0.9.8's X11
/// `Monitor::capture_image`, over one connection kept for every grab. xcap
/// opens three connections per grab and converts each pixel through a
/// function pointer with bounds checks (about 14 ms at 1080p and 54 ms at
/// 4K under Xvfb); this reads the same `GetMonitors` rectangle and
/// `GetImage` and converts the reply in place.
pub struct X11OutputGrabber {
    conn: RustConnection,
    root: Window,
}

impl X11OutputGrabber {
    /// `None` on sessions xcap treats as Wayland (where xcap grabs through
    /// Wayland instead) or without an X server; callers then use xcap.
    pub fn connect() -> Option<Self> {
        let wayland = std::env::var_os("XDG_SESSION_TYPE")
            .is_some_and(|session| session == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| {
                display.to_string_lossy().to_lowercase().contains("wayland")
            });
        if wayland {
            return None;
        }
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        Some(Self { conn, root })
    }

    /// The output xcap identifies by `output` (its `Monitor::id`), or `None`
    /// for formats other than 32 bits per pixel and on any error.
    pub fn grab(&self, output: u32) -> Option<RgbaImage> {
        let conn = &self.conn;
        let monitor = conn
            .randr_get_monitors(self.root, true)
            .ok()?
            .reply()
            .ok()?
            .monitors
            .into_iter()
            .find(|monitor| monitor.outputs.contains(&output))?;
        let reply = conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                monitor.x,
                monitor.y,
                monitor.width,
                monitor.height,
                u32::MAX,
            )
            .ok()?
            .reply()
            .ok()?;
        let setup = conn.setup();
        let bits_per_pixel = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == reply.depth)?
            .bits_per_pixel;
        if !matches!(reply.depth, 24 | 32) || bits_per_pixel != 32 {
            return None;
        }
        let mut pixels = reply.data;
        // xcap reads the byte order from the bitmap bit order; keep it so the
        // pixels stay identical on every server.
        if setup.bitmap_format_bit_order == ImageOrder::LSB_FIRST {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
        } else {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        RgbaImage::from_raw(monitor.width.into(), monitor.height.into(), pixels)
    }
}
