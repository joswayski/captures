//! Small source-compatibility layer for presentation-neutral GTK3 call shapes.
//!
//! GTK4 removed container APIs and event signals. Keeping these adapters in one
//! place lets the migrated surfaces use GTK4 children and event controllers
//! without obscuring their application behavior with mechanical rewrites.
#![allow(dead_code)]

use gtk::{gdk, glib, prelude::*};

pub mod prelude {
    pub use super::{
        BoxCompat, ButtonCompat, DrawingAreaCompat, FrameCompat, LegacyEvents, OverlayCompat,
        ResponseCompat, RevealerCompat, ScrolledWindowCompat, WidgetCompat, WindowCompat,
    };
}

pub trait BoxCompat {
    fn pack_start(&self, child: &impl IsA<gtk::Widget>, expand: bool, fill: bool, padding: u32);
    fn pack_end(&self, child: &impl IsA<gtk::Widget>, expand: bool, fill: bool, padding: u32);
    fn add(&self, child: &impl IsA<gtk::Widget>);
    fn children(&self) -> Vec<gtk::Widget>;
}

impl BoxCompat for gtk::Box {
    fn pack_start(&self, child: &impl IsA<gtk::Widget>, expand: bool, fill: bool, padding: u32) {
        set_box_child_layout(self, child, expand, fill, padding);
        child.remove_css_class("compat-pack-end");
        self.append(child);
    }

    fn pack_end(&self, child: &impl IsA<gtk::Widget>, expand: bool, fill: bool, padding: u32) {
        set_box_child_layout(self, child, expand, fill, padding);
        child.add_css_class("compat-pack-end");
        let first_end = self
            .children()
            .into_iter()
            .find(|widget| widget.has_css_class("compat-pack-end"));
        self.append(child);
        if let Some(first_end) = first_end {
            self.reorder_child_after(child, first_end.prev_sibling().as_ref());
        }
    }

    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.append(child);
    }

    fn children(&self) -> Vec<gtk::Widget> {
        children(self)
    }
}

fn set_box_child_layout(
    parent: &gtk::Box,
    child: &impl IsA<gtk::Widget>,
    expand: bool,
    fill: bool,
    padding: u32,
) {
    if parent.orientation() == gtk::Orientation::Horizontal {
        child.set_hexpand(expand);
        child.set_halign(if fill {
            gtk::Align::Fill
        } else {
            gtk::Align::Start
        });
        if padding > 0 {
            child.set_margin_start(padding as i32);
            child.set_margin_end(padding as i32);
        }
    } else {
        child.set_vexpand(expand);
        child.set_valign(if fill {
            gtk::Align::Fill
        } else {
            gtk::Align::Start
        });
        if padding > 0 {
            child.set_margin_top(padding as i32);
            child.set_margin_bottom(padding as i32);
        }
    }
}

fn children(parent: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut result = Vec::new();
    let mut child = parent.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        result.push(widget);
    }
    result
}

pub trait WindowCompat {
    fn add(&self, child: &impl IsA<gtk::Widget>);
    fn set_keep_above(&self, above: bool);
    fn set_skip_taskbar_hint(&self, skip: bool);
    fn set_accept_focus(&self, accept: bool);
    fn set_position<T>(&self, position: T);
    fn resize(&self, width: i32, height: i32);
    fn move_(&self, x: i32, y: i32);
}

impl WindowCompat for gtk::Window {
    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.set_child(Some(child));
    }
    fn set_keep_above(&self, above: bool) {
        set_wm_state(self, "_NET_WM_STATE_ABOVE", above);
    }
    fn set_skip_taskbar_hint(&self, skip: bool) {
        set_wm_state(self, "_NET_WM_STATE_SKIP_TASKBAR", skip);
    }
    fn set_accept_focus(&self, accept: bool) {
        self.set_focusable(accept);
        schedule_x11(self, move |connection, xid| {
            use x11rb::protocol::xproto::ConnectionExt;
            use x11rb::wrapper::ConnectionExt as _;
            let wm_hints = connection.intern_atom(false, b"WM_HINTS")?.reply()?.atom;
            // ICCCM WM_HINTS: InputHint + input followed by seven unused longs.
            connection.change_property32(
                x11rb::protocol::xproto::PropMode::REPLACE,
                xid,
                wm_hints,
                wm_hints,
                &[1, u32::from(accept), 0, 0, 0, 0, 0, 0, 0],
            )?;
            Ok(())
        });
    }
    fn set_position<T>(&self, _position: T) {
        let window = self.clone();
        self.connect_map(move |_| {
            let Some(display) = gdk::Display::default() else {
                return;
            };
            let Some(monitor) = display.monitors().item(0).and_downcast::<gdk::Monitor>() else {
                return;
            };
            let geometry = monitor.geometry();
            let allocation = window.allocation();
            window.move_(
                geometry.x() + (geometry.width() - allocation.width()) / 2,
                geometry.y() + (geometry.height() - allocation.height()) / 2,
            );
        });
    }
    fn resize(&self, width: i32, height: i32) {
        self.set_default_size(width, height);
    }
    fn move_(&self, x: i32, y: i32) {
        schedule_x11(self, move |connection, xid| {
            use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt};
            connection.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y))?;
            Ok(())
        });
    }
}

