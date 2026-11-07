//! The sidebar's Tool properties (`.screenshot-properties`): a heading for
//! the active tool or selection over its property sections, built from real
//! GTK controls (GtkScale, GtkCheckButton, toggle buttons) restyled with the
//! shipping `RangeSlider`, check row and format-button tokens, so AT-SPI sees
//! sliders, check boxes and pressed buttons.
use std::cell::Cell;
use std::rc::Rc;

use captures_app::editor_chrome::eraser;
use captures_app::editor_image_background::BrushMode;
use gtk::prelude::*;

use super::controls::{Choice, NumberField, field, pair};
use crate::widgets::Icon;

/// Shipping `RemoveBackgroundMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EraserMode {
    Wand,
    Brush(BrushMode),
}

const MODES: [(EraserMode, &str); 3] = [
    (EraserMode::Wand, "Wand"),
    (EraserMode::Brush(BrushMode::Erase), "Erase"),
    (EraserMode::Brush(BrushMode::Restore), "Restore"),
];

pub(crate) fn text(value: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(value));
    label.add_css_class(class);
    label.set_xalign(0.);
    label.set_wrap(true);
    label
}

/// Shipping `RangeSlider` inside its property `<label>`: caption, value
/// readout, accent-filled track and the mark labels under it.
pub(crate) struct RangeSlider {
    pub(crate) root: gtk::Box,
    pub(crate) scale: gtk::Scale,
}

impl RangeSlider {
    pub(crate) fn new(
        caption: &str,
        name: &str,
        range: (f64, f64),
        value: f64,
        marks: &[(f64, &str)],
        readout: fn(f64) -> String,
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("property-field");
        root.append(&text(caption, "property-caption"));
        let value_label = text(&readout(value), "range-value");
        value_label.set_xalign(1.);
        root.append(&value_label);
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, range.0, range.1, 1.);
        scale.add_css_class("range-slider");
        scale.set_value(value);
        scale.set_draw_value(false);
        scale.set_round_digits(0);
        for (at, label) in marks {
            scale.add_mark(*at, gtk::PositionType::Bottom, Some(label));
        }
        scale.update_property(&[
            gtk::accessible::Property::Label(name),
            gtk::accessible::Property::ValueText(&readout(value)),
        ]);
        scale.connect_value_changed(move |scale| {
            let text = readout(scale.value());
            value_label.set_label(&text);
            scale.update_property(&[gtk::accessible::Property::ValueText(&text)]);
        });
        root.append(&scale);
        RangeSlider { root, scale }
    }

    /// Show `value`; with `force` false a slider the user holds keeps its own.
    pub(crate) fn set(&self, value: f64, force: bool) {
        if force || !self.scale.has_focus() {
            self.scale.set_value(value);
        }
    }

    pub(crate) fn value(&self) -> f64 {
        self.scale.value().round()
    }
}

/// Shipping `.screenshot-check-row`: a real check box drawn as the token box
/// with the shipping check glyph.
pub(crate) fn check_row(caption: &str, active: bool) -> gtk::ToggleButton {
    // GtkCheckButton only takes a custom child from GTK 4.8, so this is a
    // toggle that AT-SPI reads as a check box with a checked state.
    let check = gtk::ToggleButton::builder()
        .accessible_role(gtk::AccessibleRole::Checkbox)
        .build();
    check.add_css_class("check-row");
    let checked = |active: bool| {
        gtk::accessible::State::Checked(if active {
            gtk::AccessibleTristate::True
        } else {
            gtk::AccessibleTristate::False
        })
    };
    check.update_state(&[checked(active)]);
    check.set_active(active);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    indicator.add_css_class("check-box");
    indicator.set_valign(gtk::Align::Center);
    let glyph = Icon::new("check", 11).with_stroke(2.4);
    glyph.set_halign(gtk::Align::Center);
    glyph.set_valign(gtk::Align::Center);
    indicator.set_hexpand(false);
    glyph.set_opacity(if active { 1. } else { 0. });
    indicator.append(&glyph);
    row.append(&indicator);
    let label = text(caption, "check-caption");
    label.set_hexpand(true);
    row.append(&label);
    check.set_child(Some(&row));
    check.update_property(&[gtk::accessible::Property::Label(caption)]);
    check.connect_toggled(move |check| {
        glyph.set_opacity(if check.is_active() { 1. } else { 0. });
        check.update_state(&[checked(check.is_active())]);
    });
    check
}

/// The Eraser section (`tool === "remove-bg"`).
pub(super) struct EraserSection {
    pub(super) root: gtk::Box,
    modes: Vec<(EraserMode, gtk::Button)>,
    wand: gtk::Box,
    brush: gtk::Box,
    pub(super) tolerance: RangeSlider,
    pub(super) contiguous: gtk::ToggleButton,
    pub(super) size: RangeSlider,
    pub(super) softness: RangeSlider,
    brush_hint: gtk::Label,
    mode: Rc<Cell<EraserMode>>,
}

