//! The Text tool: shipping's inline textarea over the canvas, and the
//! selection grips that resize a layer.
//!
//! The session omits the edited layer from its pixels while a text input is
//! active, so this host draws it, as shipping's textarea does: the plate,
//! the text in the layer's bundled face (an in-memory DirectWrite font set
//! from `editor_fonts::bundled`), the selection and the IME composition, in
//! the overlay, at the session's `inline_editor_layout` geometry. Typing goes
//! through the shared text-field model in multiline mode and is coalesced
//! into at most one in-flight `UpdateTextInput`, as the GTK4 host does; only
//! Finish publishes document history. Escape and leaving the field commit
//! (shipping has no cancel). The caret is its own small visual that blinks
//! on the compositor.
//!
//! Grips resize through `LayerEdit::Resize`: the drag previews on the worker
//! (`preview_drawing_with_geometry`, one request in flight) and commits once
//! on release, so one drag is one undo step. Side grips set a label's wrap
//! width, top and bottom set its size, corners do both, and Shift on a corner
//! keeps proportions; those rules live in the shared `ResizeDrag`.
use std::cell::RefCell;

use captures_app::editor::{Element, LayerEdit, Point, ResizeHandle, TextElement};
use captures_app::editor_session::{Request, TextCreate, TextInputTarget};
use captures_app::editor_text::{self, InlineEditorLayout};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use super::{Editor, Event, Gesture, Job, hex_color};
use crate::gfx::{self, rect};
use crate::scene::Rect;
use crate::textfield::{self, Key};

/// The named treatment new labels start with, as shipping.
const NEW_TEXT_PRESET: &str = "rounded-box";
/// Selection grips: 11 px accent dots with a white rim.
const GRIP: f32 = 5.5;

/// One inline text transaction.
pub struct Inline {
    pub id: String,
    /// Begin was accepted; the session holds a transient layer.
    started: bool,
    /// The text the session last accepted.
    accepted: String,
    /// A request is with the worker.
    in_flight: bool,
    /// Finish was requested (always a commit, as shipping).
    finishing: bool,
    layout: Option<InlineEditorLayout>,
    element: Option<TextElement>,
    pub field: textfield::Editor,
}

/// The session's active input, as the worker published it.
#[derive(Clone, Debug)]
pub struct ActiveInput {
    pub input_id: String,
    pub layer_id: String,
    pub layout: Option<InlineEditorLayout>,
}

/// The inline text laid out in well-relative DIPs.
struct View {
    layout: IDWriteTextLayout,
    /// The text box's top-left, unrotated.
    origin: (f32, f32),
    frame: Rect,
    rotation: f32,
    radius: f32,
    color: D2D1_COLOR_F,
    plate: Option<D2D1_COLOR_F>,
}

impl View {
    fn center(&self) -> (f32, f32) {
        (
            self.frame.x + self.frame.w / 2.,
            self.frame.y + self.frame.h / 2.,
        )
    }

    /// Rotate a well point about the frame centre (`sign` -1 undoes it).
    fn rotate(&self, (x, y): (f32, f32), sign: f32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let (sin, cos) = (self.rotation * sign).sin_cos();
        let (dx, dy) = (x - cx, y - cy);
        (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
    }

    fn matrix(&self) -> Matrix3x2 {
        let (cx, cy) = self.center();
        let (sin, cos) = self.rotation.sin_cos();
        Matrix3x2 {
            M11: cos,
            M12: sin,
            M21: -sin,
            M22: cos,
            M31: cx - cx * cos + cy * sin,
            M32: cy - cx * sin - cy * cos,
        }
    }

    /// The display position under a well point.
    fn hit(&self, point: (f32, f32)) -> usize {
        let (x, y) = self.rotate(point, -1.);
        let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            if self
                .layout
                .HitTestPoint(
                    x - self.origin.0,
                    y - self.origin.1,
                    &mut trailing,
                    &mut inside,
                    &mut metrics,
                )
                .is_err()
            {
                return 0;
            }
        }
        (metrics.textPosition
            + if trailing.as_bool() {
                metrics.length
            } else {
                0
            }) as usize
    }

