//! Layers rows beyond select, hide and lock: shipping's inline rename
//! (`.screenshot-layer-copy input`, image layers, on double-click) and
//! reordering, by dragging a row (`draggable` `li`s with a drop line) or,
//! in this host, by Alt+Up and Alt+Down on a focused row.
//!
//! The rename field is the shared single-line `TextField` drawn inside the
//! row's surface; its caret is a separate visual over the list that blinks
//! on the compositor. A drag fades the dragged row on the compositor and
//! moves a small drop-line visual; neither redraws a row.
use captures_app::editor::{Element, LayerEdit, LayerPlacement};
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::core::Result;
use windows_numerics::Matrix3x2;

use super::{Editor, Event, QUICK, ROW_H, Request, Target};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;
use crate::textfield::{self, Key, TextField};

/// `.screenshot-layer-copy input { height: 25px }`.
const RENAME_H: f32 = 25.;
/// `.screenshot-layer-copy { gap: 2px }`.
const COPY_GAP: f32 = 2.;
/// Windows' drag threshold (`SM_CXDRAG`, 4 px at 96 DPI), which Chromium
/// uses to start an HTML drag.
const DRAG_START: f32 = 4.;
/// `li.dragging { opacity: 0.42 }`.
pub(super) const DRAGGING_OPACITY: f32 = 0.42;
/// The drop line: 2 px tall, 3 px in from each side, 4 px outside the row,
/// with `box-shadow: 0 0 8px`.
pub const DROP_LINE_H: f32 = 2.;
pub const DROP_LINE_GLOW: f32 = 8.;

/// An open rename: the layer and its field.
pub struct Rename {
    pub id: String,
    pub field: TextField,
}

/// A press on a row that may become a drag.
pub struct RowDrag {
    pub from: usize,
    pub id: String,
    start: (f32, f32),
    pub active: bool,
    /// The row and side the line marks (shipping `layerDropTarget`, which
    /// stays while the pointer is over the dragged row or a gap).
    pub target: Option<(usize, LayerPlacement)>,
}

/// The drop line's rect for a row rect (list-relative).
pub fn drop_line_rect(row: Rect, placement: LayerPlacement) -> Rect {
    let y = match placement {
        LayerPlacement::Before => row.y - 4.,
        LayerPlacement::After => row.y + row.h + 4. - DROP_LINE_H,
    };
    Rect::new(row.x + 3., y, (row.w - 6.).max(0.), DROP_LINE_H)
}

/// A drop target as `(row, before)`, comparable.
fn side(target: Option<(usize, LayerPlacement)>) -> Option<(usize, bool)> {
    target.map(|(i, p)| (i, matches!(p, LayerPlacement::Before)))
}

/// Shipping `onDragOver`: the half of the row the pointer is in.
pub fn placement_at(row: Rect, y: f32) -> LayerPlacement {
    if y < row.y + row.h / 2. {
        LayerPlacement::Before
    } else {
        LayerPlacement::After
    }
}

/// Keyboard reorder: the row to place against and on which side, moving
/// one row toward the front (`up`, the list's top) or the back.
pub fn step_target(index: usize, count: usize, up: bool) -> Option<(usize, LayerPlacement)> {
    if up {
        index.checked_sub(1).map(|i| (i, LayerPlacement::Before))
    } else {
        (index + 1 < count).then_some((index + 1, LayerPlacement::After))
    }
}

impl Editor {
    /// The row being renamed.
    pub fn renaming(&self) -> Option<usize> {
        let id = &self.rename.as_ref()?.id;
        self.presented
            .as_ref()?
            .layers
            .iter()
            .position(|l| l.id == *id)
    }

    pub fn rename_text(&self) -> String {
        self.rename
            .as_ref()
            .map(|r| String::from_utf16_lossy(r.field.editor.units()))
            .unwrap_or_default()
    }

