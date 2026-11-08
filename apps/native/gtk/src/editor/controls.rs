//! Shipping property controls built on real GTK widgets: `ColorField`
//! (swatch buttons and the custom-color tile), `NumberInput` (a GtkSpinButton
//! with shipping's stacked steppers), the `<select>`/Text style menus and
//! `DropShadowFields`. Each is styled from tokens in `style.css`; the swatch
//! and custom tiles draw their color in a small retained node.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use captures_app::editor::{DropShadowStyle, DropShadowStylePatch};
use captures_app::editor_chrome::colors;
use gtk::prelude::*;
use gtk::{gdk, graphene, gsk};

use super::properties::{RangeSlider, check_row, text};
use crate::tokens::Tokens;
use crate::widgets::{Icon, SnapshotArea};

/// `DROP_SHADOW_BLUR_MAX` and `DROP_SHADOW_OFFSET_MAX` (`captures-image`).
const SHADOW_BLUR_MAX: f64 = 100.;
const SHADOW_OFFSET_MAX: f64 = 500.;

pub(crate) fn pressed(widget: &impl IsA<gtk::Widget>, on: bool) {
    let widget = widget.as_ref();
    if on {
        widget.add_css_class("active");
    } else {
        widget.remove_css_class("active");
    }
    widget.update_state(&[gtk::accessible::State::Pressed(if on {
        gtk::AccessibleTristate::True
    } else {
        gtk::AccessibleTristate::False
    })]);
}

/// A shipping property `<label>`: caption over its control.
pub(crate) fn field(caption: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.add_css_class("property-field");
    root.append(&text(caption, "property-caption"));
    root.append(control);
    root
}

/// `.screenshot-number-pair`: two equal columns.
pub(crate) fn pair(a: &impl IsA<gtk::Widget>, b: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_homogeneous(true);
    row.append(a);
    row.append(b);
    row
}

pub(crate) fn hex(rgba: &gdk::RGBA) -> String {
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(rgba.red()),
        byte(rgba.green()),
        byte(rgba.blue())
    )
}

fn parse(value: &str) -> gdk::RGBA {
    gdk::RGBA::parse(value).unwrap_or(gdk::RGBA::BLACK)
}

