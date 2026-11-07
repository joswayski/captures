//! Shipping's `.screenshot-sidebar-resize`: an 8 px separator straddling the
//! properties panel's left edge. Drag it, or focus it and use the arrow keys
//! (Shift for 40 px steps), Home and End; Enter or a double click returns to
//! the default width. A 2 px accent line shows while hovered, focused or
//! dragged. The width is window-local and never part of the document.
use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, graphene};

use crate::tokens::Tokens;
use crate::widgets::SnapshotArea;

/// `sidebarMinWidth`.
const MIN: i32 = 280;
/// `.screenshot-sidebar-resize { width: var(--s-4) }`.
pub(crate) const HANDLE: i32 = 8;

pub(crate) struct SidebarResize {
    pub(crate) handle: SnapshotArea,
    /// The user's width, or `None` for the default.
    preferred: Cell<Option<i32>>,
    editor_width: Cell<i32>,
    drag_from: Cell<Option<i32>>,
    hovered: Cell<bool>,
    sidebar: gtk::Widget,
}

impl SidebarResize {
    pub(crate) fn new(tokens: &Rc<Tokens>, sidebar: &impl IsA<gtk::Widget>) -> Rc<Self> {
        let handle = SnapshotArea::new(|_, _, _| {});
        handle.set_accessible_role(gtk::AccessibleRole::Separator);
        handle.set_focusable(true);
        handle.set_can_focus(true);
        handle.set_size_request(HANDLE, -1);
        handle.set_halign(gtk::Align::Start);
        handle.set_cursor_from_name(Some("col-resize"));
        handle.set_tooltip_text(Some("Drag to resize · Double-click to reset"));
        handle.update_property(&[
            gtk::accessible::Property::Label("Resize properties panel"),
            gtk::accessible::Property::Orientation(gtk::Orientation::Vertical),
        ]);
        let resize = Rc::new(SidebarResize {
            handle: handle.clone(),
            preferred: Cell::new(None),
            editor_width: Cell::new(1280),
            drag_from: Cell::new(None),
            hovered: Cell::new(false),
            sidebar: sidebar.clone().upcast(),
        });
        let accent = tokens.rgba("theme-accent");
        {
            let weak = Rc::downgrade(&resize);
            handle.set_draw(move |snapshot, w, h| {
                let Some(resize) = weak.upgrade() else { return };
                if resize.hovered.get()
                    || resize.drag_from.get().is_some()
                    || resize.handle.has_visible_focus()
                {
                    snapshot.append_color(&accent, &graphene::Rect::new((w - 2.) / 2., 0., 2., h));
                }
            });
        }
        let motion = gtk::EventControllerMotion::new();
        {
            let weak = Rc::downgrade(&resize);
            motion.connect_enter(move |_, _, _| {
                if let Some(resize) = weak.upgrade() {
                    resize.hovered.set(true);
                    resize.handle.queue_draw();
                }
            });
            let weak = Rc::downgrade(&resize);
            motion.connect_leave(move |_| {
                if let Some(resize) = weak.upgrade() {
                    resize.hovered.set(false);
                    resize.handle.queue_draw();
                }
            });
        }
        handle.add_controller(motion);
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        {
            let weak = Rc::downgrade(&resize);
            drag.connect_drag_begin(move |_, _, _| {
                if let Some(resize) = weak.upgrade() {
                    resize.drag_from.set(Some(resize.width()));
                    resize.handle.queue_draw();
                }
            });
            let weak = Rc::downgrade(&resize);
            drag.connect_drag_update(move |_, dx, _| {
                if let Some(resize) = weak.upgrade()
                    && let Some(from) = resize.drag_from.get()
                {
                    // Dragging left widens the panel.
                    resize.resize(from - dx.round() as i32);
                }
            });
            let weak = Rc::downgrade(&resize);
            drag.connect_drag_end(move |_, _, _| {
                if let Some(resize) = weak.upgrade() {
                    resize.drag_from.set(None);
                    resize.handle.queue_draw();
                }
            });
        }
        handle.add_controller(drag);
        let click = gtk::GestureClick::new();
        {
            let weak = Rc::downgrade(&resize);
            click.connect_pressed(move |gesture, presses, _, _| {
                if presses == 2
                    && let Some(resize) = weak.upgrade()
                {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    resize.reset();
                }
            });
        }
        handle.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        {
            let weak = Rc::downgrade(&resize);
            keys.connect_key_pressed(move |_, key, _, modifiers| {
                let Some(resize) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let step = if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    40
                } else {
                    10
                };
                let width = resize.width();
                match key {
                    gdk::Key::Left | gdk::Key::KP_Left => resize.resize(width + step),
                    gdk::Key::Right | gdk::Key::KP_Right => resize.resize(width - step),
                    gdk::Key::Home | gdk::Key::KP_Home => resize.resize(MIN),
                    gdk::Key::End | gdk::Key::KP_End => resize.resize(resize.max()),
                    gdk::Key::Return | gdk::Key::KP_Enter => resize.reset(),
                    // Other keys must not reach the canvas shortcuts either.
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            });
        }
        handle.add_controller(keys);
        let weak = Rc::downgrade(&resize);
        handle.connect_has_focus_notify(move |_| {
            if let Some(resize) = weak.upgrade() {
                resize.handle.queue_draw();
            }
        });
        resize.apply();
        resize
    }

    /// `sidebarMaxWidth`: at most 480, leaving the rail and 360 px of canvas.
    fn max(&self) -> i32 {
        MIN.max(480.min(self.editor_width.get() - 56 - 360))
    }

    /// `sidebarWidth`: the user's width, else 280 in a compact window and
    /// 320 otherwise, inside the limits.
    pub(crate) fn width(&self) -> i32 {
        let default =
            if f64::from(self.editor_width.get()) <= captures_app::editor_chrome::COMPACT_WIDTH {
                280
            } else {
                320
            };
        self.preferred
            .get()
            .unwrap_or(default)
            .clamp(MIN, self.max())
    }

    fn resize(&self, width: i32) {
        self.preferred.set(Some(width.clamp(MIN, self.max())));
        self.apply();
    }

    fn reset(&self) {
        self.preferred.set(None);
        self.apply();
    }

    /// Track the editor's width; the limits and the default follow it.
    pub(crate) fn set_editor_width(&self, width: i32) {
        if self.editor_width.replace(width) != width {
            self.apply();
        }
    }

    fn apply(&self) {
        let width = self.width();
        if self.sidebar.width_request() != width {
            self.sidebar.set_size_request(width, -1);
        }
        let text = format!("{width} pixels wide");
        self.handle.update_property(&[
            gtk::accessible::Property::ValueMin(f64::from(MIN)),
            gtk::accessible::Property::ValueMax(f64::from(self.max())),
            gtk::accessible::Property::ValueNow(f64::from(width)),
            gtk::accessible::Property::ValueText(&text),
        ]);
    }
}
