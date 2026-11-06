//! The sidebar's Tool properties (`.screenshot-properties`): shipping's
//! new-tool defaults and selected shape, path or label sections (the
//! label's in `text_section`) from
//! `ScreenshotEditor.tsx`, laid out at the `editor-image.css` and
//! `primitives.css` metrics. Each control is a retained node like the rest
//! of the editor chrome: hover and focus move on the compositor, a value
//! change redraws only its control, and the section body (legends and the
//! rule) redraws only when the set of rows changes. The body scrolls under
//! the sticky heading by moving its visual.
//!
//! Defaults are window-local, seeded once from the session's
//! `initial_annotation_style` as GTK and AppKit do; edits to a selection go
//! to the session as layer edits, slider drags as `Request::Live` so a drag
//! is one undo step.
use captures_app::editor::{
    AnnotationStylePatch, DropShadowStyle, DropShadowStylePatch, ElementStyle, LayerEdit,
    OptionalNullable, TextElement,
};
use captures_app::editor_chrome::{SHAPE_TOOLS, colors, eraser, text_format};
use captures_app::editor_image_background::BrushMode;
use captures_app::editor_session::{AnnotationControls, Request, TextPatch};
use captures_app::editor_text::{self, TEXT_STYLE_PRESETS};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_TRAILING;
use windows::core::*;
use windows_numerics::Vector2;

use super::menu::MenuKind;
use super::{Editor, Event, Layer, Target, closed_kind, hex_color, polyline};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

/// One Properties control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    /// `.screenshot-shape-picker` button, in `SHAPE_TOOLS` order.
    Shape(usize),
    Check(Check),
    /// A `ColorField` swatch; `CUSTOM` is the custom-color tile.
    Swatch(Field, usize),
    Range(Range),
    /// An Eraser mode button, in `MODES` order.
    Mode(usize),
    /// The Text style (or New text style) picker's trigger.
    StyleTrigger,
    /// The selected label's Font select.
    Font,
    /// A `NumberInput`, and its stepper (`true` for Increase).
    Number(Num),
    Step(Num, bool),
    /// The Text section's textarea.
    Textarea,
    /// A Text format button: Bold, Italic, then `text_format::ALIGN`.
    Format(usize),
}

/// A `NumberInput` in Tool properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Num {
    /// The selected label's Size.
    TextSize,
    /// The Text tool's New text size.
    NewTextSize,
}

impl Num {
    pub const ALL: [Num; 2] = [Num::TextSize, Num::NewTextSize];

    /// `(min, max)` of the shipping `NumberInput`.
    pub fn bounds(self) -> (f64, f64) {
        match self {
            Num::TextSize | Num::NewTextSize => (8., 512.),
        }
    }

    /// The `<label>` caption and the input's accessible name.
    pub fn label(self) -> &'static str {
        match self {
            Num::TextSize => "Size",
            Num::NewTextSize => "New text size",
        }
    }
}

/// One edit from the Text section.
#[derive(Clone, Debug, PartialEq)]
pub enum TextChange {
    Preset(&'static str),
    Content(String),
    Family(String),
    Size(f64),
    Bold,
    Italic,
    Align(&'static str),
}

/// The Bold and Italic buttons, then the alignments.
pub const FORMATS: usize = 2 + text_format::ALIGN.len();
/// The plate the Text background check adds (shipping `#111318`).
const TEXT_PLATE: &str = "#111318";
/// `.screenshot-text-style-trigger { min-height: 38px }`.
const TRIGGER_H: f32 = 38.;
/// `select { min-height: var(--h-md) }`; the number input adds its border.
const SELECT_H: f32 = 32.;
const NUMBER_H: f32 = 34.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Stroke,
    Fill,
    Shadow,
    /// The Wand's Contiguous only.
    Contiguous,
    /// The Text section's Text background.
    Plate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Stroke,
    Fill,
    Shadow,
    /// Text color and the plate's Background color.
    Text,
    Plate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    Width,
    Opacity,
    ShadowOpacity,
    ShadowBlur,
    /// The Wand's color tolerance.
    Tolerance,
    BrushSize,
    Softness,
}

/// Shipping `RemoveBackgroundMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EraserMode {
    Wand,
    Brush(BrushMode),
}

pub const MODES: [(EraserMode, &str); 3] = [
    (EraserMode::Wand, "Wand"),
    (EraserMode::Brush(BrushMode::Erase), "Erase"),
    (EraserMode::Brush(BrushMode::Restore), "Restore"),
];

/// The Eraser's window-local settings, at shipping's defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eraser {
    pub mode: EraserMode,
    pub tolerance: f64,
    pub contiguous: bool,
    pub size: f64,
    pub softness: f64,
}

impl Default for Eraser {
    fn default() -> Self {
        Eraser {
            mode: EraserMode::Wand,
            tolerance: 36.,
            contiguous: true,
            size: 28.,
            softness: 18.,
        }
    }
}

/// An Eraser paragraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    Intro,
    Hint,
}

fn eraser_range(range: Range) -> bool {
    matches!(range, Range::Tolerance | Range::BrushSize | Range::Softness)
}

/// `RangeSlider` marks.
fn marks(range: Range) -> &'static [(f64, &'static str)] {
    match range {
        Range::Tolerance => &eraser::TOLERANCE_MARKS,
        Range::BrushSize => &eraser::SIZE_MARKS,
        Range::Softness => &eraser::SOFTNESS_MARKS,
        _ => &[],
    }
}

/// The custom-color tile's index after the swatches.
pub const CUSTOM: usize = colors::SWATCHES.len();
const CHECKS: [Check; 5] = [
    Check::Stroke,
    Check::Fill,
    Check::Shadow,
    Check::Contiguous,
    Check::Plate,
];
const FIELDS: [Field; 5] = [
    Field::Stroke,
    Field::Fill,
    Field::Shadow,
    Field::Text,
    Field::Plate,
];
const RANGES: [Range; 7] = [
    Range::Width,
    Range::Opacity,
    Range::ShadowOpacity,
    Range::ShadowBlur,
    Range::Tolerance,
    Range::BrushSize,
    Range::Softness,
];
/// `.range-slider.has-marks`: a 2 px gap and the 14 px label row.
const MARKS_H: f32 = 16.;
/// `.screenshot-format-buttons button { min-height: 32px }`.
const MODE_H: f32 = 32.;
/// `.screenshot-check-row { min-height: 28px }` and its 15 px box.
const CHECK_H: f32 = 28.;
const CHECK_BOX: f32 = 15.;
/// A property caption (`text-sm`) and its `gap: var(--s-3)`.
const CAPTION_H: f32 = 14.;
/// Where a slider starts under its caption (`gap: var(--s-3)`).
pub const SLIDER_TOP: f32 = CAPTION_H + 6.;
/// `RangeSlider`: a 14 px value row, 2 px gap and the 20 px track row.
const SLIDER_H: f32 = 36.;
const THUMB: f32 = 14.;
/// `.screenshot-shape-picker button { min-height: 40px }`.
const PICKER_H: f32 = 40.;
/// Nested drop-shadow knobs line up with the label, not the box.
const INDENT: f32 = CHECK_BOX + 8.;
const TILE: f32 = colors::TILE as f32;

/// Every Properties control, in build order.
pub fn all() -> Vec<Prop> {
    let mut list: Vec<Prop> = (0..SHAPE_TOOLS.len()).map(Prop::Shape).collect();
    list.extend(CHECKS.map(Prop::Check));
    for field in FIELDS {
        list.extend((0..=CUSTOM).map(|i| Prop::Swatch(field, i)));
    }
    list.extend(RANGES.map(Prop::Range));
    list.extend((0..MODES.len()).map(Prop::Mode));
    list.extend([Prop::StyleTrigger, Prop::Textarea, Prop::Font]);
    // Steppers are built as children of their field.
    list.extend(Num::ALL.map(Prop::Number));
    list.extend((0..FORMATS).map(Prop::Format));
    list
}

