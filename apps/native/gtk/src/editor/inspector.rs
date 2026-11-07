//! Shipping's selected-layer property sections (`ScreenshotEditor.tsx`):
//! shape and freehand stroke, fill, opacity, shadow and curve; text style,
//! content, font, size, format, colors and shadow; image size and position.
//! Every control sends one [`Edit`] through a [`Sink`], which the editor turns
//! into a session request (live edits fold into one undo step per field).
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use captures_app::editor::{
    AnnotationStylePatch, Element, ElementStyle, LayerEdit, OptionalNullable, ShapeElement,
    TextElement,
};
use captures_app::editor_canvas::{self, CurveEdit};
use captures_app::editor_chrome::{colors, text_format};
use captures_app::editor_session::TextPatch;
use captures_app::editor_text::{self, TEXT_STYLE_PRESETS};
use gtk::prelude::*;

use super::controls::{
    Choice, ColorField, DropShadow, NumberField, ShadowEdit, field, pair, pressed,
};
use super::properties::{RangeSlider, check_row, text};
use crate::tokens::Tokens;
use crate::widgets::Icon;

/// The plate a Box preset or the Text background check adds.
const TEXT_PLATE: &str = "#111318";

/// One property change for the selected layer. `live` names the field when
/// consecutive changes (slider drags, typing) should be one undo step.
pub(super) enum Edit {
    Layer {
        edit: LayerEdit,
        live: Option<&'static str>,
    },
    Text {
        patch: TextPatch,
        live: Option<&'static str>,
    },
}

type Send = Box<dyn Fn(Edit)>;

/// Where controls send edits; quiet while the sections are being synced.
#[derive(Default)]
pub(super) struct Sink {
    syncing: Cell<bool>,
    send: RefCell<Option<Send>>,
}

impl Sink {
    pub(super) fn connect(&self, send: impl Fn(Edit) + 'static) {
        *self.send.borrow_mut() = Some(Box::new(send));
    }

    fn send(&self, edit: Edit) {
        if !self.syncing.get()
            && let Some(send) = self.send.borrow().as_ref()
        {
            send(edit);
        }
    }

    fn style(&self, patch: AnnotationStylePatch, live: Option<&'static str>) {
        self.send(Edit::Layer {
            edit: LayerEdit::AnnotationStyle { patch },
            live,
        });
    }
}

fn shadow_patch(change: ShadowEdit) -> (AnnotationStylePatch, Option<&'static str>) {
    match change {
        ShadowEdit::Enabled(on) => (
            AnnotationStylePatch {
                drop_shadow: Some(on),
                ..Default::default()
            },
            None,
        ),
        ShadowEdit::Style { patch, key } => (
            AnnotationStylePatch {
                drop_shadow_style: Some(patch),
                ..Default::default()
            },
            Some(key),
        ),
    }
}

/// Selected shape or freehand path.
struct ShapeSection {
    root: gtk::Box,
    stroke_check: gtk::ToggleButton,
    stroke: gtk::Box,
    color: Rc<ColorField>,
    width: RangeSlider,
    opacity: RangeSlider,
    shadow: DropShadow,
    fill_check: gtk::ToggleButton,
    fill: Rc<ColorField>,
    curve: RangeSlider,
    straighten: gtk::Button,
    curve_hint: gtk::Label,
    /// The stroke color, which Filled shape copies into the fill.
    current_color: Rc<RefCell<String>>,
}