impl EraserSection {
    fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("property-section");
        root.append(&text(eraser::INTRO, "property-hint"));
        let group = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        group.set_homogeneous(true);
        group.update_property(&[gtk::accessible::Property::Label("Eraser mode")]);
        let modes: Vec<_> = MODES
            .iter()
            .map(|(mode, label)| {
                let button = gtk::Button::with_label(label);
                button.add_css_class("format-button");
                group.append(&button);
                (*mode, button)
            })
            .collect();
        root.append(&group);

        let wand = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let tolerance = RangeSlider::new(
            eraser::TOLERANCE,
            eraser::TOLERANCE_LABEL,
            eraser::TOLERANCE_RANGE,
            36.,
            &eraser::TOLERANCE_MARKS,
            eraser::tolerance_text,
        );
        wand.append(&tolerance.root);
        let contiguous = check_row(eraser::CONTIGUOUS, true);
        wand.append(&contiguous);
        let wand_hint = text(eraser::wand_hint(true), "property-hint");
        wand.append(&wand_hint);
        contiguous.connect_toggled(move |check| {
            wand_hint.set_label(eraser::wand_hint(check.is_active()));
        });
        root.append(&wand);

        let brush = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let size = RangeSlider::new(
            eraser::SIZE,
            eraser::SIZE_LABEL,
            eraser::SIZE_RANGE,
            28.,
            &eraser::SIZE_MARKS,
            eraser::size_text,
        );
        brush.append(&size.root);
        let softness = RangeSlider::new(
            eraser::SOFTNESS,
            eraser::SOFTNESS_LABEL,
            eraser::SOFTNESS_RANGE,
            18.,
            &eraser::SOFTNESS_MARKS,
            eraser::softness_text,
        );
        brush.append(&softness.root);
        let brush_hint = text(eraser::ERASE_HINT, "property-hint");
        brush.append(&brush_hint);
        root.append(&brush);

        let section = EraserSection {
            root,
            modes,
            wand,
            brush,
            tolerance,
            contiguous,
            size,
            softness,
            brush_hint,
            mode: Rc::new(Cell::new(EraserMode::Wand)),
        };
        section.show_mode(EraserMode::Wand);
        section
    }

    pub(super) fn mode(&self) -> EraserMode {
        self.mode.get()
    }

    fn show_mode(&self, mode: EraserMode) {
        self.mode.set(mode);
        for (each, button) in &self.modes {
            let active = *each == mode;
            if active {
                button.add_css_class("active");
            } else {
                button.remove_css_class("active");
            }
            button.update_state(&[gtk::accessible::State::Pressed(if active {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            })]);
        }
        self.wand.set_visible(mode == EraserMode::Wand);
        self.brush.set_visible(mode != EraserMode::Wand);
        self.brush_hint.set_label(match mode {
            EraserMode::Brush(BrushMode::Restore) => eraser::RESTORE_HINT,
            _ => eraser::ERASE_HINT,
        });
    }

    /// Mode buttons switch sections; `changed` lets the canvas swap cursors.
    pub(super) fn connect_mode(self: &Rc<Self>, changed: impl Fn() + 'static) {
        let changed = Rc::new(changed);
        for (mode, button) in &self.modes {
            let weak = Rc::downgrade(self);
            let mode = *mode;
            let changed = changed.clone();
            button.connect_clicked(move |_| {
                if let Some(section) = weak.upgrade() {
                    section.show_mode(mode);
                    changed();
                }
            });
        }
    }
}

/// Shipping's Aspect ratio options: `(value, label, width / height)`.
const CROP_ASPECTS: [(&str, &str, Option<f64>); 5] = [
    ("free", "Free", None),
    ("1:1", "1 : 1", Some(1.)),
    ("4:3", "4 : 3", Some(4. / 3.)),
    ("3:2", "3 : 2", Some(3. / 2.)),
    ("16:9", "16 : 9", Some(16. / 9.)),
];
const CROP_KEEP_HINT: &str = "Hold Shift while dragging to keep this aspect ratio.";
const CROP_DRAG_HINT: &str = "Drag over the area you want to keep. Start from outside the canvas \
                              to crop to an edge. Hold Shift to lock the current aspect ratio.";

/// The Crop section (`tool === "crop"`): the Aspect ratio select, then the
/// staged crop's read-only Width and Height with Clear and Apply crop, or
/// the drag hint while nothing is staged.
pub(super) struct CropSection {
    pub(super) root: gtk::Box,
    aspect: Rc<Cell<Option<f64>>>,
    /// The select's click handler only holds it weakly.
    _aspect_choice: Rc<Choice>,
    staged: gtk::Box,
    width: NumberField,
    height: NumberField,
    pub(super) clear: gtk::Button,
    pub(super) apply: gtk::Button,
    hint: gtk::Label,
}

