//! Text inputs in Tool properties: shipping's `NumberInput` (a field with
//! its 26 px stepper column, `primitives.css` `.number-input`) and the Text
//! section's `<textarea>`. Both edit through the shared text-field model
//! (`crate::textfield`), with the clipboard and IMM32 as in Preferences
//! Find and the inline label.
//!
//! A field edits only while it has focus: focusing copies the shown value
//! into the model, typing applies valid values as `NumberInput`'s `onChange`
//! does (one live undo step per field), and leaving or Enter commits or
//! reverts like its `commit`. The caret is its own small visual that blinks
//! on the compositor; a collapsed caret moving repaints nothing else.
use captures_app::controls::number;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::core::*;

use super::props::{Num, Prop};
use super::{Editor, Event, Target};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;
use crate::textfield::{self, Key};

/// `textarea { min-height: 82px }`.
pub const TEXTAREA_H: f32 = 82.;

/// One focused property input.
pub struct FieldEdit {
    pub prop: Prop,
    pub field: textfield::Editor,
    /// The text as laid out (number fields scroll it, textareas wrap it).
    layout: Option<IDWriteTextLayout>,
    line_height: f32,
    /// Horizontal scroll (number) or vertical scroll (textarea).
    scroll: f32,
    /// The value the field last took from, or gave to, the document:
    /// shipping's `draft.source`. A different document value (an undo)
    /// replaces the text.
    source: String,
}