impl ShapeSection {
    fn new(tokens: &Rc<Tokens>, sink: &Rc<Sink>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("property-section");
        let stroke_check = check_row("Stroke", true);
        {
            let sink = sink.clone();
            stroke_check.connect_toggled(move |check| {
                sink.style(
                    AnnotationStylePatch {
                        stroke_enabled: Some(check.is_active()),
                        ..Default::default()
                    },
                    None,
                );
            });
        }
        root.append(&stroke_check);
        let stroke = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let color = {
            let sink = sink.clone();
            ColorField::new(tokens, colors::STROKE_COLOR, move |color| {
                sink.style(
                    AnnotationStylePatch {
                        color: Some(color),
                        ..Default::default()
                    },
                    None,
                );
            })
        };
        stroke.append(&color.root);
        let width = RangeSlider::new("Stroke width", "Stroke width", (2., 40.), 8., &[], |v| {
            format!("{} px", v.round())
        });
        {
            let sink = sink.clone();
            width.scale.connect_value_changed(move |scale| {
                sink.style(
                    AnnotationStylePatch {
                        stroke_width: Some(scale.value().round()),
                        ..Default::default()
                    },
                    Some("stroke-width"),
                );
            });
        }
        stroke.append(&width.root);
        root.append(&stroke);
        let opacity = opacity_slider(sink);
        root.append(&opacity.root);
        let shadow = {
            let sink = sink.clone();
            DropShadow::new(tokens, move |change| {
                let (patch, live) = shadow_patch(change);
                sink.style(patch, live);
            })
        };
        root.append(&shadow.root);
        let current_color = Rc::new(RefCell::new(String::new()));
        let fill_check = check_row("Filled shape", false);
        {
            let sink = sink.clone();
            let current = current_color.clone();
            fill_check.connect_toggled(move |check| {
                let fill = if check.is_active() {
                    OptionalNullable::Value(current.borrow().clone())
                } else {
                    OptionalNullable::Null
                };
                sink.style(
                    AnnotationStylePatch {
                        fill,
                        ..Default::default()
                    },
                    None,
                );
            });
        }
        root.append(&fill_check);
        let fill = {
            let sink = sink.clone();
            ColorField::new(tokens, colors::FILL_COLOR, move |fill| {
                sink.style(
                    AnnotationStylePatch {
                        fill: OptionalNullable::Value(fill),
                        ..Default::default()
                    },
                    None,
                );
            })
        };
        root.append(&fill.root);
        let curve = RangeSlider::new(
            "Curve",
            "Curve",
            (-100., 100.),
            0.,
            &[(-100., "Left"), (0., "Straight"), (100., "Right")],
            |v| format!("{}%", v.round()),
        );
        {
            let sink = sink.clone();
            curve.scale.connect_value_changed(move |scale| {
                sink.send(Edit::Layer {
                    edit: LayerEdit::Curve {
                        edit: CurveEdit::Bend {
                            bend: scale.value().round() / 100.,
                        },
                    },
                    live: Some("curve"),
                });
            });
        }
        root.append(&curve.root);
        let straighten = gtk::Button::with_label("Straighten line");
        straighten.add_css_class("ui-btn");
        straighten.set_halign(gtk::Align::Start);
        {
            let sink = sink.clone();
            straighten.connect_clicked(move |_| {
                sink.send(Edit::Layer {
                    edit: LayerEdit::Curve {
                        edit: CurveEdit::Straighten,
                    },
                    live: None,
                });
            });
        }
        root.append(&straighten);
        let curve_hint = text(
            "Drag the curve dots to reshape. Double-click the path to add more points; \
             double-click a point to remove it.",
            "property-hint",
        );
        root.append(&curve_hint);
        ShapeSection {
            root,
            stroke_check,
            stroke,
            color,
            width,
            opacity,
            shadow,
            fill_check,
            fill,
            curve,
            straighten,
            curve_hint,
            current_color,
        }
    }