/// Shipping `ColorField`: a legend over eight swatches and a custom tile.
pub(crate) struct ColorField {
    pub(crate) root: gtk::Box,
    legend: gtk::Label,
    grid: gtk::FlowBox,
    tiles: Vec<(&'static str, gtk::Button)>,
    value: Rc<RefCell<String>>,
}

impl ColorField {
    pub(crate) fn new(
        tokens: &Rc<Tokens>,
        label: &str,
        changed: impl Fn(String) + 'static,
    ) -> Rc<Self> {
        let changed: Rc<dyn Fn(String)> = Rc::new(changed);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("color-field");
        root.set_accessible_role(gtk::AccessibleRole::Group);
        let legend = text(label, "property-caption");
        root.append(&legend);
        let grid = gtk::FlowBox::new();
        grid.add_css_class("color-swatches");
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_homogeneous(true);
        grid.set_min_children_per_line(1);
        grid.set_max_children_per_line(colors::TILE_COUNT as u32);
        grid.set_row_spacing(8);
        grid.set_column_spacing(0);
        let value = Rc::new(RefCell::new(String::new()));
        let cell = |child: &gtk::Button| {
            let slot = gtk::FlowBoxChild::new();
            slot.set_can_focus(false);
            slot.set_size_request(colors::CELL as i32, -1);
            slot.set_child(Some(child));
            child.set_halign(gtk::Align::Center);
            child.set_valign(gtk::Align::Center);
            grid.insert(&slot, -1);
        };
        let tiles: Vec<_> = colors::SWATCHES
            .iter()
            .map(|swatch| {
                let button = gtk::Button::new();
                crate::widgets::action_cursor(&button);
                button.add_css_class("color-swatch");
                let fill = parse(swatch);
                let dot = SnapshotArea::new(move |snapshot, w, h| {
                    let bounds = graphene::Rect::new(0., 0., w, h);
                    let round = gsk::RoundedRect::from_rect(bounds, w / 2.);
                    snapshot.push_rounded_clip(&round);
                    snapshot.append_color(&fill, &bounds);
                    snapshot.pop();
                });
                let side = colors::TILE as i32 - 2;
                dot.set_size_request(side, side);
                button.set_child(Some(&dot));
                let changed = changed.clone();
                button.connect_clicked(move |_| changed(swatch.to_string()));
                cell(&button);
                (*swatch, button)
            })
            .collect();
        let custom = gtk::Button::new();
        crate::widgets::action_cursor(&custom);
        custom.add_css_class("custom-color");
        custom.set_tooltip_text(Some(colors::CUSTOM_COLOR));
        custom.update_property(&[gtk::accessible::Property::Label(colors::CUSTOM_COLOR)]);
        let inner = tokens.rgba("surface-raised");
        let rim = tokens.rgba("border-strong");
        let wheel = SnapshotArea::new(move |snapshot, w, h| {
            // `conic-gradient(#f44, #ff4, #4f4, #4ff, #44f, #f4f, #f44)`.
            let cr = snapshot.append_cairo(&graphene::Rect::new(0., 0., w, h));
            let (cx, cy) = (f64::from(w) / 2., f64::from(h) / 2.);
            let radius = cx.hypot(cy) + 1.;
            for step in 0..72 {
                let a0 =
                    f64::from(step) / 72. * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
                let a1 = a0 + std::f64::consts::TAU / 72. + 0.01;
                let (r, g, b) = hue(f64::from(step) / 72.);
                cr.set_source_rgb(r, g, b);
                cr.move_to(cx, cy);
                cr.arc(cx, cy, radius, a0, a1);
                cr.close_path();
                let _ = cr.fill();
            }
            let inset = colors::CUSTOM_INSET as f32 - 1.;
            let plate = graphene::Rect::new(inset, inset, w - 2. * inset, h - 2. * inset);
            let round = gsk::RoundedRect::from_rect(plate, 3.);
            snapshot.push_rounded_clip(&round);
            snapshot.append_color(&inner, &plate);
            snapshot.pop();
            snapshot.append_border(&round, &[1.; 4], &[rim; 4]);
        });
        let side = colors::TILE as i32 - 2;
        wheel.set_size_request(side, side);
        custom.set_child(Some(&wheel));
        {
            let value = value.clone();
            let changed = changed.clone();
            custom.connect_clicked(move |button| {
                let parent = button.root().and_downcast::<gtk::Window>();
                let dialog =
                    gtk::ColorChooserDialog::new(Some(colors::CUSTOM_COLOR), parent.as_ref());
                dialog.set_modal(true);
                dialog.set_use_alpha(false);
                dialog.set_rgba(&parse(&colors::custom_seed(&value.borrow())));
                let changed = changed.clone();
                dialog.connect_response(move |dialog, response| {
                    if response == gtk::ResponseType::Ok {
                        changed(hex(&dialog.rgba()));
                    }
                    dialog.close();
                });
                dialog.present();
            });
        }
        cell(&custom);
        root.append(&grid);
        let field = Rc::new(ColorField {
            root,
            legend,
            grid,
            tiles,
            value,
        });
        field.set_label(label);
        field
    }

    /// Swatches only; the legend stays for assistive tech (`compact`).
    /// `minmax(36px, 1fr)` cells across the canvas background card.
    pub(crate) fn compact(&self) {
        self.legend.set_visible(false);
        let layout = colors::grid(colors::MENU_WIDTH - 24., colors::COMPACT_CELL);
        self.grid.set_max_children_per_line(layout.columns as u32);
        self.grid.set_row_spacing(6);
        let mut child = self.grid.first_child();
        while let Some(slot) = child {
            slot.set_size_request(colors::COMPACT_CELL as i32, -1);
            child = slot.next_sibling();
        }
    }

    pub(crate) fn set_label(&self, label: &str) {
        self.legend.set_label(label);
        self.root
            .update_property(&[gtk::accessible::Property::Label(label)]);
        for (swatch, button) in &self.tiles {
            let name = colors::swatch_label(label, swatch);
            button.update_property(&[gtk::accessible::Property::Label(&name)]);
        }
    }

