//! X11 stack placement. The preview window takes shipping's work-area corner
//! (`preview::thumbnail_geometry`) and, as Tauri's thumbnail window, stays
//! above other windows on every workspace and out of the taskbar and pager.
//! GTK4 has no move API, so the X window is configured directly. Wayland
//! compositors place toplevels themselves. The tray notices reuse the
//! placer to sit in the tray corner.
use captures_app::preview::{self, ThumbnailMonitorBounds, ThumbnailStackOrigin};
use captures_app::tray_notice::LogicalRect;
use captures_settings::MiniPreviewPlacement;
use gdk4_x11::{X11Display, X11Monitor, X11Surface};
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::os::fd::AsRawFd;
use std::rc::Rc;

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt, EventMask, PropMode, Property,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub struct Placer {
    connection: RustConnection,
    root: u32,
    wm_state: Atom,
    net_wm_state: Atom,
    above: Atom,
    sticky: Atom,
    desktop: Atom,
    managed: RefCell<Option<Box<dyn Fn()>>>,
}

impl Placer {
    /// `None` off X11 (Wayland, Broadway) or when the display is unreachable.
    pub fn new(display: &gdk::Display) -> Option<Placer> {
        display.downcast_ref::<X11Display>()?;
        let (connection, screen) = x11rb::connect(Some(&display.name())).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        let atom = |name: &[u8]| Some(connection.intern_atom(false, name).ok()?.reply().ok()?.atom);
        Some(Placer {
            wm_state: atom(b"WM_STATE")?,
            net_wm_state: atom(b"_NET_WM_STATE")?,
            above: atom(b"_NET_WM_STATE_ABOVE")?,
            sticky: atom(b"_NET_WM_STATE_STICKY")?,
            desktop: atom(b"_NET_WM_DESKTOP")?,
            connection,
            root,
            managed: RefCell::default(),
        })
    }