    fn sync(&self, style: &ElementStyle, opacity: f64, shape: Option<&ShapeElement>, force: bool) {
        let closed = shape.is_some_and(|s| !editor_canvas::is_curveable(s));
        self.stroke_check.set_visible(closed);
        self.stroke_check.set_active(style.has_stroke());
        self.stroke.set_visible(!closed || style.has_stroke());
        self.color.set_label(if closed {
            colors::STROKE_COLOR
        } else {
            colors::COLOR
        });
        self.color.set(&style.color);
        *self.current_color.borrow_mut() = style.color.clone();
        self.width.set(style.stroke_width.round(), force);
        self.opacity.set(opacity.round(), force);
        self.shadow.sync(
            style.has_drop_shadow(),
            &style.resolved_drop_shadow_style(),
            force,
        );
        self.fill_check.set_visible(closed);
        self.fill_check.set_active(style.fill.is_some());
        self.fill.root.set_visible(closed && style.fill.is_some());
        if let Some(fill) = &style.fill {
            self.fill.set(fill);
        }
        let curveable = shape.filter(|s| editor_canvas::is_curveable(s));
        let slider = curveable.is_some_and(|s| s.controls.len() <= 1);
        self.curve.root.set_visible(slider);
        if let Some(shape) = curveable {
            self.curve.set(
                (editor_canvas::curve_bend_amount(shape) * 100.).round(),
                force,
            );
            self.straighten
                .set_label(editor_canvas::straighten_label(shape));
        }
        self.straighten.set_visible(curveable.is_some() && !slider);
        self.curve_hint.set_visible(curveable.is_some());
    }
}

fn opacity_slider(sink: &Rc<Sink>) -> RangeSlider {
    let opacity = RangeSlider::new("Opacity", "Opacity", (0., 100.), 100., &[], |v| {
        format!("{}%", v.round())
    });
    let sink = sink.clone();
    opacity.scale.connect_value_changed(move |scale| {
        sink.send(Edit::Layer {
            edit: LayerEdit::Opacity {
                opacity: scale.value().round(),
            },
            live: Some("opacity"),
        });
    });
    opacity
}

/// Selected text label.
struct TextSection {
    root: gtk::Box,
    style: Rc<Choice>,
    content: gtk::TextView,
    font: Rc<Choice>,
    size: NumberField,
    bold: gtk::Button,
    italic: gtk::Button,
    align: Vec<(&'static str, gtk::Button)>,
    color: Rc<ColorField>,
    plate_check: gtk::ToggleButton,
    plate: Rc<ColorField>,
    shadow: DropShadow,
    /// The selected label, for toggles and presets that read its state.
    current: Rc<RefCell<Option<TextElement>>>,
}

fn format_button(child: &impl IsA<gtk::Widget>, name: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("format-button");
    button.set_child(Some(child));
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button
}

impl TextSection {
    fn new(tokens: &Rc<Tokens>, sink: &Rc<Sink>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("property-section");
        let current: Rc<RefCell<Option<TextElement>>> = Rc::default();
        let patch = |sink: &Rc<Sink>, patch: TextPatch, live: Option<&'static str>| {
            sink.send(Edit::Text { patch, live });
        };
        let style = {
            let sink = sink.clone();
            let current = current.clone();
            Choice::new(
                Some(&text("Text style", "property-caption")),
                "Text style",
                true,
                move |id| {
                    let Some(preset) = TEXT_STYLE_PRESETS.iter().find(|p| p.id == id) else {
                        return;
                    };
                    // Presets keep an existing plate color.
                    let existing = current.borrow().as_ref().and_then(|t| t.background.clone());
                    let background = match preset.background {
                        Some(color) => {
                            OptionalNullable::Value(existing.unwrap_or_else(|| color.to_owned()))
                        }
                        None => OptionalNullable::Null,
                    };
                    patch(
                        &sink,
                        TextPatch {
                            font_family: Some(preset.font_family.to_owned()),
                            background,
                            outlined: Some(preset.outlined),
                            rounded_background: Some(preset.rounded_background),
                            ..Default::default()
                        },
                        None,
                    );
                },
            )
        };
        style.set_options(
            TEXT_STYLE_PRESETS
                .iter()
                .map(|p| (p.id.to_owned(), p.label.to_owned()))
                .collect(),
        );
        root.append(&style.root);