fn set_wm_state(window: &gtk::Window, state_name: &'static str, enabled: bool) {
    schedule_x11(window, move |connection, xid| {
        use x11rb::{
            connection::Connection,
            protocol::xproto::{ClientMessageData, ClientMessageEvent, ConnectionExt, EventMask},
        };
        let state = connection
            .intern_atom(false, b"_NET_WM_STATE")?
            .reply()?
            .atom;
        let property = connection
            .intern_atom(false, state_name.as_bytes())?
            .reply()?
            .atom;
        let root = connection.setup().roots[0].root;
        let event = ClientMessageEvent::new(
            32,
            xid,
            state,
            ClientMessageData::from([u32::from(enabled), property, 0, 1, 0]),
        );
        connection.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?;
        Ok(())
    });
}

fn schedule_x11(
    window: &gtk::Window,
    operation: impl Fn(
        &x11rb::rust_connection::RustConnection,
        u32,
    ) -> Result<(), Box<dyn std::error::Error>>
    + Clone
    + 'static,
) {
    fn apply(
        window: &gtk::Window,
        operation: &impl Fn(
            &x11rb::rust_connection::RustConnection,
            u32,
        ) -> Result<(), Box<dyn std::error::Error>>,
    ) -> bool {
        use x11rb::connection::Connection;
        let Some(surface) = gtk::prelude::NativeExt::surface(window) else {
            return false;
        };
        let Ok(surface) = surface.downcast::<gdk4_x11::X11Surface>() else {
            return false;
        };
        let Ok((connection, _)) = x11rb::connect(None) else {
            return false;
        };
        if let Err(error) = operation(&connection, surface.xid() as u32) {
            eprintln!("Cannot apply X11 window presentation: {error}");
        }
        let _ = connection.flush();
        true
    }

    if apply(window, &operation) {
        return;
    }
    let window = window.clone();
    window.clone().connect_map(move |_| {
        apply(&window, &operation);
    });
}

pub trait OverlayCompat {
    fn add(&self, child: &impl IsA<gtk::Widget>);
    fn set_overlay_pass_through(&self, child: &impl IsA<gtk::Widget>, pass: bool);
    fn reorder_overlay(&self, child: &impl IsA<gtk::Widget>, position: i32);
}
impl OverlayCompat for gtk::Overlay {
    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.set_child(Some(child));
    }
    fn set_overlay_pass_through(&self, child: &impl IsA<gtk::Widget>, pass: bool) {
        child.set_can_target(!pass);
    }
    fn reorder_overlay(&self, child: &impl IsA<gtk::Widget>, _position: i32) {
        self.remove_overlay(child);
        self.add_overlay(child);
    }
}

pub trait ScrolledWindowCompat {
    fn add(&self, child: &impl IsA<gtk::Widget>);
}
impl ScrolledWindowCompat for gtk::ScrolledWindow {
    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.set_child(Some(child));
    }
}

pub trait FrameCompat {
    fn add(&self, child: &impl IsA<gtk::Widget>);
}
impl FrameCompat for gtk::Frame {
    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.set_child(Some(child));
    }
}

pub trait RevealerCompat {
    fn add(&self, child: &impl IsA<gtk::Widget>);
}
impl RevealerCompat for gtk::Revealer {
    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.set_child(Some(child));
    }
}

pub trait ResponseCompat {
    fn run(&self) -> gtk::ResponseType;
}
impl<T: IsA<gtk::Dialog> + IsA<gtk::Window>> ResponseCompat for T {
    fn run(&self) -> gtk::ResponseType {
        let response = std::rc::Rc::new(std::cell::Cell::new(gtk::ResponseType::None));
        let main = glib::MainLoop::new(None, false);
        let main_for_response = main.clone();
        let response_for_signal = response.clone();
        self.connect_response(move |_, value| {
            response_for_signal.set(value);
            main_for_response.quit();
        });
        self.present();
        main.run();
        response.get()
    }
}