impl CropSection {
    fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("property-section");
        let aspect = Rc::new(Cell::new(None));
        let choice = {
            let aspect = aspect.clone();
            Choice::new(
                Some(&text("Aspect ratio", "property-caption")),
                "Aspect ratio",
                false,
                move |value| {
                    let ratio = CROP_ASPECTS.iter().find(|(id, ..)| *id == value);
                    aspect.set(ratio.and_then(|(.., ratio)| *ratio));
                },
            )
        };
        choice.set_options(
            CROP_ASPECTS
                .iter()
                .map(|(id, label, _)| ((*id).to_owned(), (*label).to_owned()))
                .collect(),
        );
        choice.set("free");
        root.append(&choice.root);
        let staged = gtk::Box::new(gtk::Orientation::Vertical, 12);
        // Shipping's read-only `<input>`s: values a reader can review, not edit.
        let read_only = |name: &str| {
            let number = NumberField::new(name, (0., f64::from(u32::MAX)), |_| {});
            number.spin.set_editable(false);
            number.spin.set_can_focus(true);
            if let Some(steppers) = number.root.last_child() {
                steppers.set_visible(false);
            }
            number
        };
        let width = read_only("Crop width");
        let height = read_only("Crop height");
        staged.append(&pair(
            &field("Width", &width.root),
            &field("Height", &height.root),
        ));
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.set_homogeneous(true);
        let clear = gtk::Button::with_label("Clear");
        clear.add_css_class("property-action");
        let apply = gtk::Button::with_label("Apply crop");
        apply.add_css_class("property-action");
        apply.add_css_class("primary");
        apply.add_css_class("cta-pulse");
        actions.append(&clear);
        actions.append(&apply);
        staged.append(&actions);
        root.append(&staged);
        let hint = text(CROP_DRAG_HINT, "property-hint");
        root.append(&hint);
        let section = CropSection {
            root,
            aspect,
            _aspect_choice: choice,
            staged,
            width,
            height,
            clear,
            apply,
            hint,
        };
        section.sync(None);
        section
    }

    /// The chosen aspect ratio (width / height), or a free crop.
    pub(super) fn aspect(&self) -> Option<f64> {
        self.aspect.get()
    }

    /// Show the staged crop's size and actions, or the drag hint.
    pub(super) fn sync(&self, crop: Option<captures_app::editor::Rect>) {
        self.staged.set_visible(crop.is_some());
        self.hint.set_label(if crop.is_some() {
            CROP_KEEP_HINT
        } else {
            CROP_DRAG_HINT
        });
        if let Some(crop) = crop {
            self.width.set(crop.width, true);
            self.height.set(crop.height, true);
        }
    }
}

/// `.screenshot-properties`: heading plus the active sections.
pub(super) struct Properties {
    pub(super) root: gtk::ScrolledWindow,
    heading: gtk::Box,
    title: gtk::Label,
    pub(super) eraser: Rc<EraserSection>,
    pub(super) crop: CropSection,
    pub(super) inspector: super::inspector::Inspector,
}

impl Properties {
    pub(super) fn new(tokens: &Rc<crate::tokens::Tokens>) -> Self {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("editor-properties");
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        heading.add_css_class("editor-properties-heading");
        let title = text("", "editor-properties-title");
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        heading.append(&title);
        content.append(&heading);
        let eraser = Rc::new(EraserSection::new());
        content.append(&eraser.root);
        let crop = CropSection::new();
        content.append(&crop.root);
        let inspector = super::inspector::Inspector::new(tokens, &content);
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build();
        root.add_css_class("editor-properties-scroller");
        root.update_property(&[gtk::accessible::Property::Label("Tool properties")]);
        Properties {
            root,
            heading,
            title,
            eraser,
            crop,
            inspector,
        }
    }

    /// Show the heading and sections for `tool` (and whether a layer is
    /// selected). Select alone shows nothing: the rail already says so.
    pub(super) fn show(
        &self,
        tool: &str,
        selection: Option<&captures_app::editor::Element>,
        fonts: &std::collections::BTreeMap<String, String>,
    ) {
        // Shipping shows the selected layer's sections under any tool; only
        // the heading prefers the tool name outside Select.
        self.inspector.show(selection, fonts);
        // Shipping: `!selected && tool !== crop/select/remove-bg`.
        let drawing = selection.is_none() && !matches!(tool, "v" | "c" | "b");
        self.inspector.defaults.show(drawing.then_some(tool));
        let title = match selection.map(captures_app::editor_chrome::element_label) {
            Some(label) if tool == "v" => Some(label),
            _ if tool == "v" => None,
            _ => Some(captures_app::editor_chrome::tool_label(tool).to_owned()),
        };
        self.heading.set_visible(title.is_some());
        self.title.set_label(title.as_deref().unwrap_or_default());
        self.eraser.root.set_visible(tool == "b");
        self.crop.root.set_visible(tool == "c");
    }
}

/// Lay Layers over Properties as shipping's sidebar grid
/// (`minmax(188px, 40%) minmax(0, 1fr)`).
pub(super) fn sidebar(
    layers: &impl IsA<gtk::Widget>,
    properties: &impl IsA<gtk::Widget>,
) -> crate::widgets::SplitColumn {
    crate::widgets::SplitColumn::new(layers, properties, 188)
}