        let content = gtk::TextView::new();
        content.add_css_class("property-textarea");
        content.set_wrap_mode(gtk::WrapMode::WordChar);
        content.set_size_request(-1, 88);
        content.update_property(&[gtk::accessible::Property::Label("Text")]);
        {
            let sink = sink.clone();
            content.buffer().connect_changed(move |buffer| {
                let value = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();
                patch(
                    &sink,
                    TextPatch {
                        text: Some(value),
                        ..Default::default()
                    },
                    Some("text"),
                );
            });
        }
        root.append(&field("Text", &content));

        let font = {
            let sink = sink.clone();
            Choice::new(None, "Font", false, move |family| {
                patch(
                    &sink,
                    TextPatch {
                        font_family: Some(family),
                        ..Default::default()
                    },
                    None,
                );
            })
        };
        let size = {
            let sink = sink.clone();
            NumberField::new("Size", (8., 512.), move |size| {
                patch(
                    &sink,
                    TextPatch {
                        font_size: Some(size.round()),
                        ..Default::default()
                    },
                    Some("font-size"),
                );
            })
        };
        root.append(&pair(
            &field("Font", &font.root),
            &field("Size", &size.root),
        ));

        let formats = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        formats.set_homogeneous(true);
        let bold_glyph = gtk::Label::new(Some("B"));
        let bold = format_button(&bold_glyph, text_format::BOLD);
        let italic_glyph = gtk::Label::new(None);
        italic_glyph.set_markup("<i>I</i>");
        let italic = format_button(&italic_glyph, text_format::ITALIC);
        for (button, toggle) in [(&bold, true), (&italic, false)] {
            let sink = sink.clone();
            let current = current.clone();
            button.connect_clicked(move |_| {
                let Some(label) = current.borrow().clone() else {
                    return;
                };
                let mut change = TextPatch::default();
                if toggle {
                    change.bold = Some(!label.bold);
                } else {
                    change.italic = Some(!label.italic);
                }
                patch(&sink, change, None);
            });
            formats.append(button);
        }
        let align: Vec<_> = text_format::ALIGN
            .iter()
            .map(|(value, name, icon)| {
                let button = format_button(
                    &Icon::new(icon, text_format::ICON as i32).with_stroke(1.8),
                    name,
                );
                let sink = sink.clone();
                button.connect_clicked(move |_| {
                    patch(
                        &sink,
                        TextPatch {
                            align: Some((*value).to_owned()),
                            ..Default::default()
                        },
                        None,
                    );
                });
                formats.append(&button);
                (*value, button)
            })
            .collect();
        root.append(&formats);

        let color = {
            let sink = sink.clone();
            ColorField::new(tokens, colors::TEXT_COLOR, move |color| {
                patch(
                    &sink,
                    TextPatch {
                        color: Some(color),
                        ..Default::default()
                    },
                    None,
                );
            })
        };
        root.append(&color.root);
        let plate_check = check_row("Text background", false);
        {
            let sink = sink.clone();
            plate_check.connect_toggled(move |check| {
                let on = check.is_active();
                let mut change = TextPatch {
                    background: if on {
                        OptionalNullable::Value(TEXT_PLATE.to_owned())
                    } else {
                        OptionalNullable::Null
                    },
                    ..Default::default()
                };
                if on {
                    change.outlined = Some(false);
                    change.rounded_background = Some(false);
                }
                patch(&sink, change, None);
            });
        }
        root.append(&plate_check);
        let plate = {
            let sink = sink.clone();
            ColorField::new(tokens, "Background color", move |color| {
                patch(
                    &sink,
                    TextPatch {
                        background: OptionalNullable::Value(color),
                        ..Default::default()
                    },
                    None,
                );
            })
        };
        root.append(&plate.root);
        let shadow = {
            let sink = sink.clone();
            DropShadow::new(tokens, move |change| {
                let (style, live) = shadow_patch(change);
                patch(
                    &sink,
                    TextPatch {
                        drop_shadow: style.drop_shadow,
                        drop_shadow_style: style.drop_shadow_style,
                        ..Default::default()
                    },
                    live,
                );
            })
        };
        root.append(&shadow.root);
        TextSection {
            root,
            style,
            content,
            font,
            size,
            bold,
            italic,
            align,
            color,
            plate_check,
            plate,
            shadow,
            current,
        }
    }