    pub(crate) fn set(&self, value: &str) {
        *self.value.borrow_mut() = value.to_owned();
        for (swatch, button) in &self.tiles {
            pressed(button, colors::swatch_active(value, swatch));
        }
    }
}

fn hue(t: f64) -> (f64, f64, f64) {
    // Shipping's stops are 0.27/1.0 channels (`#f44` etc.).
    let lo = 0x44 as f64 / 255.;
    let h = (t * 6.).rem_euclid(6.);
    let x = 1. - (h % 2. - 1.).abs();
    let (r, g, b) = match h as u32 {
        0 => (1., x, 0.),
        1 => (x, 1., 0.),
        2 => (0., 1., x),
        3 => (0., x, 1.),
        4 => (x, 0., 1.),
        _ => (1., 0., x),
    };
    let lift = |c: f64| lo + c * (1. - lo);
    (lift(r), lift(g), lift(b))
}

/// Shipping `NumberInput`: a real spin button (AT-SPI role, value and typed
/// entry) with its stock buttons hidden for the stacked token steppers.
pub(crate) struct NumberField {
    pub(crate) root: gtk::Box,
    pub(crate) spin: gtk::SpinButton,
}

impl NumberField {
    pub(crate) fn new(name: &str, range: (f64, f64), changed: impl Fn(f64) + 'static) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("number-input");
        let spin = gtk::SpinButton::with_range(range.0, range.1, 1.);
        spin.set_digits(0);
        spin.set_numeric(true);
        spin.set_hexpand(true);
        spin.update_property(&[gtk::accessible::Property::Label(name)]);
        let mut child = spin.first_child();
        while let Some(widget) = child {
            if widget.is::<gtk::Button>() {
                widget.set_visible(false);
            }
            child = widget.next_sibling();
        }
        spin.connect_value_changed(move |spin| changed(spin.value()));
        root.append(&spin);
        let steppers = gtk::Box::new(gtk::Orientation::Vertical, 0);
        steppers.add_css_class("number-input-steppers");
        steppers.set_homogeneous(true);
        for (icon, kind, verb) in [
            ("chevron-up", gtk::SpinType::StepForward, "Increase"),
            ("chevron-down", gtk::SpinType::StepBackward, "Decrease"),
        ] {
            let step = gtk::Button::new();
            crate::widgets::action_cursor(&step);
            step.add_css_class("number-input-step");
            step.set_focusable(false);
            step.set_child(Some(&Icon::new(icon, 11).with_stroke(2.)));
            step.update_property(&[gtk::accessible::Property::Label(&format!("{verb} {name}"))]);
            let spin = spin.clone();
            step.connect_clicked(move |_| spin.spin(kind, 1.));
            steppers.append(&step);
        }
        root.append(&steppers);
        NumberField { root, spin }
    }

    /// Show `value`; with `force` false a field the user is in keeps its text.
    pub(crate) fn set(&self, value: f64, force: bool) {
        let focused = self
            .spin
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focus| {
                focus.is_ancestor(&self.spin) || focus == *self.spin.upcast_ref::<gtk::Widget>()
            });
        if force || !focused {
            self.spin.set_value(if self.spin.digits() == 0 {
                value.round()
            } else {
                value
            });
        }
    }

    pub(crate) fn set_sensitive(&self, on: bool) {
        self.root.set_sensitive(on);
        if on {
            self.root.remove_css_class("is-disabled");
        } else {
            self.root.add_css_class("is-disabled");
        }
    }
}

/// A shipping `<select>` or Text style picker: a trigger with the current
/// choice over a `captures-menu` popover of radio rows. `preview` adds the
/// Text style sample in front of each label.
pub(crate) struct Choice {
    pub(crate) root: gtk::Box,
    trigger: gtk::Button,
    caption: gtk::Label,
    name: RefCell<String>,
    options: RefCell<Vec<(String, String)>>,
    /// Shipping `CustomSelect` `<small>` copy under each option, by index.
    descriptions: RefCell<Vec<String>>,
    /// Trigger text that differs from the option label (`triggerLabel`).
    caption_override: RefCell<Option<String>>,
    value: RefCell<String>,
    preview: Option<gtk::Label>,
    /// The listbox uses the media palette (`.custom-select-listbox-glass`).
    glass: std::cell::Cell<bool>,
    changed: Rc<dyn Fn(String)>,
}