/// The values a section edits: a selected shape or path, or the defaults
/// for the next one.
#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    pub closed: bool,
    pub color: String,
    pub fill: Option<String>,
    pub width: f64,
    pub stroke: bool,
    pub shadow: bool,
    pub shadow_color: String,
    pub shadow_opacity: f64,
    pub shadow_blur: f64,
    pub opacity: f64,
    /// A label's Text color and plate.
    pub text_color: String,
    pub plate: Option<String>,
}

impl Values {
    pub fn from_controls(c: &AnnotationControls, opacity: f64) -> Self {
        Values {
            closed: c.closed,
            color: c.color.to_owned(),
            fill: c.fill.map(str::to_owned),
            width: c.stroke_width,
            stroke: c.stroke_enabled,
            shadow: c.drop_shadow,
            shadow_color: c.drop_shadow_style.color.clone(),
            shadow_opacity: c.drop_shadow_style.opacity,
            shadow_blur: c.drop_shadow_style.blur,
            opacity,
            text_color: String::new(),
            plate: None,
        }
    }

    /// A label: its shadow as shipping's `textDropShadowStyle`.
    fn from_text(text: &TextElement) -> Self {
        let style = editor_text::shadow_style(text, text.font_size);
        Values {
            text_color: text.color.clone(),
            plate: text.background.clone(),
            ..Values::from_style(&style, false, text.base.opacity)
        }
    }

    fn from_style(style: &ElementStyle, closed: bool, opacity: f64) -> Self {
        let shadow = style.resolved_drop_shadow_style();
        Values {
            closed,
            color: style.color.clone(),
            fill: style.fill.clone(),
            width: style.stroke_width,
            stroke: style.has_stroke(),
            shadow: style.has_drop_shadow(),
            shadow_color: shadow.color,
            shadow_opacity: shadow.opacity,
            shadow_blur: shadow.blur,
            opacity,
            text_color: String::new(),
            plate: None,
        }
    }

    pub fn checked(&self, check: Check) -> bool {
        match check {
            Check::Stroke => self.stroke,
            Check::Fill => self.fill.is_some(),
            Check::Shadow => self.shadow,
            Check::Contiguous => false,
            Check::Plate => self.plate.is_some(),
        }
    }

    pub fn color(&self, field: Field) -> &str {
        match field {
            Field::Stroke => &self.color,
            Field::Fill => self.fill.as_deref().unwrap_or(&self.color),
            Field::Shadow => &self.shadow_color,
            Field::Text => &self.text_color,
            Field::Plate => self.plate.as_deref().unwrap_or(TEXT_PLATE),
        }
    }

    pub fn range(&self, range: Range) -> f64 {
        match range {
            Range::Width => self.width,
            Range::Opacity => self.opacity,
            Range::ShadowOpacity => self.shadow_opacity,
            Range::ShadowBlur => self.shadow_blur,
            Range::Tolerance | Range::BrushSize | Range::Softness => 0.,
        }
    }
}

/// Window-local new-tool defaults (shipping `defaultStyle` and
/// `defaultOpacity`).
#[derive(Clone, Debug)]
pub struct Defaults {
    pub style: ElementStyle,
    pub opacity: f64,
}

/// One change from a control.
#[derive(Clone, Debug)]
pub enum Change {
    Color(Field, String),
    Check(Check, bool),
    Range(Range, f64),
}

/// `(min, max)` from the shipping `RangeSlider`s.
pub fn bounds(range: Range) -> (f64, f64) {
    match range {
        Range::Width => (2., 40.),
        Range::Opacity | Range::ShadowOpacity => (0., 100.),
        // `DROP_SHADOW_BLUR_MAX` (captures-image), which
        // `resolved_drop_shadow_style` also clamps to.
        Range::ShadowBlur => (0., 100.),
        Range::Tolerance => eraser::TOLERANCE_RANGE,
        Range::BrushSize => eraser::SIZE_RANGE,
        Range::Softness => eraser::SOFTNESS_RANGE,
    }
}

/// `aria-label`.
pub fn range_label(range: Range) -> &'static str {
    match range {
        Range::Width => "Stroke width",
        Range::Opacity => "Opacity",
        Range::ShadowOpacity => "Shadow opacity",
        Range::ShadowBlur => "Shadow blur",
        Range::Tolerance => eraser::TOLERANCE_LABEL,
        Range::BrushSize => eraser::SIZE_LABEL,
        Range::Softness => eraser::SOFTNESS_LABEL,
    }
}

/// `aria-valuetext` and the readout.
pub fn range_text(range: Range, value: f64) -> String {
    match range {
        Range::Width | Range::ShadowBlur => format!("{} px", value.round()),
        Range::Opacity | Range::ShadowOpacity => format!("{}%", value.round()),
        Range::Tolerance => eraser::tolerance_text(value),
        Range::BrushSize => eraser::size_text(value),
        Range::Softness => eraser::softness_text(value),
    }
}

/// A laid-out row, body-relative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    Picker,
    /// The Eraser mode buttons.
    Modes,
    Note(Note),
    Check(Check),
    Color(Field),
    Range(Range),
    /// The Text style picker (legend and trigger).
    StylePicker,
    /// The Text textarea under its caption.
    Textarea,
    /// `.screenshot-number-pair`: Font and Size.
    FontSize,
    /// A full-width `NumberInput` under its caption.
    Number(Num),
    /// `.screenshot-format-buttons`.
    Formats,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Laid {
    pub row: Row,
    pub rect: Rect,
}

/// What the Properties panel edits right now.
#[derive(Clone, Debug, PartialEq)]
pub enum Section {
    Eraser,
    Defaults,
    Selected(String),
    /// A selected label.
    Text(String),
    /// The Text tool without a selection.
    TextDefaults,
}

/// The Properties body's top: the heading's 8 px margin and the section's
/// 12 px padding.
pub const BODY_TOP: f32 = 20.;

impl Editor {
    /// Shipping's section for the current tool and selection. Image and
    /// crop sections are not in this host yet.
    pub fn section(&self) -> Option<Section> {
        if self.tool == "b" {
            return Some(Section::Eraser);
        }
        if let Some(id) = &self.selected {
            if self.selected_text().is_some() {
                return Some(Section::Text(id.clone()));
            }
            let p = self.presented.as_ref()?;
            return p
                .controls
                .contains_key(id)
                .then(|| Section::Selected(id.clone()));
        }
        match self.tool {
            "t" => Some(Section::TextDefaults),
            "v" | "c" | "b" => None,
            _ => Some(Section::Defaults),
        }
    }

    pub fn values(&self) -> Option<Values> {
        match self.section()? {
            Section::Eraser => None,
            Section::Selected(id) => self.presented.as_ref()?.controls.get(&id).cloned(),
            Section::Text(_) => Some(Values::from_text(self.selected_text()?)),
            Section::TextDefaults => {
                // Shipping `textDropShadowStyle` at the new text size.
                let d = self.defaults.as_ref()?;
                let style = editor_text::new_text_shadow_style(&d.style, self.new_text_size());
                Some(Values::from_style(&style, false, d.opacity))
            }
            Section::Defaults => {
                let d = self.defaults.as_ref()?;
                Some(Values::from_style(
                    &d.style,
                    closed_kind(self.tool).is_some(),
                    d.opacity,
                ))
            }
        }
    }