    fn sync(&self, label: &TextElement, fonts: &BTreeMap<String, String>, force: bool) {
        *self.current.borrow_mut() = Some(label.clone());
        self.style.set(editor_text::text_style_preset_id(
            &label.font_family,
            label.background.is_some(),
            label.outlined,
            label.rounded_background,
        ));
        let buffer = self.content.buffer();
        let shown = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        if (force || !self.content.has_focus()) && shown != label.text {
            buffer.set_text(&label.text);
        }
        self.font
            .set_options(editor_text::font_family_options(fonts));
        self.font.set(&label.font_family);
        self.size.set(label.font_size, force);
        pressed(&self.bold, label.bold);
        pressed(&self.italic, label.italic);
        for (value, button) in &self.align {
            pressed(button, label.align == *value);
        }
        self.color.set(&label.color);
        self.plate_check.set_active(label.background.is_some());
        self.plate.root.set_visible(label.background.is_some());
        if let Some(plate) = &label.background {
            self.plate.set(plate);
        }
        let style = editor_text::shadow_style(label, label.font_size);
        self.shadow.sync(
            style.has_drop_shadow(),
            &style.resolved_drop_shadow_style(),
            force,
        );
    }
}

/// Selected image layer: Width/Height keep the aspect; X/Y move it.
struct ImageSection {
    root: gtk::Box,
    fields: [NumberField; 4],
    hint: gtk::Label,
}

impl ImageSection {
    fn new(sink: &Rc<Sink>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("property-section");
        let make = |name: &str, key: &'static str, range: (f64, f64)| {
            let sink = sink.clone();
            NumberField::new(name, range, move |value| {
                let value = Some(value.round());
                let (mut x, mut y, mut width, mut height) = (None, None, None, None);
                match key {
                    "width" => width = value,
                    "height" => height = value,
                    "x" => x = value,
                    _ => y = value,
                }
                sink.send(Edit::Layer {
                    edit: LayerEdit::Geometry {
                        x,
                        y,
                        width,
                        height,
                    },
                    live: Some(key),
                });
            })
        };
        let size = (1., 16_384.);
        let position = (-1_000_000., 1_000_000.);
        let fields = [
            make("Layer width", "width", size),
            make("Layer height", "height", size),
            make("Layer X", "x", position),
            make("Layer Y", "y", position),
        ];
        root.append(&pair(
            &field("Width", &fields[0].root),
            &field("Height", &fields[1].root),
        ));
        root.append(&pair(
            &field("X", &fields[2].root),
            &field("Y", &fields[3].root),
        ));
        let hint = text("", "property-hint");
        root.append(&hint);
        ImageSection { root, fields, hint }
    }

    fn sync(&self, image: &captures_app::editor::ImageElement, force: bool) {
        let values = [image.width, image.height, image.base.x, image.base.y];
        for (field, value) in self.fields.iter().zip(values) {
            field.set(value, force);
            field.set_sensitive(!image.base.locked);
        }
        self.hint.set_label(if image.base.locked {
            "Unlock this layer to change size and position."
        } else {
            "Width and height stay proportional to the image."
        });
    }
}

/// The selected-layer sections, one visible at a time.
pub(super) struct Inspector {
    pub(super) sink: Rc<Sink>,
    pub(super) defaults: Rc<DefaultsSection>,
    shape: ShapeSection,
    text: TextSection,
    image: ImageSection,
    shown: RefCell<Option<String>>,
}