    /// The image layer's own name (shipping `element.name`).
    fn image_name(&self, i: usize) -> Option<String> {
        let id = &self.layer(i)?.id;
        self.presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find_map(|e| match e {
                Element::Image(image) if image.base.id == *id => Some(image.name.clone()),
                _ => None,
            })
    }

    /// The input inside a row, row-relative: the copy column's first line.
    fn rename_input(&self, w: f32, h: f32) -> Rect {
        let t = &self.t;
        let (s3, s4) = (t.n("s-3"), t.n("s-4"));
        let x = 1. + s3 + 12. + s4 + 44. + s4;
        let room = (w - x - s3 - 3. * QUICK.0 - 4. - s3).max(1.);
        let kind = self
            .g
            .text(t, Style::Small, "Ag", 1e4)
            .map_or(16., |k| k.height);
        let y = ((h - (RENAME_H + COPY_GAP + kind)) / 2.).round();
        Rect::new(x, y, room, RENAME_H)
    }

    /// `padding: 1px var(--s-3)` inside the 1 px border.
    fn rename_text_box(&self, input: Rect) -> Rect {
        let s3 = self.t.n("s-3");
        Rect::new(
            input.x + 1. + s3,
            input.y + 1.,
            (input.w - 2. - 2. * s3).max(1.),
            input.h - 2.,
        )
    }

    /// The rename input in client DIPs.
    pub fn rename_rect(&self) -> Option<Rect> {
        let i = self.renaming()?;
        let row = self.target_rect(Target::Layer(i));
        let input = self.rename_input(row.w, row.h);
        Some(Rect::new(
            row.x + input.x,
            row.y + input.y,
            input.w,
            input.h,
        ))
    }

    pub fn can_rename(&self, i: usize) -> bool {
        self.image_name(i).is_some()
    }

    /// Shipping `beginLayerRename`: select the layer, switch to Select and
    /// open the field with its name selected.
    pub(super) fn begin_rename(&mut self, i: usize) {
        let (Some(name), Some(id)) = (self.image_name(i), self.layer(i).map(|l| l.id.clone()))
        else {
            return;
        };
        if self.inline.is_some() {
            self.finish_text();
        }
        self.set_focus(None, false);
        self.set_crop(None);
        self.select_tool("v");
        self.set_selected(Some(id.clone()));
        let mut field = TextField::new(Style::Button, "");
        field.editor.set_text(&name);
        field.editor.select_all();
        let input = self.rename_input(self.row_part(Target::Layer(i)).w, ROW_H);
        let width = self.rename_text_box(input).w;
        if let Err(error) = field.relayout(&self.g, &self.t, width) {
            eprintln!("rename layout failed: {error:?}");
        }
        self.rename = Some(Rename { id, field });
        self.row_dirty(i);
        self.place_rename_caret();
        self.events.push(Event::Structure);
        self.events.push(Event::RenameFocus);
    }

    /// Shipping `finishLayerRename` (Enter or blur): a trimmed, nonempty,
    /// changed name renames the layer; anything else just closes.
    pub(super) fn finish_rename(&mut self, commit: bool) {
        let index = self.renaming();
        let Some(rename) = self.rename.take() else {
            return;
        };
        if rename.field.editor.composing() && !self.hwnd.is_invalid() {
            textfield::ime_complete(self.hwnd);
        }
        self.set_visible(self.rename_caret, false);
        if let Some(i) = index {
            self.row_dirty(i);
            let name = String::from_utf16_lossy(rename.field.editor.units());
            let name = name.trim();
            if commit && !name.is_empty() && self.image_name(i).as_deref() != Some(name) {
                self.submit(Request::Layer {
                    id: rename.id,
                    edit: LayerEdit::Rename { name: name.into() },
                });
            }
        }
        self.events.push(Event::Structure);
        self.need = true;
    }