    /// The rows for the current section, in shipping order.
    fn prop_rows(&self) -> Vec<Row> {
        if self.section() == Some(Section::Eraser) {
            let mut rows = vec![Row::Note(Note::Intro), Row::Modes];
            if self.eraser.mode == EraserMode::Wand {
                rows.extend([Row::Range(Range::Tolerance), Row::Check(Check::Contiguous)]);
            } else {
                rows.extend([Row::Range(Range::BrushSize), Row::Range(Range::Softness)]);
            }
            rows.push(Row::Note(Note::Hint));
            return rows;
        }
        let (Some(section), Some(v)) = (self.section(), self.values()) else {
            return Vec::new();
        };
        let shadow_rows = |rows: &mut Vec<Row>| {
            rows.push(Row::Check(Check::Shadow));
            if v.shadow {
                rows.extend([
                    Row::Color(Field::Shadow),
                    Row::Range(Range::ShadowOpacity),
                    Row::Range(Range::ShadowBlur),
                ]);
            }
        };
        match section {
            Section::Text(_) => {
                let mut rows = vec![
                    Row::StylePicker,
                    Row::Textarea,
                    Row::FontSize,
                    Row::Formats,
                    Row::Color(Field::Text),
                    Row::Check(Check::Plate),
                ];
                if v.plate.is_some() {
                    rows.push(Row::Color(Field::Plate));
                }
                shadow_rows(&mut rows);
                return rows;
            }
            Section::TextDefaults => {
                let mut rows = vec![Row::StylePicker, Row::Number(Num::NewTextSize)];
                shadow_rows(&mut rows);
                return rows;
            }
            _ => {}
        }
        let defaults = section == Section::Defaults;
        let mut rows = Vec::new();
        if defaults && SHAPE_TOOLS.iter().any(|s| s.key == self.tool) {
            rows.push(Row::Picker);
        }
        if v.closed {
            rows.push(Row::Check(Check::Stroke));
        }
        if !v.closed || v.stroke {
            rows.extend([Row::Color(Field::Stroke), Row::Range(Range::Width)]);
        }
        rows.push(Row::Range(Range::Opacity));
        let fill = |rows: &mut Vec<Row>| {
            if v.closed {
                rows.push(Row::Check(Check::Fill));
                if v.fill.is_some() {
                    rows.push(Row::Color(Field::Fill));
                }
            }
        };
        let shadow = |rows: &mut Vec<Row>| {
            rows.push(Row::Check(Check::Shadow));
            if v.shadow {
                rows.extend([
                    Row::Color(Field::Shadow),
                    Row::Range(Range::ShadowOpacity),
                    Row::Range(Range::ShadowBlur),
                ]);
            }
        };
        // New-tool defaults put Filled shape before Drop shadow; a
        // selection lists Drop shadow first.
        if defaults {
            fill(&mut rows);
            shadow(&mut rows);
        } else {
            shadow(&mut rows);
            fill(&mut rows);
        }
        rows
    }

    /// Lay out the Properties rows inside a panel `width` wide.
    pub(super) fn layout_props(&mut self, width: f32) {
        let s5 = self.t.n("s-5");
        let w = (width - 2. * s5).max(0.);
        let mut y = BODY_TOP;
        let mut laid = Vec::new();
        for row in self.prop_rows() {
            let indent = match row {
                Row::Color(Field::Shadow)
                | Row::Range(Range::ShadowOpacity | Range::ShadowBlur) => INDENT,
                _ => 0.,
            };
            let rw = w - indent;
            let h = match row {
                Row::Picker => 2. * PICKER_H + self.t.n("s-2"),
                Row::Modes => MODE_H,
                Row::Note(note) => self
                    .g
                    .text(&self.t, Style::Option, &self.note_text(note), rw)
                    .map_or(16., |t| t.height.ceil()),
                Row::Range(range) if !marks(range).is_empty() => {
                    CAPTION_H + self.t.n("s-3") + SLIDER_H + MARKS_H
                }
                Row::Check(_) => CHECK_H,
                Row::Color(_) => CAPTION_H + self.t.n("s-4") + grid_height(rw),
                Row::Range(_) => CAPTION_H + self.t.n("s-3") + SLIDER_H,
                // A legend or `<label>` caption over its control.
                Row::StylePicker => CAPTION_H + self.t.n("s-3") + TRIGGER_H,
                Row::Textarea => CAPTION_H + self.t.n("s-3") + super::fields::TEXTAREA_H,
                Row::FontSize | Row::Number(_) => CAPTION_H + self.t.n("s-3") + NUMBER_H,
                Row::Formats => text_format::BUTTON_HEIGHT as f32,
            };
            laid.push(Laid {
                row,
                rect: Rect::new(s5 + indent, y, rw, h),
            });
            y += h + s5;
        }
        // Section padding, its rule and the panel's `--s-8` bottom padding.
        self.props_height = if laid.is_empty() {
            0.
        } else {
            y + 1. + self.t.n("s-8")
        };
        if laid
            .iter()
            .map(|l| l.row)
            .ne(self.props.iter().map(|l| l.row))
        {
            self.events.push(Event::Structure);
        }
        self.props = laid;
    }

    fn laid(&self, row: Row) -> Option<Rect> {
        self.props.iter().find(|l| l.row == row).map(|l| l.rect)
    }

    /// A control's rect, body-relative.
    pub(super) fn prop_rect(&self, prop: Prop) -> Option<Rect> {
        match prop {
            Prop::Shape(i) => {
                let r = self.laid(Row::Picker)?;
                let gap = self.t.n("s-2");
                let cell = (r.w - 2. * gap) / 3.;
                Some(Rect::new(
                    r.x + (i % 3) as f32 * (cell + gap),
                    r.y + (i / 3) as f32 * (PICKER_H + gap),
                    cell,
                    PICKER_H,
                ))
            }
            Prop::Check(check) => self.laid(Row::Check(check)),
            Prop::Mode(i) => {
                let r = self.laid(Row::Modes)?;
                let gap = self.t.n("s-2");
                let cell = (r.w - 2. * gap) / 3.;
                Some(Rect::new(r.x + i as f32 * (cell + gap), r.y, cell, r.h))
            }
            Prop::Range(range) => self.laid(Row::Range(range)),
            Prop::StyleTrigger => {
                let r = self.laid(Row::StylePicker)?;
                Some(Rect::new(r.x, r.y + r.h - TRIGGER_H, r.w, TRIGGER_H))
            }
            Prop::Textarea => {
                let r = self.laid(Row::Textarea)?;
                let h = super::fields::TEXTAREA_H;
                Some(Rect::new(r.x, r.y + r.h - h, r.w, h))
            }
            Prop::Font | Prop::Number(Num::TextSize) => {
                let r = self.laid(Row::FontSize)?;
                let gap = self.t.n("s-4");
                let half = (r.w - gap) / 2.;
                let top = r.y + r.h - NUMBER_H;
                Some(if prop == Prop::Font {
                    Rect::new(r.x, top, half, SELECT_H)
                } else {
                    Rect::new(r.x + half + gap, top, half, NUMBER_H)
                })
            }
            Prop::Number(num) => {
                let r = self.laid(Row::Number(num))?;
                Some(Rect::new(r.x, r.y + r.h - NUMBER_H, r.w, NUMBER_H))
            }
            // Inside its number field: the top or bottom half of the column.
            Prop::Step(num, up) => {
                let field = self.prop_rect(Prop::Number(num))?;
                let x = field.w - 1. - super::fields::STEPPERS_W;
                let mid = (field.h / 2.).round();
                Some(if up {
                    Rect::new(x, 1., super::fields::STEPPERS_W, mid - 1.)
                } else {
                    Rect::new(x, mid + 1., super::fields::STEPPERS_W, field.h - mid - 2.)
                })
            }
            Prop::Format(i) => {
                let r = self.laid(Row::Formats)?;
                let gap = self.t.n("s-2");
                let columns = text_format::COLUMNS as f32;
                let cell = (r.w - (columns - 1.) * gap) / columns;
                Some(Rect::new(r.x + i as f32 * (cell + gap), r.y, cell, r.h))
            }
            Prop::Swatch(field, i) => {
                let r = self.laid(Row::Color(field))?;
                let grid = colors::grid(f64::from(r.w), colors::CELL);
                let (cx, cy) = grid.center(i, f64::from(self.t.n("s-4")), 6.);
                let top = r.y + CAPTION_H + self.t.n("s-4");
                Some(Rect::new(
                    r.x + cx as f32 - TILE / 2.,
                    top + cy as f32 - TILE / 2.,
                    TILE,
                    TILE,
                ))
            }
        }
    }