impl Inspector {
    pub(super) fn new(tokens: &Rc<Tokens>, content: &gtk::Box) -> Self {
        let sink = Rc::new(Sink::default());
        let inspector = Inspector {
            defaults: DefaultsSection::new(tokens),
            shape: ShapeSection::new(tokens, &sink),
            text: TextSection::new(tokens, &sink),
            image: ImageSection::new(&sink),
            sink,
            shown: RefCell::default(),
        };
        content.append(&inspector.defaults.root);
        content.append(&inspector.shape.root);
        content.append(&inspector.text.root);
        content.append(&inspector.image.root);
        inspector.show(None, &BTreeMap::new());
        inspector.defaults.show(None);
        inspector
    }

    /// Show the section for `element`. Values are forced only when the
    /// selection changes, so a control the user is in keeps its own value.
    pub(super) fn show(&self, element: Option<&Element>, fonts: &BTreeMap<String, String>) {
        let id = element.map(|e| e.base().id.clone());
        let force = *self.shown.borrow() != id;
        *self.shown.borrow_mut() = id;
        self.sink.syncing.set(true);
        self.shape.root.set_visible(matches!(
            element,
            Some(Element::Shape(_) | Element::Path(_))
        ));
        self.text
            .root
            .set_visible(matches!(element, Some(Element::Text(_))));
        self.image
            .root
            .set_visible(matches!(element, Some(Element::Image(_))));
        match element {
            Some(Element::Shape(shape)) => {
                self.shape
                    .sync(&shape.style, shape.base.opacity, Some(shape), force);
            }
            Some(Element::Path(path)) => {
                self.shape.sync(&path.style, path.base.opacity, None, force);
            }
            Some(Element::Text(label)) => self.text.sync(label, fonts, force),
            Some(Element::Image(image)) => self.image.sync(image, force),
            None => {}
        }
        self.sink.syncing.set(false);
    }
}

/// Shipping's drawing defaults (`defaultStyle`, `defaultOpacity`,
/// `defaultFontSize`, `defaultTextStyle`): window-local, never in history.
pub(super) struct Defaults {
    pub(super) style: RefCell<ElementStyle>,
    pub(super) opacity: Cell<f64>,
    pub(super) text_size: Cell<f64>,
    pub(super) text_preset: RefCell<String>,
}

fn closed_tool(tool: &str) -> bool {
    matches!(
        tool,
        "rectangle" | "ellipse" | "triangle" | "diamond" | "star"
    )
}

/// Apply a shadow change to a local style, as the session does for layers:
/// any style value turns the shadow on over the resolved defaults.
fn apply_shadow(style: &mut ElementStyle, change: ShadowEdit) {
    match change {
        ShadowEdit::Enabled(on) => style.drop_shadow = Some(on),
        ShadowEdit::Style { patch, .. } => {
            let mut shadow = style.resolved_drop_shadow_style();
            if let Some(color) = patch.color {
                shadow.color = color;
            }
            if let Some(opacity) = patch.opacity {
                shadow.opacity = opacity;
            }
            if let Some(blur) = patch.blur {
                shadow.blur = blur;
            }
            if let Some(x) = patch.offset_x {
                shadow.offset_x = x;
            }
            if let Some(y) = patch.offset_y {
                shadow.offset_y = y;
            }
            style.drop_shadow = Some(true);
            style.drop_shadow_style = Some(shadow);
        }
    }
}

type PickTool = Box<dyn Fn(&str)>;