pub(crate) fn style_preview(preset: &str) -> gtk::Label {
    let sample = gtk::Label::new(Some("Text"));
    sample.add_css_class("text-style-preview");
    sample.add_css_class(&format!("style-{preset}"));
    sample.set_width_chars(1);
    sample.set_size_request(72, 24);
    sample.set_valign(gtk::Align::Center);
    sample.set_accessible_role(gtk::AccessibleRole::Presentation);
    sample
}

impl Choice {
    pub(crate) fn new(
        legend: Option<&gtk::Label>,
        name: &str,
        preview: bool,
        changed: impl Fn(String) + 'static,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        if let Some(legend) = legend {
            root.append(legend);
        }
        let trigger = gtk::Button::new();
        crate::widgets::action_cursor(&trigger);
        trigger.add_css_class(if preview {
            "text-style-trigger"
        } else {
            "select-trigger"
        });
        trigger.set_accessible_role(gtk::AccessibleRole::ComboBox);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let preview = preview.then(|| {
            let sample = style_preview("standard");
            row.append(&sample);
            sample
        });
        let caption = gtk::Label::new(None);
        caption.set_xalign(0.);
        caption.set_hexpand(true);
        caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&caption);
        row.append(&Icon::new("chevron-down", 14).with_stroke(1.9));
        trigger.set_child(Some(&row));
        root.append(&trigger);
        let choice = Rc::new(Choice {
            root,
            trigger,
            caption,
            name: RefCell::new(name.to_owned()),
            options: RefCell::default(),
            descriptions: RefCell::default(),
            caption_override: RefCell::default(),
            value: RefCell::default(),
            preview,
            glass: std::cell::Cell::new(false),
            changed: Rc::new(changed),
        });
        let weak = Rc::downgrade(&choice);
        choice.trigger.connect_clicked(move |_| {
            if let Some(choice) = weak.upgrade() {
                choice.open();
            }
        });
        choice
    }

    /// Open the listbox on the media palette, for selects on glass.
    pub(crate) fn set_glass(&self) {
        self.glass.set(true);
    }

    pub(crate) fn set_options(&self, options: Vec<(String, String)>) {
        self.descriptions.borrow_mut().clear();
        *self.options.borrow_mut() = options;
        let value = self.value.borrow().clone();
        self.set(&value);
    }

    /// Options with a description line each (`[id, label, description]`).
    pub(crate) fn set_described(&self, options: Vec<[String; 3]>) {
        let (options, descriptions) = options
            .into_iter()
            .map(|[id, label, description]| ((id, label), description))
            .unzip();
        *self.options.borrow_mut() = options;
        *self.descriptions.borrow_mut() = descriptions;
        let value = self.value.borrow().clone();
        self.set(&value);
    }

    /// Show `caption` on the trigger instead of the chosen label.
    pub(crate) fn set_caption(&self, caption: Option<&str>) {
        *self.caption_override.borrow_mut() = caption.map(str::to_owned);
        let value = self.value.borrow().clone();
        self.set(&value);
    }

    pub(crate) fn set_sensitive(&self, on: bool) {
        self.trigger.set_sensitive(on);
    }

    pub(crate) fn set_tooltip(&self, text: &str) {
        self.trigger.set_tooltip_text(Some(text));
    }

    /// Stretch the trigger to its row's height (an inline suffix select).
    pub(crate) fn fill_height(&self) {
        self.trigger.set_vexpand(true);
        // Explicit, so the expansion stops here instead of growing the row.
        self.root.set_vexpand(false);
        self.caption.set_hexpand(false);
    }

    pub(crate) fn set(&self, value: &str) {
        *self.value.borrow_mut() = value.to_owned();
        let label = self
            .options
            .borrow()
            .iter()
            .find(|(id, _)| id == value)
            .map_or_else(|| value.to_owned(), |(_, label)| label.clone());
        self.caption
            .set_label(self.caption_override.borrow().as_deref().unwrap_or(&label));
        if let Some(sample) = &self.preview {
            for class in sample.css_classes() {
                if class.starts_with("style-") {
                    sample.remove_css_class(&class);
                }
            }
            sample.add_css_class(&format!("style-{value}"));
        }
        let name = format!("{}: {label}", self.name.borrow());
        self.trigger
            .update_property(&[gtk::accessible::Property::Label(&name)]);
    }

    fn open(self: &Rc<Self>) {
        let popover = gtk::Popover::new();
        popover.add_css_class("captures-menu");
        popover.add_css_class("choice-menu");
        if self.glass.get() {
            popover.add_css_class("glass");
        }
        popover.set_has_arrow(false);
        // Like shipping's listbox, open upward from the lower half of the window.
        let lower = self.trigger.root().is_some_and(|root| {
            self.trigger
                .compute_point(&root, &gtk::graphene::Point::new(0., 0.))
                .is_some_and(|point| f64::from(point.y()) > f64::from(root.height()) / 2.)
        });
        popover.set_position(if lower {
            gtk::PositionType::Top
        } else {
            gtk::PositionType::Bottom
        });
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.set_accessible_role(gtk::AccessibleRole::Menu);
        list.update_property(&[gtk::accessible::Property::Label(&self.name.borrow())]);
        let width = self.trigger.width();
        let current = self.value.borrow().clone();
        let mut focus = None;
        let descriptions = self.descriptions.borrow();
        for (index, (id, label)) in self.options.borrow().iter().enumerate() {
            let option = gtk::Button::new();
            crate::widgets::action_cursor(&option);
            option.add_css_class("choice-option");
            option.set_accessible_role(gtk::AccessibleRole::MenuItemRadio);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            if self.preview.is_some() {
                row.append(&style_preview(id));
            }
            let caption = gtk::Label::new(Some(label));
            caption.set_xalign(0.);
            let description = descriptions.get(index).filter(|text| !text.is_empty());
            if let Some(text) = description {
                // `.custom-select-option-copy`: the label over its `<small>`.
                let copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
                copy.append(&caption);
                let small = gtk::Label::new(Some(text));
                small.add_css_class("choice-description");
                small.set_xalign(0.);
                small.set_wrap(true);
                small.set_max_width_chars(40);
                copy.append(&small);
                row.append(&copy);
            } else {
                row.append(&caption);
            }
            option.set_child(Some(&row));
            option.set_size_request(width - 8, -1);
            option.update_property(&[gtk::accessible::Property::Label(label)]);
            if let Some(text) = description {
                option.update_property(&[gtk::accessible::Property::Description(text)]);
            }
            let active = *id == current;
            option.update_state(&[gtk::accessible::State::Checked(if active {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            })]);
            if active {
                option.add_css_class("active");
                focus = Some(option.clone());
            }
            let weak = Rc::downgrade(self);
            let id = id.clone();
            let popover_weak = popover.downgrade();
            option.connect_clicked(move |_| {
                if let Some(popover) = popover_weak.upgrade() {
                    popover.popdown();
                }
                if let Some(choice) = weak.upgrade() {
                    choice.set(&id);
                    (choice.changed)(id.clone());
                }
            });
            list.append(&option);
        }
        popover.set_child(Some(&list));
        popover.set_parent(&self.trigger);
        self.trigger.add_css_class("open");
        self.trigger
            .update_state(&[gtk::accessible::State::Expanded(Some(true))]);
        let trigger = self.trigger.clone();
        popover.connect_closed(move |popover| {
            trigger.remove_css_class("open");
            trigger.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
            let popover = popover.clone();
            gtk::glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
        if let Some(focus) = focus {
            focus.grab_focus();
        }
    }
}

/// One drop-shadow change: the toggle, or a style value (`live` when it
/// comes from a slider drag that should fold into one undo step).
pub(crate) enum ShadowEdit {
    Enabled(bool),
    Style {
        patch: DropShadowStylePatch,
        key: &'static str,
    },
}

/// Shipping `DropShadowFields`.
pub(crate) struct DropShadow {
    pub(crate) root: gtk::Box,
    check: gtk::ToggleButton,
    settings: gtk::Box,
    color: Rc<ColorField>,
    opacity: RangeSlider,
    blur: RangeSlider,
    x: NumberField,
    y: NumberField,
    syncing: Rc<Cell<bool>>,
}

impl DropShadow {
    pub(crate) fn new(tokens: &Rc<Tokens>, edit: impl Fn(ShadowEdit) + 'static) -> Self {
        let edit: Rc<dyn Fn(ShadowEdit)> = Rc::new(edit);
        let syncing = Rc::new(Cell::new(false));
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let check = check_row("Drop shadow", false);
        root.append(&check);
        let settings = gtk::Box::new(gtk::Orientation::Vertical, 12);
        settings.add_css_class("drop-shadow-settings");
        let style =
            |patch: DropShadowStylePatch, key: &'static str| ShadowEdit::Style { patch, key };
        let send = {
            let edit = edit.clone();
            let syncing = syncing.clone();
            move |change: ShadowEdit| {
                if !syncing.get() {
                    edit(change);
                }
            }
        };
        let send = Rc::new(send);
        let color = {
            let send = send.clone();
            ColorField::new(tokens, colors::SHADOW_COLOR, move |color| {
                send(style(
                    DropShadowStylePatch {
                        color: Some(color),
                        ..Default::default()
                    },
                    "shadow-color",
                ));
            })
        };
        settings.append(&color.root);
        let opacity = RangeSlider::new("Opacity", "Shadow opacity", (0., 100.), 45., &[], |v| {
            format!("{}%", v.round())
        });
        {
            let send = send.clone();
            opacity.scale.connect_value_changed(move |scale| {
                send(style(
                    DropShadowStylePatch {
                        opacity: Some(scale.value().round()),
                        ..Default::default()
                    },
                    "shadow-opacity",
                ));
            });
        }
        settings.append(&opacity.root);
        let blur = RangeSlider::new("Blur", "Shadow blur", (0., SHADOW_BLUR_MAX), 6., &[], |v| {
            format!("{} px", v.round())
        });
        {
            let send = send.clone();
            blur.scale.connect_value_changed(move |scale| {
                send(style(
                    DropShadowStylePatch {
                        blur: Some(scale.value().round()),
                        ..Default::default()
                    },
                    "shadow-blur",
                ));
            });
        }
        settings.append(&blur.root);
        let range = (-SHADOW_OFFSET_MAX, SHADOW_OFFSET_MAX);
        let x = {
            let send = send.clone();
            NumberField::new("Shadow X offset", range, move |value| {
                send(style(
                    DropShadowStylePatch {
                        offset_x: Some(value.round()),
                        ..Default::default()
                    },
                    "shadow-x",
                ));
            })
        };
        let y = {
            let send = send.clone();
            NumberField::new("Shadow Y offset", range, move |value| {
                send(style(
                    DropShadowStylePatch {
                        offset_y: Some(value.round()),
                        ..Default::default()
                    },
                    "shadow-y",
                ));
            })
        };
        settings.append(&pair(
            &field("X offset", &x.root),
            &field("Y offset", &y.root),
        ));
        root.append(&settings);
        {
            let send = send.clone();
            check.connect_toggled(move |check| send(ShadowEdit::Enabled(check.is_active())));
        }
        DropShadow {
            root,
            check,
            settings,
            color,
            opacity,
            blur,
            x,
            y,
            syncing,
        }
    }

    pub(crate) fn sync(&self, enabled: bool, style: &DropShadowStyle, force: bool) {
        self.syncing.set(true);
        self.check.set_active(enabled);
        self.settings.set_visible(enabled);
        self.color.set(&style.color);
        self.opacity.set(style.opacity, force);
        self.blur.set(style.blur, force);
        self.x.set(style.offset_x, force);
        self.y.set(style.offset_y, force);
        self.syncing.set(false);
    }
}