    /// The Shape picker's rect in client DIPs.
    pub fn picker_rect(&self) -> Rect {
        self.row_rect(Row::Picker)
    }

    /// The Eraser mode group's rect in client DIPs.
    pub fn modes_rect(&self) -> Rect {
        self.row_rect(Row::Modes)
    }

    /// An Eraser paragraph's copy.
    pub fn note_text(&self, note: Note) -> String {
        match (note, self.eraser.mode) {
            (Note::Intro, _) => eraser::INTRO,
            (Note::Hint, EraserMode::Wand) => eraser::wand_hint(self.eraser.contiguous),
            (Note::Hint, EraserMode::Brush(BrushMode::Erase)) => eraser::ERASE_HINT,
            (Note::Hint, EraserMode::Brush(BrushMode::Restore)) => eraser::RESTORE_HINT,
        }
        .into()
    }

    /// Whether a check box is checked.
    pub fn check_on(&self, check: Check) -> bool {
        match check {
            Check::Contiguous => self.eraser.contiguous,
            check => self.values().is_some_and(|v| v.checked(check)),
        }
    }

    /// A color field's rect in client DIPs.
    pub fn color_field_rect(&self, field: Field) -> Rect {
        self.row_rect(Row::Color(field))
    }

    fn row_rect(&self, row: Row) -> Rect {
        let body = self.absolute(self.props_body);
        self.laid(row)
            .map(|r| Rect::new(body.x + r.x, body.y + r.y, r.w, r.h))
            .unwrap_or_default()
    }

    /// The Properties body's visible band in client DIPs.
    pub fn props_view(&self) -> Rect {
        let p = self.l.properties;
        Rect::new(
            p.x,
            p.y + super::HEADING_H + 1.,
            p.w,
            (p.h - super::HEADING_H - 1.).max(0.),
        )
    }

    pub(super) fn scroll_props(&mut self, delta: f32) {
        let max = (self.props_height - self.props_view().h).max(0.);
        let next = (self.props_scroll + delta).clamp(0., max);
        if next != self.props_scroll {
            self.props_scroll = next;
            self.place();
            self.need = true;
        }
    }

    /// Scroll a focused control into the visible band.
    pub(super) fn reveal_prop(&mut self, prop: Prop) {
        let Some(r) = self.prop_rect(prop) else {
            return;
        };
        let h = self.props_view().h;
        let delta = if r.y - 4. < self.props_scroll {
            r.y - 4. - self.props_scroll
        } else if r.y + r.h + 4. > self.props_scroll + h {
            r.y + r.h + 4. - h - self.props_scroll
        } else {
            return;
        };
        self.scroll_props(delta);
    }

    /// The rows or their values changed: lay out again and redraw the panel.
    pub(super) fn props_changed(&mut self) {
        self.relayout = true;
        self.nodes[self.properties].dirty = true;
        self.nodes[self.props_body].dirty = true;
        for (target, layered) in &self.controls {
            if matches!(target, Target::Prop(_)) {
                for node in [layered.base, layered.ring] {
                    self.nodes[node].dirty = true;
                }
            }
        }
        self.need = true;
    }

    pub fn prop_exists(&self, prop: Prop) -> bool {
        self.prop_rect(prop).is_some()
    }

    pub fn prop_pressed(&self, prop: Prop) -> Option<bool> {
        Some(match prop {
            Prop::Shape(i) => SHAPE_TOOLS[i].key == self.tool,
            Prop::Check(check) => self.check_on(check),
            Prop::Mode(i) => self.eraser.mode == MODES[i].0,
            Prop::Format(i) => self.format_on(i),
            Prop::Swatch(..)
            | Prop::Range(_)
            | Prop::StyleTrigger
            | Prop::Font
            | Prop::Number(_)
            | Prop::Step(..)
            | Prop::Textarea => return None,
        })
    }

    pub fn prop_name(&self, prop: Prop) -> String {
        match prop {
            Prop::Shape(i) => SHAPE_TOOLS[i].label.into(),
            Prop::Check(check) => check_label(check).into(),
            Prop::Swatch(_, CUSTOM) => colors::CUSTOM_COLOR.into(),
            Prop::Swatch(field, i) => {
                colors::swatch_label(&self.field_label(field), colors::SWATCHES[i])
            }
            Prop::Range(range) => range_label(range).into(),
            Prop::Mode(i) => MODES[i].1.into(),
            Prop::StyleTrigger => {
                let preset = self.style_preset();
                let label = TEXT_STYLE_PRESETS
                    .iter()
                    .find(|p| p.id == preset)
                    .map_or("", |p| p.label);
                format!("{}: {label}", self.style_label())
            }
            Prop::Font => "Font".into(),
            Prop::Number(num) => num.label().into(),
            // `NumberInput` without an `ariaLabel`.
            Prop::Step(_, true) => "Increase".into(),
            Prop::Step(_, false) => "Decrease".into(),
            Prop::Textarea => "Text".into(),
            Prop::Format(i) => Editor::format_name(i).into(),
        }
    }

    pub fn prop_help(&self, prop: Prop) -> String {
        match prop {
            Prop::Shape(i) => SHAPE_TOOLS[i].name(),
            Prop::Swatch(_, CUSTOM) => colors::CUSTOM_COLOR.into(),
            _ => String::new(),
        }
    }

    /// A color field's legend.
    pub fn field_label(&self, field: Field) -> String {
        match field {
            Field::Stroke
                if self.values().is_some_and(|v| v.closed)
                    || matches!(self.section(), Some(Section::Selected(_))) =>
            {
                colors::STROKE_COLOR.into()
            }
            Field::Stroke => colors::COLOR.into(),
            Field::Fill => colors::FILL_COLOR.into(),
            Field::Shadow => colors::SHADOW_COLOR.into(),
            Field::Text => colors::TEXT_COLOR.into(),
            Field::Plate => colors::BACKGROUND.into(),
        }
    }