    /// `(x, top, height)` of a display position, layout-relative.
    fn position(&self, position: usize) -> (f32, f32, f32) {
        let (mut x, mut y) = (0., 0.);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self.layout.HitTestTextPosition(
                position as u32,
                false,
                &mut x,
                &mut y,
                &mut metrics,
            );
        }
        (x, metrics.top, metrics.height)
    }

    /// Highlight rectangles for a display range, layout-relative.
    fn range(&self, start: usize, len: usize) -> Vec<DWRITE_HIT_TEST_METRICS> {
        let mut count = 0;
        unsafe {
            // The first call only counts (it fails for want of a buffer).
            let _ =
                self.layout
                    .HitTestTextRange(start as u32, len as u32, 0., 0., None, &mut count);
            let mut out = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
            if self
                .layout
                .HitTestTextRange(start as u32, len as u32, 0., 0., Some(&mut out), &mut count)
                .is_err()
            {
                return Vec::new();
            }
            out.truncate(count as usize);
            out
        }
    }

    /// The rotated frame and text extent, for the overlay's dirty box.
    fn corners(&self) -> Vec<(f32, f32)> {
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = self.layout.GetMetrics(&mut metrics);
        }
        let f = self.frame;
        let right = (f.x + f.w)
            .max(self.origin.0 + metrics.left + metrics.widthIncludingTrailingWhitespace);
        let bottom = (f.y + f.h).max(self.origin.1 + metrics.top + metrics.height);
        [(f.x, f.y), (right, f.y), (right, bottom), (f.x, bottom)]
            .into_iter()
            .map(|p| self.rotate(p, 1.))
            .collect()
    }
}

thread_local! {
    /// The bundled editor faces as one in-memory collection; `None` inside
    /// when this DirectWrite cannot load fonts from memory (Windows 10
    /// before 1903).
    static BUNDLED: RefCell<Option<Option<IDWriteFontCollection>>> = const { RefCell::new(None) };
}

fn bundled_collection(dwrite: &IDWriteFactory) -> Option<IDWriteFontCollection> {
    BUNDLED.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| {
                load_bundled(dwrite)
                    .map_err(|error| eprintln!("bundled editor fonts unavailable: {error}"))
                    .ok()
            })
            .clone()
    })
}

fn load_bundled(dwrite: &IDWriteFactory) -> Result<IDWriteFontCollection> {
    unsafe {
        let factory: IDWriteFactory5 = dwrite.cast()?;
        let loader = factory.CreateInMemoryFontFileLoader()?;
        factory.RegisterFontFileLoader(&loader)?;
        let builder: IDWriteFontSetBuilder1 = factory.CreateFontSetBuilder()?.cast()?;
        for bytes in captures_app::editor_fonts::bundled().files.values() {
            // A null owner makes the loader keep its own copy.
            let file = loader.CreateInMemoryFontFileReference(
                dwrite,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                None::<&IUnknown>,
            )?;
            builder.AddFontFile(&file)?;
        }
        let set = builder.CreateFontSet()?;
        factory.CreateFontCollectionFromFontSet(&set)?.cast()
    }
}

/// `(ascent, descent)` per em of `family` in `collection`.
fn em_metrics(
    collection: &IDWriteFontCollection,
    family: &HSTRING,
    weight: DWRITE_FONT_WEIGHT,
    style: DWRITE_FONT_STYLE,
) -> Option<(f32, f32)> {
    unsafe {
        let (mut index, mut exists) = (0, BOOL(0));
        collection
            .FindFamilyName(family, &mut index, &mut exists)
            .ok()?;
        if !exists.as_bool() {
            return None;
        }
        let font = collection
            .GetFontFamily(index)
            .ok()?
            .GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style)
            .ok()?;
        let mut m = DWRITE_FONT_METRICS::default();
        font.GetMetrics(&mut m);
        let em = f32::from(m.designUnitsPerEm.max(1));
        Some((f32::from(m.ascent) / em, f32::from(m.descent) / em))
    }
}

impl Editor {
    pub fn text_active(&self) -> bool {
        self.inline.is_some()
    }

    fn text_element(&self, id: &str) -> Option<TextElement> {
        self.presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find_map(|element| match element {
                Element::Text(text) if text.base.id == id => Some(text.clone()),
                _ => None,
            })
    }

    /// An unlocked text layer under a document point.
    fn text_at(&self, point: Point, slop: f64) -> Option<String> {
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        match self
            .presented
            .as_ref()?
            .document
            .hit_test(point, slop / scale)
            .ok()
            .flatten()
        {
            Some(Element::Text(text)) if !text.base.locked => Some(text.base.id.clone()),
            _ => None,
        }
    }