    /// Report the window manager taking `surface` on: ICCCM's `WM_STATE`
    /// appears each time it manages the window. GTK marks a surface mapped
    /// before that, when an EWMH request would still be dropped.
    pub fn watch_managed(self: &Rc<Self>, surface: &X11Surface, managed: impl Fn() + 'static) {
        *self.managed.borrow_mut() = Some(Box::new(managed));
        let events = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        let _ = self
            .connection
            .change_window_attributes(xid(surface), &events);
        let _ = self.connection.flush();
        let fd = self.connection.stream().as_raw_fd();
        let placer = Rc::downgrade(self);
        glib::source::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
            let Some(placer) = placer.upgrade() else {
                return glib::ControlFlow::Break;
            };
            placer.drain();
            glib::ControlFlow::Continue
        });
    }

    /// Handle queued events, including ones read while awaiting a reply.
    fn drain(&self) {
        while let Ok(Some(event)) = self.connection.poll_for_event() {
            if let Event::PropertyNotify(event) = event
                && event.atom == self.wm_state
                && event.state == Property::NEW_VALUE
                && let Some(managed) = self.managed.borrow().as_ref()
            {
                managed();
            }
        }
    }

    /// Keep above and on every workspace, as Tauri's `always_on_top` and
    /// `visible_on_all_workspaces`.
    /// Window managers without the sticky state (Openbox) take the
    /// all-desktops `_NET_WM_DESKTOP` instead, so send both.
    pub fn pin(&self, surface: &X11Surface) {
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        // _NET_WM_STATE_ADD for both atoms, and every desktop, each from a
        // normal application.
        for (kind, data) in [
            (self.net_wm_state, [1, self.above, self.sticky, 1, 0]),
            (self.desktop, [u32::MAX, 1, 0, 0, 0]),
        ] {
            let event = ClientMessageEvent::new(32, xid(surface), kind, data);
            let _ = self.connection.send_event(false, self.root, mask, event);
        }
        let _ = self.connection.flush();
    }

    /// The monitor's bounds in physical pixels at the GDK scale.
    pub fn bounds(
        &self,
        surface: &X11Surface,
        follow_pointer: bool,
    ) -> Option<ThumbnailMonitorBounds> {
        let monitor = self.monitor(surface, follow_pointer)?;
        let scale = monitor.scale_factor();
        let (full, work) = (monitor.geometry(), monitor.workarea());
        Some(ThumbnailMonitorBounds {
            work_x: work.x() * scale,
            work_y: work.y() * scale,
            work_width: (work.width() * scale).max(0) as u32,
            work_height: (work.height() * scale).max(0) as u32,
            full_x: full.x() * scale,
            full_y: full.y() * scale,
            full_width: (full.width() * scale).max(0) as u32,
            full_height: (full.height() * scale).max(0) as u32,
            scale_factor: f64::from(scale),
        })
    }

    /// The pointer on the desktop in logical pixels. A carried window moves
    /// under the pointer, so widget coordinates cannot track the carry.
    pub fn pointer(&self, surface: &X11Surface) -> Option<(f64, f64)> {
        let pointer = self
            .connection
            .query_pointer(self.root)
            .ok()?
            .reply()
            .ok()?;
        let scale = f64::from(self.monitor(surface, true)?.scale_factor());
        Some((
            f64::from(pointer.root_x) / scale,
            f64::from(pointer.root_y) / scale,
        ))
    }

    /// Move the window to its corner of the work area, or to a carried
    /// `origin`, keeping the anchored edge put for the window's current
    /// height. A newly shown or carried stack follows the pointer's monitor;
    /// later moves stay on the window's monitor.
    pub fn place(
        &self,
        surface: &X11Surface,
        count: usize,
        collapsed: bool,
        (placement, origin): (MiniPreviewPlacement, Option<ThumbnailStackOrigin>),
        follow_pointer: bool,
    ) {
        let Some(bounds) = self.bounds(surface, follow_pointer) else {
            return;
        };
        let scale = bounds.scale_factor;
        let geometry =
            preview::thumbnail_geometry(bounds, count.max(1), collapsed, origin, placement);
        let y = if geometry.anchor.is_top() {
            geometry.y
        } else {
            geometry.y + geometry.height - f64::from(surface.height())
        };
        let device = |logical: f64| (logical * scale).round() as i32;
        let position = ConfigureWindowAux::new().x(device(geometry.x)).y(device(y));
        let _ = self.connection.configure_window(xid(surface), &position);
        let _ = self.connection.flush();
        self.drain();
    }

    /// The pointer's monitor and its work area in logical pixels, for the
    /// tray notices' shared placement.
    pub fn areas(&self, surface: &X11Surface) -> Option<(LogicalRect, LogicalRect)> {
        let bounds = self.bounds(surface, true)?;
        let scale = bounds.scale_factor;
        let rect = |x: i32, y: i32, width: u32, height: u32| {
            LogicalRect::new(
                f64::from(x) / scale,
                f64::from(y) / scale,
                f64::from(width) / scale,
                f64::from(height) / scale,
            )
        };
        Some((
            rect(
                bounds.full_x,
                bounds.full_y,
                bounds.full_width,
                bounds.full_height,
            ),
            rect(
                bounds.work_x,
                bounds.work_y,
                bounds.work_width,
                bounds.work_height,
            ),
        ))
    }

    /// Move the window's top-left corner to logical `(x, y)`.
    pub fn move_to(&self, surface: &X11Surface, x: f64, y: f64) {
        let scale = f64::from(surface.scale_factor());
        let device = |logical: f64| (logical * scale).round() as i32;
        let position = ConfigureWindowAux::new().x(device(x)).y(device(y));
        let _ = self.connection.configure_window(xid(surface), &position);
        let _ = self.connection.flush();
        self.drain();
    }

    /// A transient notification the window manager never manages (no
    /// focus, frame or placement of its own), as the wgpu launch notice's
    /// override-redirect `_NET_WM_WINDOW_TYPE_NOTIFICATION` window. Call
    /// before the first map.
    pub fn unmanaged_notification(&self, surface: &X11Surface) {
        let attributes = ChangeWindowAttributesAux::new().override_redirect(1);
        let _ = self
            .connection
            .change_window_attributes(xid(surface), &attributes);
        let atom = |name: &[u8]| {
            Some(
                self.connection
                    .intern_atom(false, name)
                    .ok()?
                    .reply()
                    .ok()?
                    .atom,
            )
        };
        if let (Some(kind), Some(notification)) = (
            atom(b"_NET_WM_WINDOW_TYPE"),
            atom(b"_NET_WM_WINDOW_TYPE_NOTIFICATION"),
        ) {
            let _ = self.connection.change_property32(
                PropMode::REPLACE,
                xid(surface),
                kind,
                AtomEnum::ATOM,
                &[notification],
            );
        }
        let _ = self.connection.flush();
    }

    /// A capture selector over a whole display: unmanaged (no frame, focus
    /// or placement of its own, above panels) at the display's origin in X
    /// pixels and size. Call before the first map.
    pub fn cover(&self, surface: &X11Surface, (x, y, width, height): (i32, i32, u32, u32)) {
        let attributes = ChangeWindowAttributesAux::new().override_redirect(1);
        let _ = self
            .connection
            .change_window_attributes(xid(surface), &attributes);
        let geometry = ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(width)
            .height(height)
            .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE);
        let _ = self.connection.configure_window(xid(surface), &geometry);
        let _ = self.connection.flush();
    }

    fn monitor(&self, surface: &X11Surface, follow_pointer: bool) -> Option<X11Monitor> {
        let display = surface.display();
        let pointer = follow_pointer
            .then(|| self.connection.query_pointer(self.root).ok()?.reply().ok())
            .flatten();
        let monitors = display.monitors();
        let under_pointer = pointer.and_then(|pointer| {
            let (x, y) = (i32::from(pointer.root_x), i32::from(pointer.root_y));
            (0..monitors.n_items())
                .filter_map(|index| monitors.item(index).and_downcast::<gdk::Monitor>())
                .find(|monitor| {
                    let (rect, scale) = (monitor.geometry(), monitor.scale_factor());
                    (rect.x() * scale..(rect.x() + rect.width()) * scale).contains(&x)
                        && (rect.y() * scale..(rect.y() + rect.height()) * scale).contains(&y)
                })
        });
        under_pointer
            .or_else(|| display.monitor_at_surface(surface))?
            .downcast()
            .ok()
    }
}

fn xid(surface: &X11Surface) -> u32 {
    // X resource ids are 29-bit.
    surface.xid() as u32
}