    /// The caption over a slider.
    fn range_caption(&self, range: Range) -> &'static str {
        match range {
            Range::Width if self.section() == Some(Section::Defaults) => "Size",
            Range::Width => "Stroke width",
            Range::Opacity | Range::ShadowOpacity => "Opacity",
            Range::ShadowBlur => "Blur",
            Range::Tolerance => eraser::TOLERANCE,
            Range::BrushSize => eraser::SIZE,
            Range::Softness => eraser::SOFTNESS,
        }
    }

    /// A slider's shown value: the drag in progress, else the section's.
    pub fn range_value(&self, range: Range) -> f64 {
        match (self.range_drag, range) {
            (Some((r, value)), _) if r == range => value,
            (_, Range::Tolerance) => self.eraser.tolerance,
            (_, Range::BrushSize) => self.eraser.size,
            (_, Range::Softness) => self.eraser.softness,
            _ => self.values().map_or(0., |v| v.range(range)).round(),
        }
    }

    pub fn activate_prop(&mut self, prop: Prop) {
        match prop {
            Prop::Shape(i) => self.select_tool(SHAPE_TOOLS[i].key),
            Prop::Mode(i) => {
                if self.eraser.mode != MODES[i].0 {
                    self.eraser.mode = MODES[i].0;
                    for j in 0..MODES.len() {
                        self.events
                            .push(Event::Toggled(Target::Prop(Prop::Mode(j))));
                    }
                    self.props_changed();
                    self.overlay_changed();
                }
            }
            Prop::Check(check) => self.apply(Change::Check(check, !self.check_on(check))),
            Prop::Swatch(field, CUSTOM) => self.pick_color = Some(field),
            Prop::Swatch(field, i) => {
                self.apply(Change::Color(field, colors::SWATCHES[i].to_owned()))
            }
            Prop::StyleTrigger => self.toggle_menu(MenuKind::Style),
            Prop::Font => self.toggle_menu(MenuKind::Font),
            Prop::Step(num, up) => self.nudge(num, up),
            Prop::Format(i) => self.activate_format(i),
            Prop::Range(_) | Prop::Number(_) | Prop::Textarea => {}
        }
    }

    /// Step a focused slider like `input[type=range]`; true when consumed.
    pub(super) fn range_key(&mut self, range: Range, vk: u16) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let (min, max) = bounds(range);
        let value = self.range_value(range);
        let big = ((max - min) / 10.).round().max(1.);
        let next = match windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk) {
            VK_LEFT | VK_DOWN => value - 1.,
            VK_RIGHT | VK_UP => value + 1.,
            VK_PRIOR => value + big,
            VK_NEXT => value - big,
            VK_HOME => min,
            VK_END => max,
            _ => return false,
        };
        self.set_range(range, next, false);
        true
    }

    /// Set a slider's value; `live` while a drag continues.
    pub fn set_range(&mut self, range: Range, value: f64, live: bool) {
        let (min, max) = bounds(range);
        let value = value.round().clamp(min, max);
        // Ending a drag always commits; otherwise an unchanged value is a no-op.
        let ending = !live && self.range_drag.is_some();
        if !ending && value == self.range_value(range) {
            return;
        }
        self.range_drag = live.then_some((range, value));
        self.apply(Change::Range(range, value));
    }

    /// A slider value from a client x inside its track.
    pub(super) fn range_at(&self, range: Range, x: f32) -> f64 {
        let r = self.target_rect(Target::Prop(Prop::Range(range)));
        let track = (r.w - THUMB).max(1.);
        let p = f64::from(((x - r.x - THUMB / 2.) / track).clamp(0., 1.));
        let (min, max) = bounds(range);
        min + p * (max - min)
    }

    /// Apply a change to the selection (a layer edit) or to the defaults.
    pub fn apply(&mut self, change: Change) {
        let Some(section) = self.section() else {
            return;
        };
        match section {
            Section::Eraser => match change {
                Change::Check(Check::Contiguous, on) => self.eraser.contiguous = on,
                Change::Range(Range::Tolerance, value) => self.eraser.tolerance = value,
                Change::Range(Range::BrushSize, value) => {
                    self.eraser.size = value;
                    self.overlay_changed();
                }
                Change::Range(Range::Softness, value) => self.eraser.softness = value,
                _ => return,
            },
            Section::Text(id) => {
                let Some((live, key, patch)) = self.text_edit(&change) else {
                    return;
                };
                self.edit_text(&id, patch, live.then_some(key));
            }
            Section::Defaults | Section::TextDefaults => {
                let Some(d) = self.defaults.as_mut() else {
                    return;
                };
                let s = &mut d.style;
                match change.clone() {
                    Change::Color(Field::Stroke, color) => s.color = color,
                    Change::Color(Field::Fill, color) => s.fill = Some(color),
                    Change::Color(Field::Shadow, color) => {
                        s.drop_shadow = Some(true);
                        let mut shadow = s.resolved_drop_shadow_style();
                        shadow.color = color;
                        s.drop_shadow_style = Some(shadow);
                    }
                    Change::Check(Check::Stroke, on) => s.stroke_enabled = Some(on),
                    Change::Check(Check::Fill, on) => {
                        s.fill = on.then(|| s.color.clone());
                    }
                    Change::Check(Check::Shadow, on) => s.drop_shadow = Some(on),
                    Change::Check(Check::Contiguous | Check::Plate, _)
                    | Change::Color(Field::Text | Field::Plate, _) => {}
                    Change::Range(Range::Width, value) => s.stroke_width = value,
                    Change::Range(Range::Opacity, value) => d.opacity = value,
                    Change::Range(range, _) if eraser_range(range) => {}
                    Change::Range(range, value) => {
                        s.drop_shadow = Some(true);
                        let mut shadow = s.resolved_drop_shadow_style();
                        if range == Range::ShadowOpacity {
                            shadow.opacity = value;
                        } else {
                            shadow.blur = value;
                        }
                        s.drop_shadow_style = Some(shadow);
                    }
                }
            }
            Section::Selected(id) => {
                let (live, key, edit) = selected_edit(&change, self.values().as_ref());
                if let Some(values) = self
                    .presented
                    .as_mut()
                    .and_then(|p| p.controls.get_mut(&id))
                {
                    // Show the change now; the session's reply confirms it.
                    preview(values, &change);
                }
                let request = Request::Layer {
                    id: id.clone(),
                    edit,
                };
                if live {
                    self.submit_live(Request::Live {
                        key: format!("{id}:{key}"),
                        request: Box::new(request),
                    });
                } else {
                    self.submit(request);
                }
            }
        }
        match change {
            // A slider never adds or removes rows: redraw only it.
            Change::Range(range, _) => {
                self.events
                    .push(Event::Value(Target::Prop(Prop::Range(range))));
                self.prop_dirty(Prop::Range(range));
            }
            Change::Check(check, _) => {
                self.events
                    .push(Event::Toggled(Target::Prop(Prop::Check(check))));
                self.props_changed();
            }
            Change::Color(..) => self.props_changed(),
        }
    }

    /// The label edit a Text-section change makes: `(live, fold key,
    /// patch)`. Shadow edits write the whole resolved style, as shipping's
    /// `DropShadowFields` does.
    fn text_edit(&self, change: &Change) -> Option<(bool, &'static str, TextPatch)> {
        let text = self.selected_text()?;
        let mut shadow =
            editor_text::shadow_style(text, text.font_size).resolved_drop_shadow_style();
        let with_shadow = |shadow: &DropShadowStyle| DropShadowStylePatch {
            color: Some(shadow.color.clone()),
            opacity: Some(shadow.opacity),
            blur: Some(shadow.blur),
            offset_x: Some(shadow.offset_x),
            offset_y: Some(shadow.offset_y),
        };
        Some(match change.clone() {
            Change::Color(Field::Text, color) => (
                false,
                "color",
                TextPatch {
                    color: Some(color),
                    ..Default::default()
                },
            ),
            Change::Color(Field::Plate, color) => (
                false,
                "background",
                TextPatch {
                    background: OptionalNullable::Value(color),
                    ..Default::default()
                },
            ),
            // A plate replaces an outline or rounded plate.
            Change::Check(Check::Plate, on) => (
                false,
                "background",
                TextPatch {
                    background: if on {
                        OptionalNullable::Value(TEXT_PLATE.into())
                    } else {
                        OptionalNullable::Null
                    },
                    outlined: on.then_some(false),
                    rounded_background: on.then_some(false),
                    ..Default::default()
                },
            ),
            Change::Check(Check::Shadow, on) => (
                false,
                "shadow",
                TextPatch {
                    drop_shadow: Some(on),
                    drop_shadow_style: Some(with_shadow(&shadow)),
                    ..Default::default()
                },
            ),
            Change::Color(Field::Shadow, color) => {
                shadow.color = color;
                (
                    false,
                    "shadow-color",
                    TextPatch {
                        drop_shadow: Some(true),
                        drop_shadow_style: Some(with_shadow(&shadow)),
                        ..Default::default()
                    },
                )
            }
            Change::Range(range @ (Range::ShadowOpacity | Range::ShadowBlur), value) => {
                if range == Range::ShadowOpacity {
                    shadow.opacity = value;
                } else {
                    shadow.blur = value;
                }
                (
                    true,
                    if range == Range::ShadowOpacity {
                        "shadow-opacity"
                    } else {
                        "shadow-blur"
                    },
                    TextPatch {
                        drop_shadow: Some(true),
                        drop_shadow_style: Some(with_shadow(&shadow)),
                        ..Default::default()
                    },
                )
            }
            _ => return None,
        })
    }

    pub(super) fn prop_dirty(&mut self, prop: Prop) {
        let layered = self.layered(Target::Prop(prop));
        self.nodes[layered.base].dirty = true;
        self.nodes[layered.ring].dirty = true;
        self.need = true;
    }

    /// Keep a drag's value shown over a reply to an earlier step of it.
    pub(super) fn keep_drag(&mut self) {
        let (Some((range, value)), Some(Section::Selected(id))) = (self.range_drag, self.section())
        else {
            return;
        };
        if let Some(values) = self
            .presented
            .as_mut()
            .and_then(|p| p.controls.get_mut(&id))
        {
            preview(values, &Change::Range(range, value));
        }
    }

    /// Send a live edit now, or after the one in flight (latest wins).
    pub(super) fn submit_live(&mut self, request: Request) {
        if self.busy {
            self.pending = Some(request);
        } else {
            self.submit(request);
        }
    }

    /// The custom-color dialog's answer.
    pub fn set_color(&mut self, field: Field, hex: String) {
        self.apply(Change::Color(field, hex));
    }

    /// `#rrggbb` the custom-color dialog starts from.
    pub fn color_seed(&self, field: Field) -> String {
        self.values()
            .map(|v| colors::custom_seed(v.color(field)))
            .unwrap_or_else(|| "#000000".into())
    }

    // ---- drawing ----

    /// The body: color-field legends and the section rule.
    pub(super) fn draw_props_body(&self, dc: &ID2D1DeviceContext, w: f32) -> Result<()> {
        let t = &self.t;
        let Some(last) = self.props.last() else {
            return Ok(());
        };
        for laid in &self.props {
            if let Row::Note(note) = laid.row {
                let text = self
                    .g
                    .text(t, Style::Option, &self.note_text(note), laid.rect.w)?;
                gfx::text(dc, &text, laid.rect.x, laid.rect.y, t.color("text-subtle"))?;
            }
            if let Row::Color(field) = laid.row {
                let legend = self
                    .g
                    .text(t, Style::Button, &self.field_label(field), 1e4)?;
                gfx::text(dc, &legend, laid.rect.x, laid.rect.y, t.color("text-muted"))?;
            }
            // Legends and `<label>` captions over the Text controls.
            let captions: &[(Prop, &str)] = match laid.row {
                Row::StylePicker => &[(Prop::StyleTrigger, self.style_label())],
                Row::Textarea => &[(Prop::Textarea, "Text")],
                Row::FontSize => &[(Prop::Font, "Font"), (Prop::Number(Num::TextSize), "Size")],
                Row::Number(num) => &[(Prop::Number(num), num.label())],
                _ => &[],
            };
            for (prop, caption) in captions {
                let x = self.prop_rect(*prop).map_or(laid.rect.x, |r| r.x);
                let text = self.g.text(t, Style::Button, caption, 1e4)?;
                gfx::text(dc, &text, x, laid.rect.y, t.color("text-muted"))?;
            }
        }
        let y = last.rect.y + last.rect.h + t.n("s-5");
        gfx::fill(dc, rect(0., y, w, y + 1.), t.color("border-subtle"))?;
        Ok(())
    }

    pub(super) fn draw_prop(
        &self,
        dc: &ID2D1DeviceContext,
        prop: Prop,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let lit = layer == Layer::Lit;
        if layer == Layer::Ring {
            return match prop {
                Prop::Shape(_) | Prop::Mode(_) => {
                    gfx::ring(dc, all, t.n("r-sm"), 2., 4., c("theme-accent"))
                }
                Prop::Check(_) => {
                    let y = (h - CHECK_BOX) / 2.;
                    let r = rect(0., y, CHECK_BOX, y + CHECK_BOX);
                    gfx::ring(dc, r, t.n("r-xs"), 2., 4., c("theme-accent"))
                }
                Prop::Swatch(_, CUSTOM) => {
                    gfx::ring(dc, all, t.n("r-sm"), 2., 4., c("theme-accent"))
                }
                Prop::Swatch(..) => gfx::ring(dc, all, TILE / 2., 2., 4., c("theme-accent")),
                Prop::Range(range) => {
                    let (x, y) = self.thumb_at(range, w);
                    let r = rect(x, y, x + THUMB, y + THUMB);
                    gfx::ring(dc, r, THUMB / 2., 0., 2., c("theme-accent"))
                }
                Prop::StyleTrigger | Prop::Format(_) => {
                    gfx::ring(dc, all, t.n("r-md"), 2., 4., c("theme-accent"))
                }
                // Inputs and the select show `--focus-ring-tight` with their
                // accent border.
                Prop::Font | Prop::Number(_) | Prop::Textarea => gfx::ring(
                    dc,
                    all,
                    t.n("r-md"),
                    0.,
                    2.,
                    t.color_alpha("theme-accent", 0.55),
                ),
                Prop::Step(..) => Ok(()),
            };
        }
        match prop {
            Prop::StyleTrigger => self.draw_style_trigger(dc, layer, (w, h))?,
            Prop::Font => self.draw_font_select(dc, layer, (w, h))?,
            Prop::Format(i) => self.draw_format(dc, i, layer, (w, h))?,
            Prop::Number(num) => self.draw_number(dc, num, lit, (w, h))?,
            Prop::Step(num, up) => self.draw_step(dc, num, up, lit, (w, h))?,
            Prop::Textarea => self.draw_textarea(dc, lit, (w, h))?,
            Prop::Mode(i) => {
                let active = self.eraser.mode == MODES[i].0;
                let radius = t.n("r-sm");
                let ink = if active {
                    if lit {
                        return Ok(());
                    }
                    gfx::fill_round(dc, all, radius, c("theme-accent"))?;
                    c("theme-accent-ink")
                } else if lit {
                    gfx::fill_round(dc, all, radius, c("control-hover"))?;
                    gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                    c("text")
                } else {
                    gfx::fill_round(dc, all, radius, c("control"))?;
                    gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                    c("text-muted")
                };
                let label = g.text(t, Style::Strong, MODES[i].1, 1e4)?;
                gfx::text(
                    dc,
                    &label,
                    ((w - label.width) / 2.).round(),
                    ((h - label.height) / 2.).round(),
                    ink,
                )?;
            }
            Prop::Shape(i) => {
                let active = SHAPE_TOOLS[i].key == self.tool;
                let radius = t.n("r-sm");
                let ink = if active {
                    if lit {
                        return Ok(());
                    }
                    gfx::fill_round(dc, all, radius, c("theme-accent"))?;
                    c("theme-accent-ink")
                } else if lit {
                    gfx::fill_round(dc, all, radius, c("control-hover"))?;
                    gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                    c("text")
                } else {
                    gfx::fill_round(dc, all, radius, c("control"))?;
                    gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                    c("text-muted")
                };
                gfx::icon(
                    dc,
                    &g.factory,
                    SHAPE_TOOLS[i].icon,
                    &[],
                    ((w - 18.) / 2., (h - 18.) / 2., 18.),
                    1.8,
                    ink,
                )?;
            }
            Prop::Check(check) => {
                if lit {
                    return Ok(());
                }
                let y = (h - CHECK_BOX) / 2.;
                let b = rect(0., y, CHECK_BOX, y + CHECK_BOX);
                let radius = t.n("r-xs");
                if self.check_on(check) {
                    gfx::fill_round(dc, b, radius, c("theme-accent"))?;
                    // The rotated L of `::before`: 7.5 × 4 at 1.75 px.
                    let (cx, cy) = (CHECK_BOX / 2., y + CHECK_BOX / 2. - 0.5);
                    polyline(
                        dc,
                        g,
                        &[(cx - 3.2, cy), (cx - 1.1, cy + 2.1), (cx + 3.3, cy - 2.3)],
                        false,
                        c("theme-accent-ink"),
                        1.75,
                    )?;
                } else {
                    gfx::fill_round(dc, b, radius, c("surface-field"))?;
                    gfx::border(dc, b, radius, 1., c("border-strong"))?;
                }
                let label = g.text(t, Style::Button, check_label(check), 1e4)?;
                gfx::text(
                    dc,
                    &label,
                    CHECK_BOX + t.n("s-4"),
                    ((h - label.height) / 2.).round(),
                    c("text-muted"),
                )?;
            }
            Prop::Swatch(_, CUSTOM) => {
                if lit {
                    return Ok(());
                }
                let radius = t.n("r-sm");
                gfx::shadow(dc, all, radius, t.shadow("shadow-xs"))?;
                conic(dc, &g.factory, all, radius)?;
                let inset = colors::CUSTOM_INSET as f32;
                let inner = rect(inset, inset, w - inset, h - inset);
                gfx::fill_round(dc, inner, 3., c("surface-raised"))?;
                gfx::border(dc, inner, 3., 1., c("border-strong"))?;
                gfx::border(dc, all, radius, 1., c("border-strong"))?;
            }
            Prop::Swatch(field, i) => {
                if lit {
                    return Ok(());
                }
                let r = TILE / 2.;
                let Some(v) = self.values() else {
                    return Ok(());
                };
                if colors::swatch_active(v.color(field), colors::SWATCHES[i]) {
                    gfx::fill_round(
                        dc,
                        rect(-4., -4., w + 4., h + 4.),
                        r + 4.,
                        c("theme-accent"),
                    )?;
                    gfx::fill_round(
                        dc,
                        rect(-2., -2., w + 2., h + 2.),
                        r + 2.,
                        c("surface-raised"),
                    )?;
                } else {
                    gfx::shadow(dc, all, r, t.shadow("shadow-xs"))?;
                }
                gfx::fill_round(dc, all, r, hex_color(colors::SWATCHES[i]))?;
                gfx::border(dc, all, r, 1., c("border-strong"))?;
            }
            Prop::Range(range) => {
                if lit {
                    return Ok(());
                }
                let caption = g.text(t, Style::Button, self.range_caption(range), 1e4)?;
                gfx::text(dc, &caption, 0., 0., c("text-muted"))?;
                let top = SLIDER_TOP;
                let value = self.range_value(range);
                let readout = g.text(t, Style::Status, &range_text(range, value), w)?;
                unsafe {
                    readout
                        .layout
                        .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
                }
                gfx::text(
                    dc,
                    &readout,
                    0.,
                    top + ((14. - readout.height) / 2.).round(),
                    c("text-subtle"),
                )?;
                let (min, max) = bounds(range);
                let p = ((value - min) / (max - min).max(1.)).clamp(0., 1.) as f32;
                let mid = top + 16. + 10.;
                let track = rect(0., mid - 2., w, mid + 2.);
                gfx::fill_round(dc, track, 2., c("n-6"))?;
                if p > 0. {
                    gfx::fill_round(
                        dc,
                        rect(0., mid - 2., w * p, mid + 2.),
                        2.,
                        c("theme-accent"),
                    )?;
                }
                // `.range-slider-ticks` and `.range-slider-labels`, inset
                // half a thumb at each end.
                let span = (w - THUMB).max(1.);
                let count = marks(range).len();
                for (k, (at, label)) in marks(range).iter().enumerate() {
                    let x = THUMB / 2. + span * ((at - min) / (max - min).max(1.)) as f32;
                    gfx::fill_round(
                        dc,
                        rect(x - 1., mid - 1., x + 1., mid + 1.),
                        1.,
                        c("border-strong"),
                    )?;
                    let text = g.text(t, Style::Tiny, label, 1e4)?;
                    let lx = if k == 0 {
                        x
                    } else if k + 1 == count {
                        x - text.width
                    } else {
                        x - text.width / 2.
                    };
                    gfx::text(
                        dc,
                        &text,
                        lx.round(), // `line-height: 1` on a 10 px label.
                        top + SLIDER_H + 1.,
                        c("text-faint"),
                    )?;
                }
                let (x, y) = self.thumb_at(range, w);
                let thumb = rect(x, y, x + THUMB, y + THUMB);
                gfx::shadow(dc, thumb, THUMB / 2., t.shadow("shadow-sm"))?;
                gfx::fill_round(dc, thumb, THUMB / 2., c("surface-raised"))?;
                gfx::border(dc, thumb, THUMB / 2., 1., c("border-strong"))?;
            }
        }
        Ok(())
    }

    /// The thumb's top-left inside a slider control `w` wide.
    fn thumb_at(&self, range: Range, w: f32) -> (f32, f32) {
        let (min, max) = bounds(range);
        let p = ((self.range_value(range) - min) / (max - min).max(1.)).clamp(0., 1.) as f32;
        let top = SLIDER_TOP + 16.;
        ((w - THUMB) * p, top + (20. - THUMB) / 2.)
    }

    /// The shown controls in DOM (and Tab) order.
    pub fn prop_order(&self) -> Vec<Prop> {
        let mut list = Vec::new();
        for laid in &self.props {
            match laid.row {
                Row::Picker => list.extend((0..SHAPE_TOOLS.len()).map(Prop::Shape)),
                Row::Modes => list.extend((0..MODES.len()).map(Prop::Mode)),
                Row::Note(_) => {}
                Row::Check(check) => list.push(Prop::Check(check)),
                Row::Color(field) => list.extend((0..=CUSTOM).map(|i| Prop::Swatch(field, i))),
                Row::Range(range) => list.push(Prop::Range(range)),
                Row::StylePicker => list.push(Prop::StyleTrigger),
                Row::Textarea => list.push(Prop::Textarea),
                Row::FontSize => list.extend([Prop::Font, Prop::Number(Num::TextSize)]),
                Row::Number(num) => list.push(Prop::Number(num)),
                Row::Formats => list.extend((0..FORMATS).map(Prop::Format)),
            }
        }
        list
    }
}