    /// Fit the field to its row again (a sidebar resize or re-theme).
    pub(super) fn relayout_rename(&mut self) {
        let Some(i) = self.renaming() else {
            return;
        };
        let input = self.rename_input(self.row_part(Target::Layer(i)).w, ROW_H);
        let width = self.rename_text_box(input).w;
        if let Some(rename) = self.rename.as_mut()
            && let Err(error) = rename.field.relayout(&self.g, &self.t, width)
        {
            eprintln!("rename layout failed: {error:?}");
        }
        self.row_dirty(i);
    }

    /// A reply without the renamed layer (an undo) closes the field.
    pub(super) fn sync_rename(&mut self) {
        if self.rename.is_some() && self.renaming().is_none() {
            self.rename = None;
            self.set_visible(self.rename_caret, false);
            self.events.push(Event::Structure);
        }
    }

    fn row_dirty(&mut self, i: usize) {
        if i < self.rows {
            let base = self.layered(Target::Layer(i)).base;
            self.nodes[base].dirty = true;
            self.need = true;
        }
    }

    /// The caret visual over the list, hidden over a selection.
    pub(super) fn place_rename_caret(&mut self) {
        let Some((i, rename)) = self.renaming().zip(self.rename.as_ref()) else {
            self.set_visible(self.rename_caret, false);
            return;
        };
        let (a, b) = rename.field.editor.selection();
        let row = self.row_part(Target::Layer(i));
        let input = self.rename_input(row.w, row.h);
        let text_box = self.rename_text_box(input);
        let (x, y, h) = rename.field.caret_box(0., text_box.h);
        let collapsed = a == b || rename.field.editor.composing();
        let r = Rect::new(
            row.x + text_box.x + x,
            row.y + text_box.y + y,
            1.,
            h.max(4.),
        );
        let node = self.rename_caret;
        let moved = self.nodes[node].rect != r || !self.nodes[node].visible;
        self.set_rect(node, r);
        self.set_visible(node, collapsed);
        if moved && collapsed {
            self.caret_blink = true;
            self.need = true;
        }
    }

    /// Apply an edit to the rename field.
    fn rename_edit(&mut self, edit: impl FnOnce(&mut textfield::Editor) -> textfield::Change) {
        let Some(i) = self.renaming() else {
            return;
        };
        let Some(rename) = self.rename.as_mut() else {
            return;
        };
        let change = edit(&mut rename.field.editor);
        match rename.field.apply(&self.g, &self.t, change) {
            Ok(true) => self.row_dirty(i),
            Ok(false) => {}
            Err(error) => eprintln!("rename layout failed: {error:?}"),
        }
        if change.text {
            self.events.push(Event::RenameText);
        }
        if change.any() {
            self.place_rename_caret();
        }
    }