    /// A Text click: edit the label under the pointer, or place a new one.
    pub(super) fn begin_text(&mut self, point: Point) {
        let target = match self.text_at(point, 8.) {
            Some(id) => TextInputTarget::Existing { id },
            None => {
                let Some(p) = self.presented.as_ref() else {
                    return;
                };
                let style = self
                    .defaults
                    .as_ref()
                    .map_or_else(|| p.style.clone(), |d| d.style.clone());
                TextInputTarget::New {
                    create: TextCreate {
                        point,
                        text: String::new(),
                        font_size: p.text_size,
                        font_family: "sans".into(),
                        // Shipping places new text in the drawing color.
                        color: style.color.clone(),
                        style_preset: Some(NEW_TEXT_PRESET.into()),
                        drop_shadow: style.drop_shadow,
                        drop_shadow_style: style.drop_shadow_style.clone(),
                    },
                }
            }
        };
        self.start_text(target);
    }

    /// Double-click with Select edits an existing label.
    pub(super) fn edit_text_at(&mut self, point: Point) -> bool {
        let Some(id) = self.text_at(point, 4.) else {
            return false;
        };
        self.start_text(TextInputTarget::Existing { id });
        true
    }

    fn start_text(&mut self, target: TextInputTarget) {
        // Until Begin is accepted, draw the existing layer or the unfitted
        // layer a click creates, so the field appears at once.
        let element = match &target {
            TextInputTarget::Existing { id } => self.text_element(id),
            TextInputTarget::New { create } => {
                captures_app::editor_session::new_text_element(String::new(), create).ok()
            }
        };
        let Some(element) = element else {
            return;
        };
        self.input_counter += 1;
        let id = format!("win-text-{}", self.input_counter);
        let mut field = textfield::Editor::default();
        field.multiline = true;
        field.set_text(&element.text);
        field.select_all();
        self.inline = Some(Inline {
            id: id.clone(),
            started: false,
            accepted: element.text.clone(),
            in_flight: true,
            finishing: false,
            layout: editor_text::inline_editor_layout(&element).ok(),
            element: Some(element),
            field,
        });
        // The inline field takes focus from any control.
        self.set_focus(None, false);
        self.events.push(Event::InlineFocus);
        self.events.push(Event::Structure);
        self.nodes[self.caret].dirty = true;
        self.submit(Request::BeginTextInput {
            input_id: id,
            target,
        });
        self.overlay_changed();
    }

    /// Escape, a click elsewhere, Tab or another tool: commit the label.
    pub(super) fn finish_text(&mut self) {
        if let Some(inline) = self.inline.as_mut() {
            inline.finishing = true;
            if inline.field.composing() && !self.hwnd.is_invalid() {
                textfield::ime_complete(self.hwnd);
            }
        }
        self.drain_text();
    }

    /// Send the newest text, or Finish, when the worker is free.
    fn drain_text(&mut self) {
        let Some(inline) = self.inline.as_mut() else {
            return;
        };
        if inline.in_flight {
            return;
        }
        let text = String::from_utf16_lossy(inline.field.units());
        let request = if !inline.started {
            // Begin failed: nothing reached the document; drop the field.
            self.close_text();
            return;
        } else if text != inline.accepted {
            Request::UpdateTextInput {
                input_id: inline.id.clone(),
                text,
            }
        } else if inline.finishing {
            Request::FinishTextInput {
                input_id: inline.id.clone(),
                commit: true,
            }
        } else {
            return;
        };
        inline.in_flight = true;
        self.submit(request);
    }

    /// A worker result arrived; `presented` is already current.
    pub(super) fn received_text(&mut self) {
        let active = self.presented.as_ref().and_then(|p| p.active_input.clone());
        let Some(inline) = self.inline.as_ref() else {
            return;
        };
        match active {
            Some(active) if active.input_id == inline.id => {
                let element = self.text_element(&active.layer_id);
                let inline = self.inline.as_mut().expect("inline");
                inline.started = true;
                inline.in_flight = false;
                if let Some(element) = &element {
                    inline.accepted = element.text.clone();
                }
                if active.layout.is_some() {
                    inline.layout = active.layout;
                }
                if element.is_some() {
                    inline.element = element;
                }
                self.set_selected(Some(active.layer_id));
                self.overlay_changed();
                self.drain_text();
            }
            Some(_) => {}
            // Finished (or another edit ended the input): the committed
            // layer is in the canvas pixels again.
            None if !inline.in_flight || inline.started => self.close_text(),
            None => {}
        }
    }