pub fn check_label(check: Check) -> &'static str {
    match check {
        Check::Stroke => "Stroke",
        Check::Fill => "Filled shape",
        Check::Shadow => "Drop shadow",
        Check::Contiguous => eraser::CONTIGUOUS,
        Check::Plate => "Text background",
    }
}

/// `repeat(auto-fill, minmax(44px, 1fr))` with `row-gap: var(--s-4)` and
/// `padding: var(--s-3) 0`.
fn grid_height(width: f32) -> f32 {
    colors::grid(f64::from(width), colors::CELL).height(8., 6.) as f32
}

/// The edit a change makes to a selected layer: `(live, fold key, edit)`.
fn selected_edit(change: &Change, values: Option<&Values>) -> (bool, &'static str, LayerEdit) {
    let style = |patch: AnnotationStylePatch| LayerEdit::AnnotationStyle { patch };
    let shadow = |patch: DropShadowStylePatch| {
        style(AnnotationStylePatch {
            drop_shadow_style: Some(patch),
            ..Default::default()
        })
    };
    match change.clone() {
        Change::Color(Field::Stroke, color) => (
            false,
            "color",
            style(AnnotationStylePatch {
                color: Some(color),
                ..Default::default()
            }),
        ),
        Change::Color(Field::Fill, color) => (
            false,
            "fill",
            style(AnnotationStylePatch {
                fill: OptionalNullable::Value(color),
                ..Default::default()
            }),
        ),
        Change::Color(Field::Shadow, color) => (
            false,
            "shadow-color",
            shadow(DropShadowStylePatch {
                color: Some(color),
                ..Default::default()
            }),
        ),
        Change::Check(Check::Stroke, on) => (
            false,
            "stroke",
            style(AnnotationStylePatch {
                stroke_enabled: Some(on),
                ..Default::default()
            }),
        ),
        Change::Check(Check::Fill, on) => {
            let color = values.map(|v| v.color.clone()).unwrap_or_default();
            (
                false,
                "fill",
                style(AnnotationStylePatch {
                    fill: if on {
                        OptionalNullable::Value(color)
                    } else {
                        OptionalNullable::Null
                    },
                    ..Default::default()
                }),
            )
        }
        Change::Check(Check::Contiguous, _) => unreachable!("Eraser settings never edit a layer"),
        Change::Check(Check::Plate, _) | Change::Color(Field::Text | Field::Plate, _) => {
            unreachable!("Text settings edit through `EditText`")
        }
        Change::Check(Check::Shadow, on) => (
            false,
            "shadow",
            style(AnnotationStylePatch {
                drop_shadow: Some(on),
                ..Default::default()
            }),
        ),
        Change::Range(Range::Width, value) => (
            true,
            "width",
            style(AnnotationStylePatch {
                stroke_width: Some(value),
                ..Default::default()
            }),
        ),
        Change::Range(Range::Opacity, value) => {
            (true, "opacity", LayerEdit::Opacity { opacity: value })
        }
        Change::Range(Range::ShadowOpacity, value) => (
            true,
            "shadow-opacity",
            shadow(DropShadowStylePatch {
                opacity: Some(value),
                ..Default::default()
            }),
        ),
        Change::Range(_, value) => (
            true,
            "shadow-blur",
            shadow(DropShadowStylePatch {
                blur: Some(value),
                ..Default::default()
            }),
        ),
    }
}