pub trait ButtonCompat {
    fn add(&self, child: &impl IsA<gtk::Widget>);
    fn set_image(&self, child: Option<&impl IsA<gtk::Widget>>);
    fn set_always_show_image(&self, always: bool);
    fn clicked(&self);
}
impl ButtonCompat for gtk::Button {
    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.set_child(Some(child));
    }
    fn set_image(&self, child: Option<&impl IsA<gtk::Widget>>) {
        self.set_child(child);
    }
    fn set_always_show_image(&self, _always: bool) {}
    fn clicked(&self) {
        self.emit_clicked();
    }
}
impl ButtonCompat for gtk::ToggleButton {
    fn add(&self, child: &impl IsA<gtk::Widget>) {
        self.set_child(Some(child));
    }
    fn set_image(&self, child: Option<&impl IsA<gtk::Widget>>) {
        self.set_child(child);
    }
    fn set_always_show_image(&self, _always: bool) {}
    fn clicked(&self) {
        self.emit_clicked();
    }
}

pub trait WidgetCompat {
    fn show_all(&self);
    fn set_no_show_all(&self, hidden: bool);
    fn is_no_show_all(&self) -> bool;
    fn add_events<T>(&self, events: T);
    fn set_border_width(&self, width: u32);
    fn set_has_window(&self, has_window: bool);
    fn connect_size_allocate<F: Fn(&Self, &gtk::Allocation) + 'static>(&self, f: F)
    where
        Self: Sized + Clone + 'static;
}
impl<T: IsA<gtk::Widget>> WidgetCompat for T {
    fn show_all(&self) {
        self.set_visible(true);
    }
    fn set_no_show_all(&self, hidden: bool) {
        self.set_visible(!hidden);
    }
    fn is_no_show_all(&self) -> bool {
        !self.is_visible()
    }
    fn add_events<U>(&self, _events: U) {}
    fn set_border_width(&self, width: u32) {
        let width = width as i32;
        self.set_margin_top(width);
        self.set_margin_bottom(width);
        self.set_margin_start(width);
        self.set_margin_end(width);
    }
    fn set_has_window(&self, _has_window: bool) {}
    fn connect_size_allocate<F: Fn(&Self, &gtk::Allocation) + 'static>(&self, f: F)
    where
        Self: Sized + Clone + 'static,
    {
        let callback = std::rc::Rc::new(f);
        for property in ["width", "height"] {
            let widget = self.clone();
            let callback = callback.clone();
            self.connect_notify_local(Some(property), move |_, _| {
                callback(&widget, &widget.allocation());
            });
        }
    }
}

#[derive(Clone, Copy)]
pub struct PointerEvent {
    x: f64,
    y: f64,
    root_x: f64,
    root_y: f64,
    button: u32,
    presses: i32,
    state: gdk::ModifierType,
}
impl PointerEvent {
    pub fn position(self) -> (f64, f64) {
        (self.x, self.y)
    }
    pub fn root(self) -> (f64, f64) {
        (self.root_x, self.root_y)
    }
    pub fn button(self) -> u32 {
        self.button
    }
    pub fn state(self) -> gdk::ModifierType {
        self.state
    }
    pub fn delta(self) -> (f64, f64) {
        (0.0, self.y)
    }
    pub fn direction(self) -> gdk::ScrollDirection {
        if self.y < 0.0 {
            gdk::ScrollDirection::Up
        } else {
            gdk::ScrollDirection::Down
        }
    }
    pub fn detail(self) -> gdk::NotifyType {
        gdk::NotifyType::Nonlinear
    }
    pub fn is_double_click(self) -> bool {
        self.presses >= 2
    }
}

#[derive(Clone, Copy)]
pub struct KeyEvent {
    key: gdk::Key,
    state: gdk::ModifierType,
}
impl KeyEvent {
    pub fn keyval(self) -> gdk::Key {
        self.key
    }
    pub fn state(self) -> gdk::ModifierType {
        self.state
    }
}