    /// A worker request failed: keep the typing and allow a retry.
    pub(super) fn text_failed(&mut self) {
        let Some(inline) = self.inline.as_mut() else {
            return;
        };
        if !inline.in_flight {
            return;
        }
        inline.in_flight = false;
        inline.finishing = false;
        if !inline.started {
            self.close_text();
        }
    }

    fn close_text(&mut self) {
        if self.inline.take().is_none() {
            return;
        }
        self.set_visible(self.caret, false);
        self.events.push(Event::Structure);
        self.overlay_changed();
    }

    /// Lay the inline text out where the session says it sits.
    fn view(&self) -> Option<View> {
        let inline = self.inline.as_ref()?;
        let (layout, element) = (inline.layout.as_ref()?, inline.element.as_ref()?);
        let r = self.image_rect()?;
        let (dw, _) = self.document_size()?;
        let s = (r.width / dw) as f32;
        let (fx, fy) = self.to_well(Point {
            x: layout.frame.x,
            y: layout.frame.y,
        })?;
        let frame = Rect::new(
            fx,
            fy,
            layout.frame.width as f32 * s,
            layout.frame.height as f32 * s,
        );
        let [top, right, _, left] = layout.padding.map(|v| v as f32 * s);
        let family = self
            .presented
            .as_ref()
            .and_then(|p| p.font_families.get(&element.font_family).cloned())
            .unwrap_or_else(|| "Segoe UI".into());
        let weight = if element.bold {
            DWRITE_FONT_WEIGHT_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        let style = if element.italic {
            DWRITE_FONT_STYLE_ITALIC
        } else {
            DWRITE_FONT_STYLE_NORMAL
        };
        let dwrite = self.g.dwrite();
        let name = HSTRING::from(family.as_str());
        let bundled =
            bundled_collection(dwrite).filter(|c| em_metrics(c, &name, weight, style).is_some());
        let size = element.font_size as f32 * s;
        unsafe {
            let (name, metrics) = match &bundled {
                Some(c) => (name.clone(), em_metrics(c, &name, weight, style)),
                None => {
                    let mut system = None;
                    dwrite.GetSystemFontCollection(&mut system, false).ok()?;
                    let fallback = HSTRING::from("Segoe UI");
                    let metrics = system
                        .as_ref()
                        .and_then(|c| em_metrics(c, &fallback, weight, style));
                    (fallback, metrics)
                }
            };
            let format = dwrite
                .CreateTextFormat(
                    &name,
                    bundled.as_ref(),
                    weight,
                    style,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size.max(1.),
                    w!("en-us"),
                )
                .ok()?;
            let text = inline.field.display();
            let inner = (frame.w - left - right).max(1.);
            let text_layout = dwrite.CreateTextLayout(&text, &format, inner, 1e5).ok()?;
            text_layout
                .SetWordWrapping(if layout.auto_width {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_WRAP
                })
                .ok()?;
            text_layout
                .SetTextAlignment(match element.align.as_str() {
                    "center" => DWRITE_TEXT_ALIGNMENT_CENTER,
                    "right" => DWRITE_TEXT_ALIGNMENT_TRAILING,
                    _ => DWRITE_TEXT_ALIGNMENT_LEADING,
                })
                .ok()?;
            // CSS line boxes: half the leading above, then the ascent.
            let (ascent, descent) = metrics.unwrap_or((0.9, 0.25));
            let line = layout.line_height as f32 * s;
            text_layout
                .SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    line.max(1.),
                    (line - (ascent + descent) * size) / 2. + ascent * size,
                )
                .ok()?;
            Some(View {
                layout: text_layout,
                origin: (frame.x + left, frame.y + top),
                frame,
                rotation: layout.rotation as f32,
                radius: layout.plate_radius as f32 * s,
                color: hex_color(&element.color),
                plate: element
                    .background
                    .as_deref()
                    .filter(|color| !color.is_empty())
                    .map(hex_color),
            })
        }
    }