/// Apply a change to shown values before the session confirms it.
fn preview(v: &mut Values, change: &Change) {
    match change.clone() {
        Change::Color(Field::Stroke, color) => v.color = color,
        Change::Color(Field::Fill, color) => v.fill = Some(color),
        Change::Color(Field::Shadow, color) => {
            v.shadow = true;
            v.shadow_color = color;
        }
        Change::Check(Check::Stroke, on) => v.stroke = on,
        Change::Check(Check::Fill, on) => v.fill = on.then(|| v.color.clone()),
        Change::Check(Check::Shadow, on) => v.shadow = on,
        Change::Range(Range::Width, value) => v.width = value,
        Change::Range(Range::Opacity, value) => v.opacity = value,
        Change::Range(Range::ShadowOpacity, value) => {
            v.shadow = true;
            v.shadow_opacity = value;
        }
        Change::Range(Range::ShadowBlur, value) => {
            v.shadow = true;
            v.shadow_blur = value;
        }
        Change::Color(Field::Text, color) => v.text_color = color,
        Change::Color(Field::Plate, color) => v.plate = Some(color),
        Change::Check(Check::Plate, on) => v.plate = on.then(|| TEXT_PLATE.into()),
        Change::Check(Check::Contiguous, _)
        | Change::Range(Range::Tolerance | Range::BrushSize | Range::Softness, _) => {}
    }
}