    /// Keys while renaming; the input stops every key from reaching the
    /// editor (`stopPropagation`). True when consumed.
    pub(super) fn rename_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
        let Some(i) = self.renaming() else {
            return false;
        };
        let edit = |key: Key, shift: bool, ctrl: bool| {
            move |f: &mut textfield::Editor| f.key(key, shift, ctrl)
        };
        match vk {
            VK_RETURN => self.finish_rename(true),
            VK_ESCAPE => self.finish_rename(false),
            // The browser moves focus out of the input, which commits.
            VK_TAB => {
                self.finish_rename(true);
                let list = self.focusable();
                let next = list
                    .iter()
                    .position(|&t| t == Target::Layer(i))
                    .and_then(|p| if shift { p.checked_sub(1) } else { Some(p + 1) })
                    .and_then(|p| list.get(p).copied());
                self.set_focus(next, true);
            }
            VK_LEFT => self.rename_edit(edit(Key::Left, shift, ctrl)),
            VK_RIGHT => self.rename_edit(edit(Key::Right, shift, ctrl)),
            VK_HOME | VK_UP => self.rename_edit(edit(Key::Home, shift, false)),
            VK_END | VK_DOWN => self.rename_edit(edit(Key::End, shift, false)),
            VK_BACK => self.rename_edit(edit(Key::Backspace, false, ctrl)),
            VK_DELETE if shift && !ctrl => self.rename_cut(),
            VK_DELETE => self.rename_edit(edit(Key::Delete, false, ctrl)),
            VK_A if ctrl => self.rename_edit(edit(Key::SelectAll, false, false)),
            VK_Z if ctrl && !shift => self.rename_edit(edit(Key::Undo, false, false)),
            VK_Z | VK_Y if ctrl => self.rename_edit(edit(Key::Redo, false, false)),
            VK_C | VK_INSERT if ctrl => {
                let text = self
                    .rename
                    .as_ref()
                    .map(|r| r.field.editor.selected_text())
                    .unwrap_or_default();
                if !text.is_empty() {
                    textfield::copy(self.hwnd, &text);
                }
            }
            VK_X if ctrl => self.rename_cut(),
            VK_V if ctrl => self.rename_paste(),
            VK_INSERT if shift => self.rename_paste(),
            // Characters arrive as `WM_CHAR`; nothing else acts.
            _ => {}
        }
        true
    }

    fn rename_cut(&mut self) {
        let hwnd = self.hwnd;
        self.rename_edit(|f| match f.cut() {
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

    fn rename_paste(&mut self) {
        if let Some(text) = textfield::paste(self.hwnd) {
            self.rename_edit(|f| f.insert(&text));
        }
    }

    pub(super) fn rename_char(&mut self, unit: u16) {
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
        if !text.chars().any(char::is_control) {
            self.rename_edit(|f| f.insert(&text));
        }
    }

    pub(super) fn rename_ime(&mut self, flags: u32) {
        let hwnd = self.hwnd;
        self.rename_edit(|f| textfield::ime_composition(hwnd, flags, f));
    }

    pub(super) fn rename_ime_end(&mut self) {
        self.rename_edit(|f| f.commit_composition(&[]));
    }

    /// UI Automation `SetValue` on the open field.
    pub fn set_rename_value(&mut self, value: &str) {
        self.rename_edit(|f| f.set_text(value));
    }

    /// A press (or a drag with `extend`) inside the rename input.
    pub(super) fn rename_point(&mut self, x: f32, double: bool, extend: bool) {
        let Some(input) = self.rename_rect() else {
            return;
        };
        if self
            .rename
            .as_ref()
            .is_some_and(|r| r.field.editor.composing())
        {
            textfield::ime_complete(self.hwnd);
        }
        let s3 = self.t.n("s-3");
        let local = x - input.x - 1. - s3;
        let Some(position) = self.rename.as_ref().map(|r| r.field.hit(local)) else {
            return;
        };
        self.rename_edit(|f| {
            if double {
                let (a, b) = f.word_at(position);
                f.select(a, b)
            } else {
                f.move_to(position, extend)
            }
        });
        self.text_drag = !double;
    }

    /// The rename caret's IME anchor in client pixels.
    pub(super) fn rename_ime_rect(&self) -> Option<(i32, i32, i32)> {
        self.rename.as_ref()?;
        let r = self.absolute(self.rename_caret);
        let s = self.scale;
        let px = |v: f32| (v * s).round() as i32;
        Some((px(r.x), px(r.y), px(r.h)))
    }

    /// The open field in its row's surface (row-relative).
    pub(super) fn draw_rename(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let Some(rename) = self.rename.as_ref() else {
            return Ok(());
        };
        let t = &self.t;
        let input = self.rename_input(w, h);
        let r = rect(input.x, input.y, input.x + input.w, input.y + input.h);
        let radius = t.n("r-sm");
        // `--editor-field` is `--surface-field`; `outline: none`.
        gfx::fill_round(dc, r, radius, t.color("surface-field"))?;
        gfx::border(dc, r, radius, 1., t.color("theme-accent"))?;
        let text_box = self.rename_text_box(input);
        unsafe {
            let mut m = Matrix3x2::default();
            dc.GetTransform(&mut m);
            dc.SetTransform(&(Matrix3x2::translation(0., text_box.y) * m));
            let drawn = rename
                .field
                .draw(dc, t, (text_box.x, text_box.w, text_box.h), true);
            dc.SetTransform(&m);
            drawn
        }
    }

    /// The kind line's top under an open field (row-relative).
    pub(super) fn rename_kind_y(&self, w: f32, h: f32) -> f32 {
        let input = self.rename_input(w, h);
        input.y + input.h + COPY_GAP
    }

    // ---- reorder ----

    /// A press on an unlocked row arms a drag.
    pub(super) fn press_row(&mut self, i: usize, x: f32, y: f32) {
        self.row_drag = self
            .layer(i)
            .filter(|l| !l.locked && self.rename.is_none())
            .map(|l| RowDrag {
                from: i,
                id: l.id.clone(),
                start: (x, y),
                active: false,
                target: None,
            });
    }

    /// The row under a client point, if the point is on one.
    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.l.list.contains(x, y) {
            return None;
        }
        (0..self.layer_count()).find(|&i| self.target_rect(Target::Layer(i)).contains(x, y))
    }

    /// Pointer moves with a row pressed: start the drag past the threshold,
    /// then follow it with the drop line.
    pub(super) fn drag_row_to(&mut self, x: f32, y: f32) {
        let Some(drag) = self.row_drag.as_mut() else {
            return;
        };
        if !drag.active {
            let (dx, dy) = (x - drag.start.0, y - drag.start.1);
            if dx.abs() < DRAG_START && dy.abs() < DRAG_START {
                return;
            }
            drag.active = true;
            // A drag is not a click.
            self.pressed = None;
            self.refresh(false);
            self.need = true;
        }
        let from = self.row_drag.as_ref().map_or(0, |d| d.from);
        let next = match self.row_at(x, y) {
            Some(i) if i != from => {
                let row = self.target_rect(Target::Layer(i));
                Some((i, placement_at(row, y)))
            }
            // Over the dragged row or a gap: the last target stays.
            _ => self.row_drag.as_ref().and_then(|d| d.target),
        };
        if let Some(drag) = self.row_drag.as_mut()
            && side(drag.target) != side(next)
        {
            drag.target = next;
            self.place_drop_line();
        }
    }

    /// Show the drop line at the drag's target.
    pub(super) fn place_drop_line(&mut self) {
        let target = self
            .row_drag
            .as_ref()
            .filter(|d| d.active)
            .and_then(|d| d.target)
            .filter(|(i, _)| *i < self.layer_count());
        let node = self.drop_line;
        match target {
            Some((i, placement)) => {
                let r = drop_line_rect(self.row_part(Target::Layer(i)), placement);
                if self.nodes[node].rect != r || !self.nodes[node].visible {
                    self.need = true;
                }
                self.set_rect(node, r);
                self.set_visible(node, true);
            }
            None => self.set_visible(node, false),
        }
    }

    /// Release: a drag over another row reorders (shipping `dropLayer`);
    /// true when a drag ended, so the press is not a click.
    pub(super) fn drop_row(&mut self, x: f32, y: f32) -> bool {
        let Some(drag) = self.row_drag.take() else {
            return false;
        };
        if !drag.active {
            return false;
        }
        self.end_row_drag();
        if let Some(i) = self.row_at(x, y).filter(|&i| i != drag.from)
            && let Some(target) = self.layer(i).map(|l| l.id.clone())
        {
            let placement = match drag.target {
                Some((t, placement)) if t == i => placement,
                _ => LayerPlacement::Before,
            };
            self.reorder(drag.id, target, placement);
        }
        true
    }

    /// Escape (or a lost capture) ends a drag without a drop.
    pub(super) fn cancel_row_drag(&mut self) -> bool {
        match self.row_drag.take() {
            Some(drag) if drag.active => {
                self.end_row_drag();
                true
            }
            _ => false,
        }
    }

    fn end_row_drag(&mut self) {
        self.refresh(false);
        self.set_visible(self.drop_line, false);
        self.need = true;
    }

    /// `reorderScreenshotLayers` through the session, then select the
    /// moved layer with the Select tool.
    fn reorder(&mut self, id: String, target_id: String, placement: LayerPlacement) {
        self.submit(Request::Layer {
            id: id.clone(),
            edit: LayerEdit::Reorder {
                target_id,
                placement,
            },
        });
        self.set_crop(None);
        self.select_tool("v");
        self.set_selected(Some(id));
    }

    /// Alt+Up or Alt+Down on a focused row moves its layer one place; focus
    /// follows the layer. Shipping has only drag and the layer menu's
    /// Bring to front and Send to back.
    pub(super) fn reorder_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let Some(Target::Layer(i)) = self.focus else {
            return false;
        };
        if !matches!(vk, VK_UP | VK_DOWN) {
            return false;
        }
        let Some(layer) = self.layer(i) else {
            return false;
        };
        let (id, locked) = (layer.id.clone(), layer.locked);
        let target = step_target(i, self.layer_count(), vk == VK_UP)
            .and_then(|(t, placement)| self.layer(t).map(|l| (l.id.clone(), placement)));
        if let (false, Some((target_id, placement))) = (locked, target) {
            self.row_follow = Some(id.clone());
            self.reorder(id, target_id, placement);
        }
        true
    }

    /// After a reply: keep focus on the row of a layer moved by keyboard.
    pub(super) fn follow_row(&mut self) {
        let Some(id) = self.row_follow.take() else {
            return;
        };
        let Some(index) = self
            .presented
            .as_ref()
            .and_then(|p| p.layers.iter().position(|l| l.id == id))
        else {
            return;
        };
        if matches!(self.focus, Some(Target::Layer(_))) {
            let visible = self.focus_visible;
            // `set_focus` skips an unchanged target; the row moved under it.
            self.focus = None;
            self.set_focus(Some(Target::Layer(index)), visible);
        }
    }

    /// The drop line: an accent pill with its glow, in its own surface.
    pub(super) fn draw_drop_line(&self, dc: &ID2D1DeviceContext, w: f32) -> Result<()> {
        let accent = self.t.color("theme-accent");
        let r = rect(0., 0., w, DROP_LINE_H);
        let glow = windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F { a: 0.5, ..accent };
        gfx::glow(dc, r, DROP_LINE_H / 2., DROP_LINE_GLOW, glow)?;
        gfx::fill_round(dc, r, DROP_LINE_H / 2., accent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_line_sits_four_pixels_outside_the_row() {
        let row = Rect::new(2., 62., 200., 56.);
        let before = drop_line_rect(row, LayerPlacement::Before);
        assert_eq!(
            (before.x, before.y, before.w, before.h),
            (5., 58., 194., 2.)
        );
        let after = drop_line_rect(row, LayerPlacement::After);
        assert_eq!(after.y, 62. + 56. + 2.);
    }

    #[test]
    fn the_pointer_half_picks_the_side() {
        let row = Rect::new(0., 100., 10., 56.);
        assert!(matches!(placement_at(row, 127.9), LayerPlacement::Before));
        assert!(matches!(placement_at(row, 128.), LayerPlacement::After));
    }

    #[test]
    fn keyboard_steps_stop_at_the_ends() {
        assert!(step_target(0, 3, true).is_none());
        assert!(matches!(
            step_target(1, 3, true),
            Some((0, LayerPlacement::Before))
        ));
        assert!(matches!(
            step_target(1, 3, false),
            Some((2, LayerPlacement::After))
        ));
        assert!(step_target(2, 3, false).is_none());
    }
}