/// The section for a drawing or Text tool with nothing selected.
pub(super) struct DefaultsSection {
    pub(super) root: gtk::Box,
    pub(super) values: Rc<Defaults>,
    tool: Rc<RefCell<String>>,
    picker: gtk::Grid,
    shapes: Vec<(&'static str, gtk::Button)>,
    stroke_check: gtk::ToggleButton,
    stroke: gtk::Box,
    color: Rc<ColorField>,
    width: RangeSlider,
    opacity: RangeSlider,
    fill_check: gtk::ToggleButton,
    fill: Rc<ColorField>,
    text_style: Rc<Choice>,
    text_size: NumberField,
    text_box: gtk::Box,
    shadow: DropShadow,
    syncing: Rc<Cell<bool>>,
    pick: Rc<RefCell<Option<PickTool>>>,
}

impl DefaultsSection {
    pub(super) fn new(tokens: &Rc<Tokens>) -> Rc<Self> {
        let values = Rc::new(Defaults {
            style: RefCell::new(ElementStyle::default()),
            opacity: Cell::new(100.),
            text_size: Cell::new(48.),
            text_preset: RefCell::new("rounded-box".into()),
        });
        let syncing = Rc::new(Cell::new(false));
        let tool = Rc::new(RefCell::new(String::new()));
        let pick: Rc<RefCell<Option<PickTool>>> = Rc::default();
        let section: Rc<RefCell<Option<std::rc::Weak<DefaultsSection>>>> = Rc::default();
        // Every change updates the defaults, then re-syncs visibility.
        let update = {
            let values = values.clone();
            let syncing = syncing.clone();
            let section = section.clone();
            Rc::new(move |change: &dyn Fn(&Defaults)| {
                if syncing.get() {
                    return;
                }
                change(&values);
                if let Some(section) = section.borrow().as_ref().and_then(|w| w.upgrade()) {
                    section.sync();
                }
            })
        };
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("property-section");

        // `.screenshot-shape-picker`: three equal columns, two rows.
        let picker = gtk::Grid::builder()
            .column_spacing(4)
            .row_spacing(4)
            .column_homogeneous(true)
            .build();
        picker.set_accessible_role(gtk::AccessibleRole::Group);
        picker.update_property(&[gtk::accessible::Property::Label("Shape")]);
        let shapes: Vec<_> = captures_app::editor_chrome::SHAPE_TOOLS
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let button = gtk::Button::new();
                button.add_css_class("shape-pick");
                button.set_child(Some(&Icon::new(item.key, 18).with_stroke(1.8)));
                button.set_tooltip_text(Some(&item.name()));
                button.update_property(&[gtk::accessible::Property::Label(item.label)]);
                let pick = pick.clone();
                let key = item.key;
                button.connect_clicked(move |_| {
                    if let Some(pick) = pick.borrow().as_ref() {
                        pick(key);
                    }
                });
                picker.attach(&button, index as i32 % 3, index as i32 / 3, 1, 1);
                (item.key, button)
            })
            .collect();
        root.append(&picker);

        let stroke_check = check_row("Stroke", false);
        {
            let update = update.clone();
            stroke_check.connect_toggled(move |check| {
                let on = check.is_active();
                update(&|d| d.style.borrow_mut().stroke_enabled = Some(on));
            });
        }
        root.append(&stroke_check);
        let stroke = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let color = {
            let update = update.clone();
            ColorField::new(tokens, colors::COLOR, move |color| {
                update(&|d| d.style.borrow_mut().color = color.clone());
            })
        };
        stroke.append(&color.root);
        let width = RangeSlider::new("Size", "Stroke width", (2., 40.), 8., &[], |v| {
            format!("{} px", v.round())
        });
        {
            let update = update.clone();
            width.scale.connect_value_changed(move |scale| {
                let value = scale.value().round();
                update(&|d| d.style.borrow_mut().stroke_width = value);
            });
        }
        stroke.append(&width.root);
        root.append(&stroke);
        let opacity = RangeSlider::new("Opacity", "Opacity", (0., 100.), 100., &[], |v| {
            format!("{}%", v.round())
        });
        {
            let update = update.clone();
            opacity.scale.connect_value_changed(move |scale| {
                let value = scale.value().round();
                update(&|d| d.opacity.set(value));
            });
        }
        root.append(&opacity.root);
        let fill_check = check_row("Filled shape", true);
        {
            let update = update.clone();
            fill_check.connect_toggled(move |check| {
                let on = check.is_active();
                update(&|d| {
                    let mut style = d.style.borrow_mut();
                    style.fill = on.then(|| style.color.clone());
                });
            });
        }
        root.append(&fill_check);
        let fill = {
            let update = update.clone();
            ColorField::new(tokens, colors::FILL_COLOR, move |color| {
                update(&|d| d.style.borrow_mut().fill = Some(color.clone()));
            })
        };
        root.append(&fill.root);

        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let text_style = {
            let update = update.clone();
            Choice::new(
                Some(&text("New text style", "property-caption")),
                "New text style",
                true,
                move |id| update(&|d| *d.text_preset.borrow_mut() = id.clone()),
            )
        };
        text_style.set_options(
            TEXT_STYLE_PRESETS
                .iter()
                .map(|p| (p.id.to_owned(), p.label.to_owned()))
                .collect(),
        );
        text_box.append(&text_style.root);
        let text_size = {
            let update = update.clone();
            NumberField::new("New text size", (8., 512.), move |size| {
                update(&|d| d.text_size.set(size.round()));
            })
        };
        text_box.append(&field("New text size", &text_size.root));
        root.append(&text_box);