/// `.screenshot-custom-color`'s `conic-gradient(#f44, #ff4, #4f4, #4ff,
/// #44f, #f4f, #f44)` as thin wedges, clipped to its rounded square.
fn conic(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    r: D2D_RECT_F,
    radius: f32,
) -> Result<()> {
    const STOPS: [u32; 7] = [
        0xff4444, 0xffff44, 0x44ff44, 0x44ffff, 0x4444ff, 0xff44ff, 0xff4444,
    ];
    const WEDGES: usize = 48;
    let (cx, cy) = ((r.left + r.right) / 2., (r.top + r.bottom) / 2.);
    // Just past the corners, so the wedges stay small where a mask is missing.
    let reach = (r.right - r.left).hypot(r.bottom - r.top) / 2. + 0.5;
    let color_at = |p: f32| {
        let at = p * (STOPS.len() - 1) as f32;
        let i = (at.floor() as usize).min(STOPS.len() - 2);
        let f = at - i as f32;
        let channel = |v: u32, shift: u32| ((v >> shift) & 0xff) as f32 / 255.;
        let mix = |shift| channel(STOPS[i], shift) * (1. - f) + channel(STOPS[i + 1], shift) * f;
        D2D1_COLOR_F {
            r: mix(16),
            g: mix(8),
            b: mix(0),
            a: 1.,
        }
    };
    unsafe {
        let tile = factory.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
            rect: r,
            radiusX: radius,
            radiusY: radius,
        })?;
        let mut layer = D2D1_LAYER_PARAMETERS1 {
            contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
            geometricMask: std::mem::ManuallyDrop::new(Some(tile.cast()?)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: windows_numerics::Matrix3x2::identity(),
            opacity: 1.,
            ..Default::default()
        };
        dc.PushLayer(&layer, None::<&ID2D1Layer>);
        let mut result = Ok(());
        for k in 0..WEDGES {
            // CSS conic gradients start at 12 o'clock and run clockwise.
            let a0 = k as f32 / WEDGES as f32;
            let a1 = (k + 1) as f32 / WEDGES as f32 + 0.002;
            let at = |p: f32| {
                let angle = p * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                Vector2 {
                    X: cx + reach * angle.cos(),
                    Y: cy + reach * angle.sin(),
                }
            };
            let wedge = (|| -> Result<()> {
                let path = factory.CreatePathGeometry()?;
                let sink = path.Open()?;
                sink.BeginFigure(Vector2 { X: cx, Y: cy }, D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&[at(a0), at(a1)]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                dc.FillGeometry(&path, &gfx::brush(dc, color_at((a0 + a1) / 2.))?, None);
                Ok(())
            })();
            if wedge.is_err() {
                result = wedge;
                break;
            }
        }
        dc.PopLayer();
        std::mem::ManuallyDrop::drop(&mut layer.geometricMask);
        result
    }
}