pub trait LegacyEvents: IsA<gtk::Widget> + Clone + 'static {
    fn connect_button_press_event<F: Fn(&Self, PointerEvent) -> glib::Propagation + 'static>(
        &self,
        f: F,
    ) {
        let gesture = gtk::GestureClick::new();
        let widget = self.clone();
        gesture.connect_pressed(move |gesture, presses, x, y| {
            let (root_x, root_y) = root_pointer().unwrap_or((x, y));
            let event = PointerEvent {
                x,
                y,
                root_x,
                root_y,
                button: gesture.current_button(),
                presses,
                state: gesture.current_event_state(),
            };
            let _ = f(&widget, event);
        });
        self.add_controller(gesture);
    }
    fn connect_button_release_event<F: Fn(&Self, PointerEvent) -> glib::Propagation + 'static>(
        &self,
        f: F,
    ) {
        let gesture = gtk::GestureClick::new();
        let widget = self.clone();
        gesture.connect_released(move |gesture, _, x, y| {
            let (root_x, root_y) = root_pointer().unwrap_or((x, y));
            let event = PointerEvent {
                x,
                y,
                root_x,
                root_y,
                button: gesture.current_button(),
                presses: 0,
                state: gesture.current_event_state(),
            };
            let _ = f(&widget, event);
        });
        self.add_controller(gesture);
    }
    fn connect_motion_notify_event<F: Fn(&Self, PointerEvent) -> glib::Propagation + 'static>(
        &self,
        f: F,
    ) {
        let controller = gtk::EventControllerMotion::new();
        let widget = self.clone();
        controller.connect_motion(move |controller, x, y| {
            let (root_x, root_y) = root_pointer().unwrap_or((x, y));
            let event = PointerEvent {
                x,
                y,
                root_x,
                root_y,
                button: 0,
                presses: 0,
                state: controller.current_event_state(),
            };
            let _ = f(&widget, event);
        });
        self.add_controller(controller);
    }
    fn connect_enter_notify_event<F: Fn(&Self, PointerEvent) -> glib::Propagation + 'static>(
        &self,
        f: F,
    ) {
        let controller = gtk::EventControllerMotion::new();
        let widget = self.clone();
        controller.connect_enter(move |controller, x, y| {
            let event = PointerEvent {
                x,
                y,
                root_x: x,
                root_y: y,
                button: 0,
                presses: 0,
                state: controller.current_event_state(),
            };
            let _ = f(&widget, event);
        });
        self.add_controller(controller);
    }
    fn connect_leave_notify_event<F: Fn(&Self, PointerEvent) -> glib::Propagation + 'static>(
        &self,
        f: F,
    ) {
        let controller = gtk::EventControllerMotion::new();
        let widget = self.clone();
        controller.connect_leave(move |controller| {
            let event = PointerEvent {
                x: -1.0,
                y: -1.0,
                root_x: -1.0,
                root_y: -1.0,
                button: 0,
                presses: 0,
                state: controller.current_event_state(),
            };
            let _ = f(&widget, event);
        });
        self.add_controller(controller);
    }
    fn connect_key_press_event<F: Fn(&Self, KeyEvent) -> glib::Propagation + 'static>(&self, f: F) {
        let controller = gtk::EventControllerKey::new();
        let widget = self.clone();
        controller.connect_key_pressed(move |_, key, _, state| f(&widget, KeyEvent { key, state }));
        self.add_controller(controller);
    }
    fn connect_key_release_event<F: Fn(&Self, KeyEvent) -> glib::Propagation + 'static>(
        &self,
        f: F,
    ) {
        let controller = gtk::EventControllerKey::new();
        let widget = self.clone();
        controller.connect_key_released(move |_, key, _, state| {
            let _ = f(&widget, KeyEvent { key, state });
        });
        self.add_controller(controller);
    }
    fn connect_focus_in_event<F: Fn(&Self, ()) -> glib::Propagation + 'static>(&self, f: F) {
        let controller = gtk::EventControllerFocus::new();
        let widget = self.clone();
        controller.connect_enter(move |_| {
            let _ = f(&widget, ());
        });
        self.add_controller(controller);
    }
    fn connect_focus_out_event<F: Fn(&Self, ()) -> glib::Propagation + 'static>(&self, f: F) {
        let controller = gtk::EventControllerFocus::new();
        let widget = self.clone();
        controller.connect_leave(move |_| {
            let _ = f(&widget, ());
        });
        self.add_controller(controller);
    }
    fn connect_scroll_event<F: Fn(&Self, PointerEvent) -> glib::Propagation + 'static>(
        &self,
        f: F,
    ) {
        let controller =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let widget = self.clone();
        controller.connect_scroll(move |controller, x, y| {
            f(
                &widget,
                PointerEvent {
                    x,
                    y,
                    root_x: x,
                    root_y: y,
                    button: 0,
                    presses: 0,
                    state: controller.current_event_state(),
                },
            )
        });
        self.add_controller(controller);
    }
}

fn root_pointer() -> Option<(f64, f64)> {
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};
    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let pointer = connection.query_pointer(root).ok()?.reply().ok()?;
    Some((f64::from(pointer.root_x), f64::from(pointer.root_y)))
}
impl<T: IsA<gtk::Widget> + Clone + 'static> LegacyEvents for T {}

pub trait DrawingAreaCompat {
    fn connect_draw<F: Fn(&gtk::DrawingArea, &gtk::cairo::Context) -> glib::Propagation + 'static>(
        &self,
        f: F,
    );
}
impl DrawingAreaCompat for gtk::DrawingArea {
    fn connect_draw<
        F: Fn(&gtk::DrawingArea, &gtk::cairo::Context) -> glib::Propagation + 'static,
    >(
        &self,
        f: F,
    ) {
        self.set_draw_func(move |area, context, _, _| {
            let _ = f(area, context);
        });
    }
}
