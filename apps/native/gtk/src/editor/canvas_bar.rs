//! Shipping's header `.screenshot-canvas-toolbar`: Canvas W × H fields,
//! Trim edges and the Background color picker (`CanvasBackgroundPicker`).
//! Each control sends one session request; values follow the document the
//! worker publishes.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use captures_app::editor_chrome::{colors, header};
use captures_app::editor_session::Request;
use gtk::prelude::*;
use gtk::{gdk, graphene, gsk};

use super::controls::{ColorField, NumberField};
use super::properties::check_row;
use crate::tokens::Tokens;
use crate::widgets::{Icon, SnapshotArea, icon_button};

/// `MAX_SCREENSHOT_OUTPUT_DIMENSION`.
const MAX_DIMENSION: f64 = 16_384.;

type Send = Rc<dyn Fn(Request)>;

pub(super) struct CanvasBar {
    pub(super) root: gtk::Box,
    width: NumberField,
    height: NumberField,
    trim: gtk::Button,
    background: gtk::Button,
    /// The document background (`None` is transparent) and the last solid
    /// color, which Solid background restores.
    value: Rc<RefCell<Option<String>>>,
    last_solid: Rc<RefCell<String>>,
    chip: SnapshotArea,
    syncing: Rc<Cell<bool>>,
    /// The published size the W/H fields compare against.
    size: Rc<Cell<(f64, f64)>>,
    /// The open menu's color field, kept in step with the document.
    menu_field: RefCell<Option<Rc<ColorField>>>,
}

fn dimension(caption: &str, field: &NumberField) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("canvas-dim");
    let label = gtk::Label::new(Some(caption));
    label.add_css_class("canvas-dim-label");
    label.set_accessible_role(gtk::AccessibleRole::Presentation);
    row.append(&label);
    // `.screenshot-canvas-dim .number-input { width: 74px }`.
    field.root.set_size_request(74, -1);
    field.root.set_hexpand(false);
    field.spin.set_hexpand(false);
    field.spin.set_width_chars(5);
    field.spin.set_max_width_chars(5);
    row.append(&field.root);
    row
}

impl CanvasBar {
    pub(super) fn new(tokens: &Rc<Tokens>, send: impl Fn(Request) + 'static) -> Rc<Self> {
        let send: Send = Rc::new(send);
        let syncing = Rc::new(Cell::new(false));
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        root.add_css_class("editor-group");
        root.add_css_class("canvas-toolbar");
        root.set_valign(gtk::Align::Center);
        root.set_accessible_role(gtk::AccessibleRole::Group);
        root.update_property(&[gtk::accessible::Property::Label(header::CANVAS)]);
        let caption = gtk::Label::new(Some(header::CANVAS));
        caption.add_css_class("editor-group-label");
        caption.set_accessible_role(gtk::AccessibleRole::Presentation);
        root.append(&caption);

        let value: Rc<RefCell<Option<String>>> = Rc::default();
        let size: Rc<Cell<(f64, f64)>> = Rc::new(Cell::new((1., 1.)));
        let resize = |axis_width: bool| {
            let send = send.clone();
            let syncing = syncing.clone();
            let size = size.clone();
            move |value: f64| {
                if syncing.get() {
                    return;
                }
                let next = value.round().clamp(1., MAX_DIMENSION);
                let (w, h) = size.get();
                let (width, height) = if axis_width { (next, h) } else { (w, next) };
                if (width, height) != (w, h) {
                    send(Request::ResizeCanvas { width, height });
                }
            }
        };
        let width = NumberField::new(header::CANVAS_WIDTH, (1., MAX_DIMENSION), resize(true));
        let height = NumberField::new(header::CANVAS_HEIGHT, (1., MAX_DIMENSION), resize(false));
        for field in [&width, &height] {
            field.root.add_css_class("compact");
        }
        width.spin.set_tooltip_text(Some(header::CANVAS_WIDTH));
        height.spin.set_tooltip_text(Some(header::CANVAS_HEIGHT));
        root.append(&dimension("W", &width));
        let times = gtk::Label::new(Some("×"));
        times.add_css_class("canvas-dim-label");
        times.set_accessible_role(gtk::AccessibleRole::Presentation);
        root.append(&times);
        root.append(&dimension("H", &height));
        let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        split.add_css_class("canvas-toolbar-split");
        split.set_valign(gtk::Align::Center);
        root.append(&split);

        let (trim, _, _) = icon_button("trim", header::TRIM, &["canvas-tool"]);
        trim.set_tooltip_text(Some(header::TRIM_TOOLTIP));
        {
            let send = send.clone();
            trim.connect_clicked(move |_| send(Request::TrimCanvas));
        }
        root.append(&trim);

        let background = gtk::Button::new();
        crate::widgets::action_cursor(&background);
        background.add_css_class("canvas-tool");
        background.add_css_class("canvas-bg-trigger");
        background.set_tooltip_text(Some(colors::BACKGROUND_TOOLTIP));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let checker = (
            tokens.rgba("canvas-checker-a"),
            tokens.rgba("canvas-checker-b"),
        );
        let rim = tokens.rgba("border-strong");
        let chip = {
            let value = value.clone();
            SnapshotArea::new(move |snapshot, w, h| {
                let bounds = graphene::Rect::new(0., 0., w, h);
                let round = gsk::RoundedRect::from_rect(bounds, 3.);
                snapshot.push_rounded_clip(&round);
                match value.borrow().as_deref() {
                    Some(color) => {
                        let fill = gdk::RGBA::parse(color).unwrap_or(gdk::RGBA::WHITE);
                        snapshot.append_color(&fill, &bounds);
                    }
                    // `.is-transparent`: 4 px checker squares.
                    None => {
                        snapshot.append_color(&checker.1, &bounds);
                        for (x, y) in [(0., 0.), (4., 4.), (8., 0.), (0., 8.), (8., 8.)] {
                            snapshot.append_color(&checker.0, &graphene::Rect::new(x, y, 4., 4.));
                        }
                    }
                }
                snapshot.pop();
                snapshot.append_border(&round, &[1.; 4], &[rim; 4]);
            })
        };
        chip.set_size_request(14, 14);
        chip.set_valign(gtk::Align::Center);
        row.append(&chip);
        row.append(&gtk::Label::new(Some(colors::BACKGROUND)));
        let chevron = Icon::new("chevron-down", 11).with_stroke(2.);
        chevron.add_css_class("canvas-bg-chevron");
        row.append(&chevron);
        background.set_child(Some(&row));
        root.append(&background);

        let bar = Rc::new(CanvasBar {
            root,
            width,
            height,
            trim,
            background,
            value,
            last_solid: Rc::new(RefCell::new(colors::DEFAULT_CANVAS_BACKGROUND.into())),
            chip,
            syncing,
            size,
            menu_field: RefCell::default(),
        });
        {
            let weak = Rc::downgrade(&bar);
            let tokens = tokens.clone();
            bar.background.connect_clicked(move |_| {
                if let Some(bar) = weak.upgrade() {
                    bar.open_menu(&tokens, &send);
                }
            });
        }
        bar
    }