    /// The inline field's corners, well-relative, for the overlay's box.
    pub(super) fn text_corners(&self) -> Vec<(f32, f32)> {
        self.view().map(|v| v.corners()).unwrap_or_default()
    }

    /// The inline field's frame in client DIPs (UI Automation).
    pub fn text_rect(&self) -> Rect {
        let Some(v) = self.view() else {
            return Rect::default();
        };
        let corners = v.corners();
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in corners {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Rect::new(self.l.well.x + x0, self.l.well.y + y0, x1 - x0, y1 - y0)
    }

    pub fn text_value(&self) -> String {
        self.inline
            .as_ref()
            .map(|i| String::from_utf16_lossy(i.field.units()))
            .unwrap_or_default()
    }

    /// Draw the plate, selection, text and composition (overlay).
    pub(super) fn draw_text(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let (Some(v), Some(inline)) = (self.view(), self.inline.as_ref()) else {
            return Ok(());
        };
        unsafe {
            let mut base = Matrix3x2::default();
            dc.GetTransform(&mut base);
            dc.SetTransform(&(v.matrix() * base));
            if let Some(plate) = v.plate {
                let f = v.frame;
                gfx::fill_round(dc, rect(f.x, f.y, f.x + f.w, f.y + f.h), v.radius, plate)?;
            }
            let f = &inline.field;
            let (a, b) = f.selection();
            if a != b && !f.composing() {
                let highlight = self.t.color_alpha("theme-accent", 0.38);
                for m in v.range(a, b - a) {
                    let (x, y) = (v.origin.0 + m.left, v.origin.1 + m.top);
                    gfx::fill(dc, rect(x, y, x + m.width.max(3.), y + m.height), highlight)?;
                }
            }
            dc.DrawTextLayout(
                Vector2 {
                    X: v.origin.0,
                    Y: v.origin.1,
                },
                &v.layout,
                &gfx::brush(dc, v.color)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            if let Some((start, len)) = f.composition() {
                for m in v.range(start, len) {
                    let (x, y) = (v.origin.0 + m.left, v.origin.1 + m.top + m.height);
                    gfx::fill(dc, rect(x, y - 1.5, x + m.width, y), v.color)?;
                }
            }
            dc.SetTransform(&base);
        }
        Ok(())
    }

    /// Put the caret visual at the caret; hidden over a selection.
    pub(super) fn place_caret(&mut self) {
        let caret = self.view().and_then(|v| {
            let f = &self.inline.as_ref()?.field;
            let (a, b) = f.selection();
            if a != b && !f.composing() {
                return None;
            }
            let (x, top, h) = v.position(f.display_caret());
            let (px, py) = v.rotate((v.origin.0 + x, v.origin.1 + top), 1.);
            Some((Rect::new(px, py, 1.5, h.max(4.)), v.color))
        });
        match caret {
            Some((r, color)) => {
                let moved = self.nodes[self.caret].rect != r || !self.nodes[self.caret].visible;
                self.set_rect(self.caret, r);
                self.set_visible(self.caret, true);
                if self.caret_color != Some(color_key(color)) {
                    self.caret_color = Some(color_key(color));
                    self.nodes[self.caret].dirty = true;
                }
                if moved {
                    self.caret_blink = true;
                }
            }
            None => self.set_visible(self.caret, false),
        }
        self.need = true;
    }

    pub(super) fn caret_fill(&self) -> D2D1_COLOR_F {
        self.inline
            .as_ref()
            .and_then(|i| i.element.as_ref())
            .map_or(self.t.color("text"), |e| hex_color(&e.color))
    }

    /// Whether a client point is inside the inline field.
    pub(super) fn in_text(&self, x: f32, y: f32) -> bool {
        let Some(v) = self.view() else {
            return false;
        };
        let (lx, ly) = v.rotate((x - self.l.well.x, y - self.l.well.y), -1.);
        let pad = 4.;
        let f = v.corners();
        let unrotated: Vec<(f32, f32)> = f.into_iter().map(|p| v.rotate(p, -1.)).collect();
        let (x0, y0) = unrotated[0];
        let (x1, y1) = unrotated[2];
        lx >= x0 - pad && lx <= x1 + pad && ly >= y0 - pad && ly <= y1 + pad
    }

    fn edit(&mut self, edit: impl FnOnce(&mut textfield::Editor) -> textfield::Change) {
        let Some(inline) = self.inline.as_mut() else {
            return;
        };
        let change = edit(&mut inline.field);
        if change.text {
            self.events.push(Event::InlineText);
            self.drain_text();
        }
        if change.any() {
            self.overlay_changed();
        }
    }

    /// A press in the field: place the caret, a word on double-click.
    pub(super) fn text_click(&mut self, x: f32, y: f32, double: bool, shift: bool) {
        let Some(v) = self.view() else {
            return;
        };
        if self.inline.as_ref().is_some_and(|i| i.field.composing()) {
            textfield::ime_complete(self.hwnd);
        }
        let display = v.hit((x - self.l.well.x, y - self.l.well.y));
        self.edit(|f| {
            let position = f.text_position(display);
            if double {
                let (a, b) = f.word_at(position);
                f.select(a, b)
            } else {
                f.move_to(position, shift)
            }
        });
        self.text_drag = !double;
    }

    pub(super) fn text_drag_to(&mut self, x: f32, y: f32) {
        let Some(v) = self.view() else {
            return;
        };
        let display = v.hit((x - self.l.well.x, y - self.l.well.y));
        self.edit(|f| {
            let position = f.text_position(display);
            f.move_to(position, true)
        });
    }

    /// Keys for the inline field; false lets the editor handle them.
    pub(super) fn text_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
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
            VK_ESCAPE => self.finish_text(),
            VK_TAB => {
                self.finish_text();
                return false;
            }
            VK_RETURN => self.edit(|f| f.insert("\n")),
            VK_LEFT => self.edit(|f| f.key(Key::Left, shift, ctrl)),
            VK_RIGHT => self.edit(|f| f.key(Key::Right, shift, ctrl)),
            VK_HOME if ctrl => self.edit(|f| f.key(Key::Home, shift, false)),
            VK_END if ctrl => self.edit(|f| f.key(Key::End, shift, false)),
            VK_HOME | VK_END => self.edit(|f| {
                let to = line_edge(f, vk == VK_END);
                f.move_to(to, shift)
            }),
            VK_UP | VK_DOWN => {
                let Some(v) = self.view() else {
                    return true;
                };
                let caret = self.inline.as_ref().map_or(0, |i| i.field.display_caret());
                let (x, top, h) = v.position(caret);
                let y = if vk == VK_UP {
                    top - h / 2.
                } else {
                    top + h * 1.5
                };
                let (px, py) = v.rotate((v.origin.0 + x, v.origin.1 + y), 1.);
                let display = v.hit((px, py));
                self.edit(|f| {
                    let position = f.text_position(display);
                    f.move_to(position, shift)
                });
            }
            VK_BACK => self.edit(|f| f.key(Key::Backspace, false, ctrl)),
            VK_DELETE if shift && !ctrl => self.cut_text(),
            VK_DELETE => self.edit(|f| f.key(Key::Delete, false, ctrl)),
            VK_A if ctrl => self.edit(|f| f.key(Key::SelectAll, false, false)),
            VK_Z if ctrl && !shift => self.edit(|f| f.key(Key::Undo, false, false)),
            VK_Z | VK_Y if ctrl => self.edit(|f| f.key(Key::Redo, false, false)),
            VK_C | VK_INSERT if ctrl => {
                if let Some(i) = &self.inline {
                    let text = i.field.selected_text();
                    if !text.is_empty() {
                        textfield::copy(self.hwnd, &text);
                    }
                }
            }
            VK_X if ctrl => self.cut_text(),
            VK_V if ctrl => self.paste_text(),
            VK_INSERT if shift => self.paste_text(),
            // Everything else types through `WM_CHAR` or does nothing, so
            // tool letters never switch tools mid-label.
            _ => {}
        }
        true
    }