/// `String(value)` for a JavaScript number.
pub fn js_number(value: f64) -> String {
    if value.fract() == 0. && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

impl FieldEdit {
    fn multiline(&self) -> bool {
        self.prop == Prop::Textarea
    }
}

impl Editor {
    /// The props control is a text input.
    pub fn is_field(prop: Prop) -> bool {
        matches!(prop, Prop::Number(_) | Prop::Textarea)
    }

    /// The shown text of a field, focused or not.
    pub fn field_text(&self, prop: Prop) -> String {
        if let Some(edit) = self.field.as_ref().filter(|e| e.prop == prop) {
            return String::from_utf16_lossy(edit.field.units());
        }
        self.document_text(prop)
    }

    /// Focus moved onto (`Some`) or off a field: start or end its edit.
    pub(super) fn field_focus(&mut self, focus: Option<Target>, keyboard: bool) {
        let next = match focus {
            Some(Target::Prop(prop)) if Self::is_field(prop) => Some(prop),
            _ => None,
        };
        if self.field.as_ref().map(|e| e.prop) == next {
            return;
        }
        self.end_field();
        let Some(prop) = next else {
            return;
        };
        // A field takes focus from an open label: commit it.
        if self.inline.is_some() {
            self.finish_text();
        }
        let mut field = textfield::Editor::default();
        field.multiline = prop == Prop::Textarea;
        let source = self.field_text(prop);
        field.set_text(&source);
        // Chromium selects an `<input>`'s text when Tab focuses it; a
        // textarea keeps the caret at its start.
        if keyboard && prop != Prop::Textarea {
            field.select_all();
        } else {
            field.move_to(0, false);
        }
        self.field = Some(FieldEdit {
            prop,
            field,
            layout: None,
            line_height: 0.,
            scroll: 0.,
            source,
        });
        self.field_relayout();
    }

    /// The document's value of the focused field.
    fn document_text(&self, prop: Prop) -> String {
        match prop {
            // Shipping's textarea shows an open label's text as it is typed.
            Prop::Textarea => match &self.inline {
                Some(inline) if self.selected.as_deref() == Some(inline.id.as_str()) => {
                    self.text_value()
                }
                _ => self
                    .selected_text()
                    .map(|t| t.text.clone())
                    .unwrap_or_default(),
            },
            Prop::Number(num) => js_number(self.number_value(num)),
            _ => String::new(),
        }
    }

    /// The session presented the document again: like shipping's
    /// `draft.source !== value`, a value the field didn't set replaces its
    /// text, caret at the end.
    pub(super) fn sync_field(&mut self) {
        let Some(prop) = self.field.as_ref().map(|e| e.prop) else {
            return;
        };
        let value = self.document_text(prop);
        let Some(edit) = self.field.as_mut() else {
            return;
        };
        if edit.source == value {
            return;
        }
        edit.source.clone_from(&value);
        if String::from_utf16_lossy(edit.field.units()) != value {
            edit.field.set_text(&value);
            edit.field.move_to(edit.field.len(), false);
            self.field_relayout();
        }
    }

    /// Leave the focused field: numbers commit like `NumberInput`'s blur.
    pub(super) fn end_field(&mut self) {
        let Some(edit) = self.field.take() else {
            return;
        };
        if edit.field.composing() && !self.hwnd.is_invalid() {
            textfield::ime_complete(self.hwnd);
        }
        if let Prop::Number(num) = edit.prop {
            let text = String::from_utf16_lossy(edit.field.units());
            self.commit_number(num, &text);
        }
        self.hide_field_carets();
        self.prop_dirty(edit.prop);
    }

    /// UIA Invoke on another control. Chromium's accessible click focuses
    /// a button, so the focused input blurs and commits its draft (Canvas
    /// W and H included) before the control acts, as a pointer press does.
    /// The field's own steppers leave it focused, like their pointer press.
    pub fn blur_field_for(&mut self, target: Target) {
        let Some(prop) = self.field.as_ref().map(|e| e.prop) else {
            return;
        };
        let own_step =
            matches!(target, Target::Prop(Prop::Step(num, _)) if Prop::Number(num) == prop);
        if own_step || target == Target::Prop(prop) {
            return;
        }
        let focus = self.focusable().contains(&target).then_some(target);
        self.set_focus(focus, false);
    }

    /// Enter or blur: a finite value clamps and applies; anything else
    /// shows the current value again.
    ///
    /// Returns the value the field shows afterwards. Canvas W and H commit
    /// through the session (`commitCanvasSize`, rounded), so the shown value
    /// is the committed one before the reply arrives.
    fn commit_number(&mut self, num: Num, text: &str) -> String {
        let parsed = text.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        let (min, max) = num.bounds();
        let committed = parsed.map(|value| {
            let value = if num.commits_once() {
                value.round()
            } else {
                value
            };
            value.clamp(min, max)
        });
        if let Some(value) = committed {
            self.set_number(num, value);
        }
        self.own_source(Prop::Number(num));
        self.prop_dirty(Prop::Number(num));
        match committed {
            Some(value) if num.commits_once() => js_number(value),
            _ => js_number(self.number_value(num)),
        }
    }

    /// The field's text box (content area) relative to its control.
    fn field_box(&self, prop: Prop) -> Rect {
        let r = self.target_rect(Target::Prop(prop));
        let (s3, s4) = (self.t.n("s-3"), self.t.n("s-4"));
        match prop {
            // `padding: var(--s-3) var(--s-4)` inside the 1 px border.
            Prop::Textarea => Rect::new(1. + s4, 1. + s3, r.w - 2. - 2. * s4, r.h - 2. - 2. * s3),
            // `.is-compact input { padding: var(--s-1) var(--s-3) }`, which
            // comes after `.has-steppers`' padding-right.
            Prop::Number(num) if num.is_canvas() => {
                let s3 = self.t.n("s-3");
                Rect::new(
                    1. + s3,
                    1.,
                    (r.w - 2. - 2. * s3 - num.steppers_w()).max(1.),
                    r.h - 2.,
                )
            }
            // `.has-steppers input { padding-right: var(--s-2) }`.
            Prop::Number(num) if self.num_steppers(num) => Rect::new(
                1. + s4,
                1.,
                (r.w - 2. - s4 - self.t.n("s-2") - num.steppers_w()).max(1.),
                r.h - 2.,
            ),
            _ => Rect::new(1. + s4, 1., (r.w - 2. - 2. * s4).max(1.), r.h - 2.),
        }
    }

    /// Lay the focused field's text out again and place its caret.
    pub(super) fn field_relayout(&mut self) {
        let Some(edit) = self.field.as_ref() else {
            return;
        };
        let prop = edit.prop;
        let multiline = edit.multiline();
        let b = self.field_box(prop);
        let style = Self::field_style(prop);
        let display = String::from_utf16_lossy(&edit.field.display());
        let composition = edit.field.composition();
        let width = if multiline { b.w } else { 1e5 };
        let Ok(text) = self.g.text(&self.t, style, &display, width) else {
            return;
        };
        if let Some((start, length)) = composition {
            unsafe {
                let _ = text.layout.SetUnderline(
                    true,
                    DWRITE_TEXT_RANGE {
                        startPosition: start as u32,
                        length: length as u32,
                    },
                );
            }
        }
        let line = self
            .g
            .text(&self.t, style, "Ag", 1e5)
            .map_or(text.height, |t| t.height);
        if let Some(edit) = self.field.as_mut() {
            edit.layout = Some(text.layout);
            edit.line_height = line;
        }
        self.field_follow();
        self.prop_dirty(prop);
        self.place_field_caret();
    }

    /// `(x, top)` of a display position in the laid-out text, unscrolled.
    fn field_position(&self, position: usize) -> (f32, f32) {
        let Some(layout) = self.field.as_ref().and_then(|e| e.layout.as_ref()) else {
            return (0., 0.);
        };
        let (mut x, mut y) = (0., 0.);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ =
                layout.HitTestTextPosition(position as u32, false, &mut x, &mut y, &mut metrics);
        }
        (x, metrics.top)
    }

    /// Scroll so the caret stays inside the text box.
    fn field_follow(&mut self) {
        let Some(edit) = self.field.as_ref() else {
            return;
        };
        let (prop, multiline, line) = (edit.prop, edit.multiline(), edit.line_height);
        let caret = edit.field.display_caret();
        let (x, top) = self.field_position(caret);
        let b = self.field_box(prop);
        let extent = edit.layout.as_ref().map_or(0., |l| {
            let mut m = DWRITE_TEXT_METRICS::default();
            unsafe {
                let _ = l.GetMetrics(&mut m);
            }
            if multiline { m.height } else { m.width + 1. }
        });
        let Some(edit) = self.field.as_mut() else {
            return;
        };
        let before = edit.scroll;
        let (at, size, view) = if multiline {
            (top, line, b.h)
        } else {
            (x, 1., b.w)
        };
        if at + size - edit.scroll > view {
            edit.scroll = at + size - view;
        } else if at < edit.scroll {
            edit.scroll = at;
        }
        edit.scroll = edit.scroll.clamp(0., (extent - view).max(0.));
        if edit.scroll != before {
            self.prop_dirty(prop);
        }
    }

    /// The focused field's caret visual: Canvas W and H have theirs in the
    /// header, the rest in the Properties body.
    pub(super) fn field_caret_node(&self) -> usize {
        match self.field.as_ref().map(|e| e.prop) {
            Some(Prop::Number(num)) if num.is_canvas() => self.header_caret,
            _ => self.field_caret,
        }
    }

    pub(super) fn hide_field_carets(&mut self) {
        self.set_visible(self.field_caret, false);
        self.set_visible(self.header_caret, false);
    }

    /// `.is-compact input { text-align: center }`: the text's x offset in
    /// its box while it fits.
    fn field_shift(&self, prop: Prop, width: f32) -> f32 {
        match prop {
            Prop::Number(num) if num.is_canvas() => ((self.field_box(prop).w - width) / 2.).max(0.),
            _ => 0.,
        }
    }

    /// The focused field's laid-out width.
    fn field_width(&self) -> f32 {
        self.field
            .as_ref()
            .and_then(|e| e.layout.as_ref())
            .map_or(0., |l| {
                let mut m = DWRITE_TEXT_METRICS::default();
                unsafe {
                    let _ = l.GetMetrics(&mut m);
                }
                m.widthIncludingTrailingWhitespace
            })
    }

    /// The caret visual at the caret, in its parent's coordinates; hidden
    /// over a selection.
    pub(super) fn place_field_caret(&mut self) {
        let Some(edit) = self.field.as_ref() else {
            self.hide_field_carets();
            return;
        };
        let (a, b) = edit.field.selection();
        if a != b && !edit.field.composing() {
            self.hide_field_carets();
            return;
        }
        let (prop, multiline, scroll, line) =
            (edit.prop, edit.multiline(), edit.scroll, edit.line_height);
        let (x, top) = self.field_position(edit.field.display_caret());
        let Some(control) = self.prop_rect(prop) else {
            return;
        };
        let bx = self.field_box(prop);
        let shift = self.field_shift(prop, self.field_width());
        let (cx, cy) = if multiline {
            (bx.x + x, bx.y + top - scroll)
        } else {
            (bx.x + shift + x - scroll, (control.h - line) / 2.)
        };
        // Outside the box (a textarea scrolled away): no caret.
        let inside = cy + line > bx.y && cy < bx.y + bx.h;
        let r = Rect::new(control.x + cx, control.y + cy, 1., line.max(4.));
        let node = self.field_caret_node();
        let other = if node == self.field_caret {
            self.header_caret
        } else {
            self.field_caret
        };
        self.set_visible(other, false);
        let moved = self.nodes[node].rect != r || !self.nodes[node].visible;
        self.set_rect(node, r);
        self.set_visible(node, inside);
        if moved && inside {
            self.caret_blink = true;
        }
        self.need = true;
    }

    /// `.number-input input`: mono; the textarea and the crop's plain
    /// inputs: `--text-sm`.
    fn field_style(prop: Prop) -> Style {
        match prop {
            // `.is-compact input { font-size: var(--text-xs) }`.
            Prop::Number(num) if num.is_canvas() => Style::MonoSmall,
            Prop::Number(num) if !num.read_only() => Style::Mono,
            _ => Style::Option,
        }
    }

    /// Apply an edit to the focused field's model.
    fn field_edit(&mut self, edit: impl FnOnce(&mut textfield::Editor) -> textfield::Change) {
        let Some(focused) = self.field.as_mut() else {
            return;
        };
        let selection = focused.field.selection();
        let before = matches!(focused.prop, Prop::Number(num) if num.read_only())
            .then(|| String::from_utf16_lossy(focused.field.units()));
        let mut change = edit(&mut focused.field);
        // A `readOnly` input keeps its text; only the caret and selection move.
        if let Some(before) = before
            && change.text
        {
            focused.field.set_text(&before);
            let end = focused.field.len();
            focused
                .field
                .select(selection.0.min(end), selection.1.min(end));
            change.text = false;
            change.caret = true;
        }
        let prop = focused.prop;
        let text = String::from_utf16_lossy(focused.field.units());
        let after = focused.field.selection();
        if change.text {
            self.field_relayout();
            self.field_changed(prop, text);
            self.events.push(Event::Value(Target::Prop(prop)));
        } else if change.caret {
            self.field_follow();
            if selection != after && (selection.0 != selection.1 || after.0 != after.1) {
                self.prop_dirty(prop);
            }
            self.place_field_caret();
        }
    }

    /// Typing: a textarea updates the label live; a number field applies a
    /// valid in-range value (`NumberInput.handleInput`).
    fn field_changed(&mut self, prop: Prop, text: String) {
        match prop {
            Prop::Textarea => self.apply_text(super::props::TextChange::Content(text)),
            // `onTextChange`: Canvas W and H only hold a draft until Enter,
            // blur or a stepper commits it.
            Prop::Number(num) if num.commits_once() => {}
            Prop::Number(num) => {
                let parsed = if text.trim().is_empty() {
                    None
                } else {
                    text.trim().parse::<f64>().ok()
                };
                let (min, max) = num.bounds();
                if let Some(value) = parsed.filter(|v| v.is_finite() && (min..=max).contains(v)) {
                    self.set_number(num, value);
                }
            }
            _ => {}
        }
        self.own_source(prop);
    }

    /// The field set the document's value itself: its reply must not reset
    /// the text being typed.
    fn own_source(&mut self, prop: Prop) {
        let value = self.document_text(prop);
        if let Some(edit) = self.field.as_mut().filter(|e| e.prop == prop) {
            edit.source = value;
        }
    }

    /// A stepper or ArrowUp/ArrowDown (`NumberInput.nudge`).
    pub fn nudge(&mut self, num: Num, up: bool) {
        let text = self.field_text(Prop::Number(num));
        let shown = if text.trim().is_empty() {
            js_number(self.number_value(num))
        } else {
            text
        };
        let (min, max) = num.bounds();
        let next = number::step_from(&shown, 1., up, Some(min), Some(max));
        self.set_number(num, next);
        self.own_source(Prop::Number(num));
        if let Some(edit) = self.field.as_mut().filter(|e| e.prop == Prop::Number(num)) {
            edit.field.set_text(&js_number(next));
            edit.field.move_to(edit.field.len(), false);
            self.field_relayout();
        }
        self.prop_dirty(Prop::Number(num));
        self.events
            .push(Event::Value(Target::Prop(Prop::Number(num))));
    }

    /// Whether a stepper is disabled at its bound.
    pub fn step_disabled(&self, num: Num, up: bool) -> bool {
        let (min, max) = num.bounds();
        let value = self.number_value(num);
        if up { value >= max } else { value <= min }
    }

    /// Keys for the focused field; false lets the editor handle them.
    pub(super) fn field_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
        let Some(edit) = self.field.as_ref() else {
            return false;
        };
        let prop = edit.prop;
        let multiline = edit.multiline();
        let line_edge = |f: &textfield::Editor, end: bool| {
            let units = f.units();
            let caret = f.caret();
            if end {
                units[caret..]
                    .iter()
                    .position(|&u| u == u16::from(b'\n'))
                    .map_or(units.len(), |i| caret + i)
            } else {
                units[..caret]
                    .iter()
                    .rposition(|&u| u == u16::from(b'\n'))
                    .map_or(0, |i| i + 1)
            }
        };
        match vk {
            VK_TAB => return false,
            VK_RETURN if multiline => self.field_edit(|f| f.insert("\n")),
            VK_RETURN => {
                if let Prop::Number(num) = prop {
                    let text = self.field_text(prop);
                    let shown = self.commit_number(num, &text);
                    self.field_edit(|f| {
                        let mut change = textfield::Change::default();
                        if String::from_utf16_lossy(f.units()) != shown {
                            change = f.set_text(&shown);
                        }
                        change
                    });
                }
            }
            VK_UP | VK_DOWN if !multiline => {
                if let Prop::Number(num) = prop {
                    self.nudge(num, vk == VK_UP);
                }
            }
            VK_UP | VK_DOWN => {
                let caret = edit.field.display_caret();
                let (x, top) = self.field_position(caret);
                let line = edit.line_height;
                let y = if vk == VK_UP {
                    top - line / 2.
                } else {
                    top + line * 1.5
                };
                let display = self.field_hit_local(x, y);
                self.field_edit(|f| {
                    let position = f.text_position(display);
                    f.move_to(position, shift)
                });
            }
            VK_LEFT => self.field_edit(|f| f.key(Key::Left, shift, ctrl)),
            VK_RIGHT => self.field_edit(|f| f.key(Key::Right, shift, ctrl)),
            VK_HOME | VK_END if multiline && !ctrl => self.field_edit(|f| {
                let to = line_edge(f, vk == VK_END);
                f.move_to(to, shift)
            }),
            VK_HOME => self.field_edit(|f| f.key(Key::Home, shift, false)),
            VK_END => self.field_edit(|f| f.key(Key::End, shift, false)),
            VK_BACK => self.field_edit(|f| f.key(Key::Backspace, false, ctrl)),
            VK_DELETE if shift && !ctrl => self.field_cut(),
            VK_DELETE => self.field_edit(|f| f.key(Key::Delete, false, ctrl)),
            VK_A if ctrl => self.field_edit(|f| f.key(Key::SelectAll, false, false)),
            VK_Z if ctrl && !shift => self.field_edit(|f| f.key(Key::Undo, false, false)),
            VK_Z | VK_Y if ctrl => self.field_edit(|f| f.key(Key::Redo, false, false)),
            VK_C | VK_INSERT if ctrl => {
                let text = edit.field.selected_text();
                if !text.is_empty() {
                    textfield::copy(self.hwnd, &text);
                }
            }
            VK_X if ctrl => self.field_cut(),
            VK_V if ctrl => self.field_paste(),
            VK_INSERT if shift => self.field_paste(),
            VK_ESCAPE => return false,
            // Everything else types through `WM_CHAR` or does nothing, so
            // tool letters never switch tools mid-entry.
            _ if ctrl => return false,
            _ => {}
        }
        true
    }

    fn field_cut(&mut self) {
        let hwnd = self.hwnd;
        self.field_edit(|f| match f.cut() {
            Some(text) => {
                textfield::copy(hwnd, &text);
                textfield::Change {
                    text: true,
                    caret: true,
                }
            }
            None => textfield::Change::default(),
        });
    }

    fn field_paste(&mut self) {
        if let Some(text) = textfield::paste(self.hwnd) {
            self.field_edit(|f| f.insert(&text));
        }
    }

    /// `WM_CHAR` for the focused field.
    pub(super) fn field_char(&mut self, unit: u16) {
        let units = match unit {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(unit);
                return;
            }
            0xDC00..=0xDFFF => match self.high_surrogate.take() {
                Some(high) => vec![high, unit],
                None => return,
            },
            _ => vec![unit],
        };
        let text = String::from_utf16_lossy(&units);
        if text.chars().any(char::is_control) {
            return;
        }
        self.field_edit(|f| f.insert(&text));
    }

    pub(super) fn field_ime(&mut self, flags: u32) {
        let hwnd = self.hwnd;
        self.field_edit(|f| textfield::ime_composition(hwnd, flags, f));
    }

    pub(super) fn field_ime_end(&mut self) {
        self.field_edit(|f| f.commit_composition(&[]));
    }

    /// UI Automation `SetValue`.
    pub fn set_field_value(&mut self, prop: Prop, value: &str) {
        if self.field.as_ref().map(|e| e.prop) != Some(prop) {
            self.set_focus(Some(Target::Prop(prop)), false);
        }
        self.field_edit(|f| f.set_text(value));
    }

    /// A display position under a point in the text box (unscrolled box
    /// coordinates).
    fn field_hit_local(&self, x: f32, y: f32) -> usize {
        let Some(edit) = self.field.as_ref() else {
            return 0;
        };
        let Some(layout) = edit.layout.as_ref() else {
            return 0;
        };
        let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        let y = if edit.multiline() {
            y
        } else {
            edit.line_height / 2.
        };
        unsafe {
            let _ = layout.HitTestPoint(x, y, &mut trailing, &mut inside, &mut metrics);
        }
        metrics.textPosition as usize
            + if trailing.as_bool() {
                metrics.length as usize
            } else {
                0
            }
    }

    /// A press or drag in the focused field at a client point.
    pub(super) fn field_point(&mut self, x: f32, y: f32, double: bool, extend: bool) {
        let Some(edit) = self.field.as_ref() else {
            return;
        };
        if edit.field.composing() {
            textfield::ime_complete(self.hwnd);
        }
        let prop = edit.prop;
        let scroll = edit.scroll;
        let multiline = edit.multiline();
        let control = self.target_rect(Target::Prop(prop));
        let b = self.field_box(prop);
        let (lx, ly) = (x - control.x - b.x, y - control.y - b.y);
        let display = if multiline {
            self.field_hit_local(lx, ly + scroll)
        } else {
            let shift = self.field_shift(prop, self.field_width());
            self.field_hit_local(lx - shift + scroll, 0.)
        };
        self.field_edit(|f| {
            let position = f.text_position(display);
            if double {
                let (a, b) = f.word_at(position);
                f.select(a, b)
            } else {
                f.move_to(position, extend)
            }
        });
        self.text_drag = !double;
    }

    /// The field's IME anchor in client pixels.
    pub(super) fn field_ime_rect(&self) -> Option<(i32, i32, i32)> {
        self.field.as_ref()?;
        let r = self.absolute(self.field_caret_node());
        let s = self.scale;
        let px = |v: f32| (v * s).round() as i32;
        Some((px(r.x), px(r.y), px(r.h)))
    }

    // ---- drawing ----

    /// A number field: frame, text and the stepper column.
    pub(super) fn draw_number(
        &self,
        dc: &ID2D1DeviceContext,
        num: Num,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        let prop = Prop::Number(num);
        let focused = self.focus == Some(Target::Prop(prop));
        if lit && focused {
            return Ok(());
        }
        if num.is_canvas() {
            return self.draw_canvas_number(dc, num, lit, (w, h));
        }
        gfx::fill_round(dc, all, radius, c("surface-field"))?;
        let border = if focused {
            c("theme-accent")
        } else if lit {
            c("border-strong")
        } else {
            c("control-border")
        };
        gfx::border(dc, all, radius, 1., border)?;
        if !self.num_steppers(num) {
            return if lit {
                Ok(())
            } else {
                self.draw_field_text(dc, prop, (w, h))
            };
        }
        // `.number-input-steppers`.
        let sx = w - 1. - num.steppers_w();
        gfx::fill(dc, rect(sx, 1., w - 1., h - 1.), c("surface-hover"))?;
        gfx::fill(dc, rect(sx, 1., sx + 1., h - 1.), c("border-subtle"))?;
        let mid = (h / 2.).round();
        gfx::fill(dc, rect(sx + 1., mid, w - 1., mid + 1.), c("border-subtle"))?;
        if lit {
            return Ok(());
        }
        self.draw_field_text(dc, prop, (w, h))
    }

    /// `.screenshot-canvas-dim .number-input.is-compact`: borderless and
    /// transparent until hovered (`--border-strong`) or focused (accent
    /// border on `--editor-field`, which is `--surface-field`); the stepper
    /// column always shows.
    fn draw_canvas_number(
        &self,
        dc: &ID2D1DeviceContext,
        num: Num,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-sm");
        let prop = Prop::Number(num);
        if lit {
            return gfx::border(dc, all, radius, 1., c("border-strong"));
        }
        if self.focus == Some(Target::Prop(prop)) {
            gfx::fill_round(dc, all, radius, c("surface-field"))?;
            gfx::border(dc, all, radius, 1., c("theme-accent"))?;
        }
        let sw = num.steppers_w();
        let sx = w - 1. - sw;
        // The column's corners follow the field's (`overflow: hidden`).
        let column = rect(sx, 1., w - 1., h - 1.);
        unsafe { dc.PushAxisAlignedClip(&column, D2D1_ANTIALIAS_MODE_ALIASED) };
        let filled = gfx::fill_round(
            dc,
            rect(sx - radius, 1., w - 1., h - 1.),
            (radius - 1.).max(0.),
            c("surface-hover"),
        );
        unsafe { dc.PopAxisAlignedClip() };
        filled?;
        gfx::fill(dc, rect(sx, 1., sx + 1., h - 1.), c("border-subtle"))?;
        let mid = (h / 2.).round();
        gfx::fill(dc, rect(sx + 1., mid, w - 1., mid + 1.), c("border-subtle"))?;
        self.draw_field_text(dc, prop, (w, h))
    }

    /// A stepper: `M3 7.5 6 4.5 9 7.5` (up) in a 12-unit box at 11 px.
    pub(super) fn draw_step(
        &self,
        dc: &ID2D1DeviceContext,
        num: Num,
        up: bool,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let disabled = self.step_disabled(num, up);
        if lit {
            if disabled {
                return Ok(());
            }
            gfx::fill(dc, rect(0., 0., w, h), t.color("surface-active"))?;
        }
        let ink = if lit {
            t.color("text")
        } else {
            t.color("text-subtle")
        };
        let ink = if disabled {
            windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
                a: ink.a * 0.3,
                ..ink
            }
        } else {
            ink
        };
        let s = 11. / 12.;
        let (ox, oy) = ((w - 11.) / 2., (h - 11.) / 2.);
        let p = |x: f32, y: f32| (ox + x * s, oy + y * s);
        let points = if up {
            [p(3., 7.5), p(6., 4.5), p(9., 7.5)]
        } else {
            [p(3., 4.5), p(6., 7.5), p(9., 4.5)]
        };
        super::polyline(dc, &self.g, &points, false, ink, 2.)
    }

    /// The textarea: frame and wrapped text.
    pub(super) fn draw_textarea(
        &self,
        dc: &ID2D1DeviceContext,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        if lit {
            return Ok(());
        }
        let t = &self.t;
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        let focused = self.focus == Some(Target::Prop(Prop::Textarea));
        gfx::fill_round(dc, all, radius, t.color("surface-field"))?;
        let border = if focused {
            t.color("theme-accent")
        } else {
            t.color("control-border")
        };
        gfx::border(dc, all, radius, 1., border)?;
        self.draw_field_text(dc, Prop::Textarea, (w, h))
    }

    /// The text of a field, its selection and IME underline, clipped to
    /// the text box.
    fn draw_field_text(
        &self,
        dc: &ID2D1DeviceContext,
        prop: Prop,
        (_, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let b = self.field_box(prop);
        let clip = rect(b.x, b.y, b.x + b.w, b.y + b.h);
        unsafe { dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED) };
        let drawn = (|| -> Result<()> {
            match self.field.as_ref().filter(|e| e.prop == prop) {
                Some(edit) => {
                    let Some(layout) = edit.layout.as_ref() else {
                        return Ok(());
                    };
                    let (ox, oy) = if edit.multiline() {
                        (b.x, b.y - edit.scroll)
                    } else {
                        let shift = self.field_shift(prop, self.field_width());
                        (
                            b.x + shift - edit.scroll,
                            ((h - edit.line_height) / 2.).max(0.),
                        )
                    };
                    let (a, z) = edit.field.selection();
                    if a != z && !edit.field.composing() {
                        let mut buffer = [DWRITE_HIT_TEST_METRICS::default(); 32];
                        let mut count = 0;
                        unsafe {
                            let _ = layout.HitTestTextRange(
                                a as u32,
                                (z - a) as u32,
                                0.,
                                0.,
                                Some(&mut buffer),
                                &mut count,
                            );
                        }
                        for m in &buffer[..(count as usize).min(32)] {
                            let r = rect(
                                ox + m.left,
                                oy + m.top,
                                ox + m.left + m.width.max(3.),
                                oy + m.top + m.height,
                            );
                            gfx::fill(dc, r, t.color_alpha("theme-accent", 0.38))?;
                        }
                    }
                    unsafe {
                        dc.DrawTextLayout(
                            windows_numerics::Vector2 { X: ox, Y: oy },
                            layout,
                            &gfx::brush(dc, t.color("text"))?,
                            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                        );
                    }
                }
                None => {
                    let shown = self.field_text(prop);
                    if prop == Prop::Textarea {
                        let text = self.g.text(t, Style::Option, &shown, b.w)?;
                        gfx::text(dc, &text, b.x, b.y, t.color("text"))?;
                    } else {
                        // A disabled input (a locked layer's geometry) fades.
                        let ink = match prop {
                            Prop::Number(num) if self.num_disabled(num) => t.color("text-faint"),
                            _ => t.color("text"),
                        };
                        let text = self.g.text(t, Self::field_style(prop), &shown, 1e5)?;
                        let x = b.x + self.field_shift(prop, text.width);
                        gfx::text(dc, &text, x, ((h - text.height) / 2.).max(0.), ink)?;
                    }
                }
            }
            Ok(())
        })();
        unsafe { dc.PopAxisAlignedClip() };
        drawn
    }
}