    /// Show the published canvas size, background and trim availability.
    pub(super) fn sync(&self, width: f64, height: f64, background: Option<&str>, can_trim: bool) {
        self.syncing.set(true);
        self.size.set((width, height));
        self.width.set(width, false);
        self.height.set(height, false);
        self.syncing.set(false);
        self.trim.set_sensitive(can_trim);
        *self.value.borrow_mut() = background.map(str::to_owned);
        if let Some(color) = background {
            *self.last_solid.borrow_mut() = color.to_owned();
        }
        let name = colors::background_label(background);
        self.background
            .update_property(&[gtk::accessible::Property::Label(&name)]);
        self.chip.queue_draw();
        if let Some(field) = self.menu_field.borrow().as_ref() {
            field.set(background.unwrap_or(&self.last_solid.borrow()));
        }
    }

    fn open_menu(self: &Rc<Self>, tokens: &Rc<Tokens>, send: &Send) {
        let popover = gtk::Popover::new();
        popover.add_css_class("captures-menu");
        popover.add_css_class("canvas-bg-menu");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.set_accessible_role(gtk::AccessibleRole::Dialog);
        content.update_property(&[gtk::accessible::Property::Label(colors::CANVAS_BACKGROUND)]);
        content.set_size_request(colors::MENU_WIDTH as i32 - 16, -1);
        let solid = check_row(colors::SOLID_BACKGROUND, self.value.borrow().is_some());
        {
            let send = send.clone();
            let last = self.last_solid.clone();
            solid.connect_toggled(move |check| {
                let color = check.is_active().then(|| last.borrow().clone());
                send(Request::SetBackground { color });
            });
        }
        content.append(&solid);
        let field = {
            let send = send.clone();
            let last = self.last_solid.clone();
            ColorField::new(tokens, colors::CANVAS_BACKGROUND, move |color| {
                *last.borrow_mut() = color.clone();
                send(Request::SetBackground { color: Some(color) });
            })
        };
        field.compact();
        field.set(
            self.value
                .borrow()
                .as_deref()
                .unwrap_or(&self.last_solid.borrow()),
        );
        content.append(&field.root);
        popover.set_child(Some(&content));
        popover.set_parent(&self.background);
        self.background.add_css_class("open");
        self.background
            .update_state(&[gtk::accessible::State::Expanded(Some(true))]);
        let trigger = self.background.clone();
        popover.connect_closed(move |popover| {
            trigger.remove_css_class("open");
            trigger.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
            let popover = popover.clone();
            gtk::glib::idle_add_local_once(move || popover.unparent());
        });
        *self.menu_field.borrow_mut() = Some(field);
        let weak = Rc::downgrade(self);
        popover.connect_closed(move |_| {
            if let Some(bar) = weak.upgrade() {
                bar.menu_field.borrow_mut().take();
            }
        });
        popover.popup();
    }
}