    fn cut_text(&mut self) {
        let hwnd = self.hwnd;
        self.edit(|f| match f.cut() {
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

    fn paste_text(&mut self) {
        if let Some(text) = textfield::paste(self.hwnd) {
            self.edit(|f| f.insert(&text));
        }
    }

    /// `WM_CHAR`: surrogate pairs arrive in halves; control characters
    /// (Ctrl+letters, Backspace, Enter, Escape) do not type.
    pub(super) fn text_char(&mut self, unit: u16) {
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
        self.edit(|f| f.insert(&text));
    }

    pub(super) fn ime_composition(&mut self, hwnd: HWND, flags: u32) {
        self.edit(|f| textfield::ime_composition(hwnd, flags, f));
    }

    pub(super) fn ime_end(&mut self) {
        self.edit(|f| f.commit_composition(&[]));
    }

    /// UI Automation `SetValue` on the inline field.
    pub fn set_text_value(&mut self, value: &str) {
        self.edit(|f| f.set_text(value));
    }

    /// The caret's client pixels, for the IME windows.
    pub(super) fn ime_rect(&self) -> Option<(i32, i32, i32)> {
        self.inline.as_ref()?;
        let n = &self.nodes[self.caret];
        let s = self.scale;
        let px = |v: f32| (v * s).round() as i32;
        Some((
            px(self.l.well.x + n.rect.x),
            px(self.l.well.y + n.rect.y),
            px(n.rect.h),
        ))
    }

    // ---- grips ----

    /// The selected layer, when it shows resize grips: text, images and
    /// closed shapes, unlocked, with Select or Text and no label open.
    fn grip_layer(&self) -> Option<&Element> {
        if self.inline.is_some() || !matches!(self.tool, "v" | "t") {
            return None;
        }
        if matches!(self.gesture, Some(Gesture::Move { .. })) {
            return None;
        }
        let id = self.selected.as_ref()?;
        let element = self
            .presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find(|e| e.base().id == *id)?;
        let resizable = match element {
            Element::Text(_) | Element::Image(_) => true,
            Element::Shape(shape) => super::closed_kind(&shape.shape).is_some(),
            Element::Path(_) => false,
        };
        (resizable && !element.base().locked && element.base().visible).then_some(element)
    }

    /// The grip under a document point.
    pub(super) fn handle_at(&self, point: Point) -> Option<ResizeHandle> {
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        self.grip_layer()?
            .resize_handle_at(point, f64::from(GRIP + 3.) / scale)
            .ok()
            .flatten()
    }

    /// Grip centres in well DIPs: corners and edge midpoints.
    pub(super) fn grips(&self) -> Vec<(f32, f32)> {
        if self.grip_layer().is_none() {
            return Vec::new();
        }
        let Some(outline) = self.selection_outline() else {
            return Vec::new();
        };
        let corners: Vec<(f32, f32)> = outline.iter().filter_map(|p| self.to_well(*p)).collect();
        if corners.len() != 4 {
            return Vec::new();
        }
        let mut points = corners.clone();
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            points.push(((a.0 + b.0) / 2., (a.1 + b.1) / 2.));
        }
        points
    }

    pub(super) fn draw_grips(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let accent = self.t.color("theme-accent");
        let white = D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a: 1.,
        };
        for (x, y) in self.grips() {
            let outer = rect(
                x - GRIP - 1.5,
                y - GRIP - 1.5,
                x + GRIP + 1.5,
                y + GRIP + 1.5,
            );
            gfx::fill_round(dc, outer, GRIP + 1.5, white)?;
            gfx::fill_round(
                dc,
                rect(x - GRIP, y - GRIP, x + GRIP, y + GRIP),
                GRIP,
                accent,
            )?;
        }
        Ok(())
    }

    pub(super) fn resize_request(
        &self,
        id: &str,
        handle: ResizeHandle,
        current: Point,
        lock_aspect: bool,
    ) -> Request {
        Request::Layer {
            id: id.to_owned(),
            edit: LayerEdit::Resize {
                handle,
                current,
                display_scale: self.zoom_percent().unwrap_or(100.) / 100.,
                lock_aspect,
            },
        }
    }

    /// Render the drag so far on the worker, one request at a time.
    pub(super) fn preview_resize(&mut self) {
        if self.preview_busy {
            self.preview_dirty = true;
            return;
        }
        let Some(Gesture::Resize {
            id, handle, end, ..
        }) = self.gesture.clone()
        else {
            return;
        };
        let lock = self.shift_down();
        self.preview_busy = true;
        self.preview_dirty = false;
        let request = self.resize_request(&id, handle, end, lock);
        let _ = self.jobs.send(Job::Preview(request));
    }
}

/// A color as a comparable key.
fn color_key(c: D2D1_COLOR_F) -> [u32; 4] {
    [c.r, c.g, c.b, c.a].map(f32::to_bits)
}