        let shadow = {
            let update = update.clone();
            DropShadow::new(tokens, move |change| {
                let change = RefCell::new(Some(change));
                update(&|d| {
                    if let Some(change) = change.borrow_mut().take() {
                        apply_shadow(&mut d.style.borrow_mut(), change);
                    }
                });
            })
        };
        root.append(&shadow.root);

        let defaults = Rc::new(DefaultsSection {
            root,
            values,
            tool,
            picker,
            shapes,
            stroke_check,
            stroke,
            color,
            width,
            opacity,
            fill_check,
            fill,
            text_style,
            text_size,
            text_box,
            shadow,
            syncing,
            pick,
        });
        *section.borrow_mut() = Some(Rc::downgrade(&defaults));
        defaults
    }

    /// The shape picker selects a tool through the editor.
    pub(super) fn connect_pick(&self, pick: impl Fn(&str) + 'static) {
        *self.pick.borrow_mut() = Some(Box::new(pick));
    }

    /// Show the defaults for `tool` (`None` hides the section).
    pub(super) fn show(&self, tool: Option<&str>) {
        self.root.set_visible(tool.is_some());
        if let Some(tool) = tool {
            *self.tool.borrow_mut() = tool.to_owned();
            self.sync();
        }
    }

    fn sync(&self) {
        let tool = self.tool.borrow().clone();
        let text_tool = tool == "t";
        let closed = closed_tool(&tool);
        let grouped = captures_app::editor_chrome::SHAPE_TOOLS
            .iter()
            .any(|item| item.key == tool);
        let values = &self.values;
        let style = values.style.borrow().clone();
        self.syncing.set(true);
        self.picker.set_visible(grouped);
        for (key, button) in &self.shapes {
            pressed(button, *key == tool);
        }
        self.stroke_check.set_visible(closed);
        self.stroke_check.set_active(style.has_stroke());
        self.stroke
            .set_visible(!text_tool && (!closed || style.has_stroke()));
        self.color.set_label(if closed {
            colors::STROKE_COLOR
        } else {
            colors::COLOR
        });
        self.color.set(&style.color);
        self.width.set(style.stroke_width, false);
        self.opacity.root.set_visible(!text_tool);
        self.opacity.set(values.opacity.get(), false);
        self.fill_check.set_visible(closed);
        self.fill_check.set_active(style.fill.is_some());
        self.fill.root.set_visible(closed && style.fill.is_some());
        if let Some(fill) = &style.fill {
            self.fill.set(fill);
        }
        self.text_box.set_visible(text_tool);
        self.text_style.set(&values.text_preset.borrow());
        self.text_size.set(values.text_size.get(), false);
        // Text's default shadow scales with the new text size.
        let shadow_style = if text_tool {
            editor_text::new_text_shadow_style(&style, values.text_size.get())
        } else {
            style
        };
        self.shadow.sync(
            shadow_style.has_drop_shadow(),
            &shadow_style.resolved_drop_shadow_style(),
            false,
        );
        self.syncing.set(false);
    }
}
