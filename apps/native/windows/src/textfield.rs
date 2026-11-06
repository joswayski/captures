//! A token-drawn single-line text field: the editing model ([`Editor`]), its
//! DirectWrite view ([`TextField`]), clipboard and IMM32 helpers.
//!
//! The model is plain UTF-16 with a caret, an anchor and an inline IME
//! composition, and follows Chromium's `<input>` on Windows: Ctrl+arrows
//! stop at word starts, double-click selects a word with its trailing
//! spaces, pasted line breaks are dropped. The model builds on every OS so
//! its tests run on Linux; the view, clipboard and IME parts are Windows-only.
//! Hosts own focus and drawing order: Preferences Find uses it now, the
//! editor and Capture History later.
#![cfg_attr(not(windows), allow(dead_code))]

/// A word character for Ctrl+arrows and double-click (letters, digits, `_`
/// and surrogate halves, so emoji and CJK extension planes count as words).
fn is_word(unit: u16) -> bool {
    (0xD800..0xE000).contains(&unit)
        || char::from_u32(u32::from(unit)).is_some_and(|c| c.is_alphanumeric() || c == '_')
}

fn is_space(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(char::is_whitespace)
}

/// Editing keys the model understands; the host maps virtual keys to these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    SelectAll,
    Undo,
    Redo,
}

/// What a key or edit did, so the host redraws and notifies only as needed.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Change {
    /// The text changed.
    pub text: bool,
    /// The caret or selection moved.
    pub caret: bool,
}

impl Change {
    pub fn any(self) -> bool {
        self.text || self.caret
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Snapshot {
    text: Vec<u16>,
    caret: usize,
    anchor: usize,
}

#[derive(Default, Debug)]
pub struct Editor {
    text: Vec<u16>,
    caret: usize,
    anchor: usize,
    /// The IME composition, shown at the caret until it commits.
    comp: Vec<u16>,
    comp_cursor: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The last edit was typing, so the next typed text joins its undo step.
    typing: bool,
    /// HTML `maxLength`, in UTF-16 units: typed and pasted text is cut to fit.
    pub max_len: Option<usize>,
}

impl Editor {
    pub fn units(&self) -> &[u16] {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// `(start, end)` of the selection, in UTF-16 units.
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection();
        String::from_utf16_lossy(&self.text[a..b])
    }

    pub fn composing(&self) -> bool {
        !self.comp.is_empty()
    }

    /// The text as drawn: the composition sits at the caret.
    pub fn display(&self) -> Vec<u16> {
        if self.comp.is_empty() {
            return self.text.clone();
        }
        let mut display = self.text[..self.caret].to_vec();
        display.extend_from_slice(&self.comp);
        display.extend_from_slice(&self.text[self.caret..]);
        display
    }

    /// The composition's `(start, length)` in the display text.
    pub fn composition(&self) -> Option<(usize, usize)> {
        (!self.comp.is_empty()).then_some((self.caret, self.comp.len()))
    }

    /// The caret in the display text (inside a composition, its cursor).
    pub fn display_caret(&self) -> usize {
        self.caret
            + if self.comp.is_empty() {
                0
            } else {
                self.comp_cursor
            }
    }

    /// A display position back to a text position (composition collapses).
    pub fn text_position(&self, position: usize) -> usize {
        let position = if !self.comp.is_empty() && position > self.caret {
            position.saturating_sub(self.comp.len()).max(self.caret)
        } else {
            position
        };
        position.min(self.text.len())
    }

    /// Replace everything (UIA `SetValue`, a restored query); caret at the end.
    pub fn set_text(&mut self, text: &str) -> Change {
        let units = single_line(text);
        if units == self.text {
            return Change::default();
        }
        self.checkpoint(false);
        self.text = units;
        if let Some(max) = self.max_len {
            self.text.truncate(max);
        }
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.comp.clear();
        Change {
            text: true,
            caret: true,
        }
    }

    pub fn select_all(&mut self) -> Change {
        self.select(0, self.text.len())
    }

    /// Select `anchor..caret` (either order); positions snap off surrogate pairs.
    pub fn select(&mut self, anchor: usize, caret: usize) -> Change {
        let (anchor, caret) = (self.snap(anchor), self.snap(caret));
        if (anchor, caret) == (self.anchor, self.caret) {
            return Change::default();
        }
        self.anchor = anchor;
        self.caret = caret;
        self.typing = false;
        Change {
            text: false,
            caret: true,
        }
    }

    /// Put the caret at `position`, extending the selection when `extend`.
    pub fn move_to(&mut self, position: usize, extend: bool) -> Change {
        let anchor = if extend { self.anchor } else { position };
        self.select(anchor, position)
    }

    /// Typed or pasted text replaces the selection; line breaks are dropped.
    pub fn insert(&mut self, text: &str) -> Change {
        let units = single_line(text);
        self.insert_units(&units, true)
    }

    fn insert_units(&mut self, units: &[u16], typing: bool) -> Change {
        let (a, b) = self.selection();
        let room = self.max_len.map_or(units.len(), |max| {
            max.saturating_sub(self.text.len() - (b - a))
        });
        let mut units = &units[..units.len().min(room)];
        // Never keep half a surrogate pair.
        if units.last().is_some_and(|&u| is_high(u)) {
            units = &units[..units.len() - 1];
        }
        if units.is_empty() && a == b {
            return Change::default();
        }
        self.checkpoint(typing && a == b);
        self.text.splice(a..b, units.iter().copied());
        self.caret = a + units.len();
        self.anchor = self.caret;
        self.typing = typing;
        Change {
            text: true,
            caret: true,
        }
    }

    /// Cut: the selection's text, removed.
    pub fn cut(&mut self) -> Option<String> {
        let (a, b) = self.selection();
        if a == b {
            return None;
        }
        let text = self.selected_text();
        self.insert_units(&[], false);
        Some(text)
    }

    pub fn key(&mut self, key: Key, shift: bool, ctrl: bool) -> Change {
        let (a, b) = self.selection();
        match key {
            Key::Left | Key::Right => {
                let forward = key == Key::Right;
                let to = if ctrl {
                    if forward {
                        self.word_right(self.caret)
                    } else {
                        self.word_left(self.caret)
                    }
                } else if !shift && a != b {
                    // A plain arrow collapses the selection to that side.
                    if forward { b } else { a }
                } else if forward {
                    self.next(self.caret)
                } else {
                    self.prev(self.caret)
                };
                self.move_to(to, shift)
            }
            Key::Home => self.move_to(0, shift),
            Key::End => self.move_to(self.text.len(), shift),
            Key::Backspace | Key::Delete => {
                if a == b {
                    let forward = key == Key::Delete;
                    let to = match (forward, ctrl) {
                        (true, true) => self.word_right(self.caret),
                        (true, false) => self.next(self.caret),
                        (false, true) => self.word_left(self.caret),
                        (false, false) => self.prev(self.caret),
                    };
                    if to == self.caret {
                        return Change::default();
                    }
                    self.anchor = to;
                }
                self.insert_units(&[], false)
            }
            Key::SelectAll => self.select_all(),
            Key::Undo => self.restore(true),
            Key::Redo => self.restore(false),
        }
    }

    /// The word (with Windows' trailing spaces) or the run of other
    /// characters at `position`, for double-click.
    pub fn word_at(&self, position: usize) -> (usize, usize) {
        let n = self.text.len();
        if n == 0 {
            return (0, 0);
        }
        let at = position.min(n - 1);
        let kind = |unit: u16| (is_word(unit), is_space(unit));
        let class = kind(self.text[at]);
        let (mut start, mut end) = (at, at + 1);
        while start > 0 && kind(self.text[start - 1]) == class {
            start -= 1;
        }
        while end < n && kind(self.text[end]) == class {
            end += 1;
        }
        if class.0 {
            while end < n && is_space(self.text[end]) {
                end += 1;
            }
        }
        (start, end)
    }

    /// IME: show `text` at the caret with its cursor; the selection is
    /// replaced when a composition starts, like Chromium.
    pub fn set_composition(&mut self, text: &[u16], cursor: usize) -> Change {
        if self.comp.is_empty() && text.is_empty() {
            return Change::default();
        }
        if self.comp.is_empty() && self.caret != self.anchor {
            self.insert_units(&[], false);
        }
        self.comp = text.to_vec();
        self.comp_cursor = cursor.min(text.len());
        Change {
            text: true,
            caret: true,
        }
    }

    /// IME: the composition ends; `result` is inserted (it may be empty).
    pub fn commit_composition(&mut self, result: &[u16]) -> Change {
        let had = !self.comp.is_empty();
        self.comp.clear();
        let mut change = self.insert_units(&single_line_units(result), true);
        change.text |= had;
        change.caret |= had;
        change
    }

    pub fn prev(&self, i: usize) -> usize {
        if i == 0 {
            return 0;
        }
        let j = i - 1;
        if j > 0 && is_low(self.text[j]) && is_high(self.text[j - 1]) {
            j - 1
        } else {
            j
        }
    }

    pub fn next(&self, i: usize) -> usize {
        let n = self.text.len();
        if i >= n {
            return n;
        }
        if is_high(self.text[i]) && i + 1 < n && is_low(self.text[i + 1]) {
            i + 2
        } else {
            i + 1
        }
    }

    /// Ctrl+Left: the start of this or the previous word.
    pub fn word_left(&self, mut i: usize) -> usize {
        i = i.min(self.text.len());
        while i > 0 && !is_word(self.text[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(self.text[i - 1]) {
            i -= 1;
        }
        i
    }

    /// Ctrl+Right: the start of the next word.
    pub fn word_right(&self, mut i: usize) -> usize {
        let n = self.text.len();
        while i < n && is_word(self.text[i]) {
            i += 1;
        }
        while i < n && !is_word(self.text[i]) {
            i += 1;
        }
        i
    }

    fn snap(&self, position: usize) -> usize {
        let position = position.min(self.text.len());
        if position > 0
            && position < self.text.len()
            && is_low(self.text[position])
            && is_high(self.text[position - 1])
        {
            position - 1
        } else {
            position
        }
    }

    /// Record an undo step, unless this edit continues the current typing.
    fn checkpoint(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push(Snapshot {
                text: self.text.clone(),
                caret: self.caret,
                anchor: self.anchor,
            });
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    fn restore(&mut self, undo: bool) -> Change {
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(snapshot) = from.pop() else {
            return Change::default();
        };
        to.push(Snapshot {
            text: std::mem::take(&mut self.text),
            caret: self.caret,
            anchor: self.anchor,
        });
        self.text = snapshot.text;
        self.caret = snapshot.caret;
        self.anchor = snapshot.anchor;
        self.typing = false;
        Change {
            text: true,
            caret: true,
        }
    }
}

fn is_high(unit: u16) -> bool {
    (0xD800..0xDC00).contains(&unit)
}

fn is_low(unit: u16) -> bool {
    (0xDC00..0xE000).contains(&unit)
}

/// Chromium drops line breaks from text put into a single-line `<input>`.
fn single_line(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .filter(|&u| u != u16::from(b'\r') && u != u16::from(b'\n'))
        .collect()
}

fn single_line_units(units: &[u16]) -> Vec<u16> {
    units
        .iter()
        .copied()
        .filter(|&u| u != u16::from(b'\r') && u != u16::from(b'\n'))
        .collect()
}

#[cfg(windows)]
pub use view::*;

#[cfg(windows)]
mod view {
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
    use windows::Win32::Graphics::Direct2D::{D2D1_ANTIALIAS_MODE_ALIASED, ID2D1DeviceContext};
    use windows::Win32::Graphics::DirectWrite::*;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    use windows::Win32::UI::Input::Ime::*;
    use windows::core::Result;

    use super::{Change, Editor};
    use crate::gfx::{self, Gfx, Style, Text, rect};
    use crate::tokens::Tokens;

    const CF_UNICODETEXT: u32 = 13;

    /// The editor laid out in one DirectWrite line, scrolled horizontally
    /// so the caret stays in view. Coordinates are DIPs from the left edge
    /// of the text box (the field's content box).
    pub struct TextField {
        pub editor: Editor,
        pub style: Style,
        placeholder: String,
        display: Option<Text>,
        placeholder_text: Option<Text>,
        /// Visible width of the text box.
        width: f32,
        /// How far the text is scrolled left.
        scroll: f32,
        line_height: f32,
        /// The selection the text was last drawn with.
        drawn_selection: (usize, usize),
        /// CSS `text-transform: uppercase` (ASCII, so positions still match).
        pub uppercase: bool,
    }

    impl TextField {
        pub fn new(style: Style, placeholder: &str) -> Self {
            Self {
                editor: Editor::default(),
                style,
                placeholder: placeholder.into(),
                display: None,
                placeholder_text: None,
                width: 0.,
                scroll: 0.,
                line_height: 0.,
                drawn_selection: (0, 0),
                uppercase: false,
            }
        }

        /// Lay the display text out again after an edit, a re-theme or a
        /// resize of the text box to `width`.
        pub fn relayout(&mut self, g: &Gfx, t: &Tokens, width: f32) -> Result<()> {
            self.width = width.max(1.);
            let mut units = self.editor.display();
            if self.uppercase {
                for unit in &mut units {
                    if (u16::from(b'a')..=u16::from(b'z')).contains(unit) {
                        *unit -= 32;
                    }
                }
            }
            let display = String::from_utf16_lossy(&units);
            let text = g.text(t, self.style, &display, 1e5)?;
            if let Some((start, length)) = self.editor.composition() {
                unsafe {
                    text.layout.SetUnderline(
                        true,
                        DWRITE_TEXT_RANGE {
                            startPosition: start as u32,
                            length: length as u32,
                        },
                    )?;
                }
            }
            if self.placeholder_text.is_none() {
                self.placeholder_text = Some(g.text(t, self.style, &self.placeholder, 1e5)?);
            }
            self.line_height = text.height;
            self.display = Some(text);
            self.follow_caret();
            Ok(())
        }

        pub fn line_height(&self) -> f32 {
            self.line_height
        }

        fn text_width(&self) -> f32 {
            self.display.as_ref().map_or(0., |t| t.width)
        }

        /// The x of a display position, before scrolling.
        fn x_of(&self, position: usize) -> f32 {
            let Some(text) = &self.display else {
                return 0.;
            };
            let (mut x, mut y) = (0., 0.);
            let mut metrics = DWRITE_HIT_TEST_METRICS::default();
            unsafe {
                let _ = text.layout.HitTestTextPosition(
                    position as u32,
                    false,
                    &mut x,
                    &mut y,
                    &mut metrics,
                );
            }
            x
        }

        /// The caret's x in the text box, after scrolling.
        pub fn caret_x(&self) -> f32 {
            self.x_of(self.editor.display_caret()) - self.scroll
        }

        /// Scroll so the caret is inside the box (Chromium keeps 1 px for it).
        fn follow_caret(&mut self) {
            let x = self.x_of(self.editor.display_caret());
            let max = (self.text_width() + 1. - self.width).max(0.);
            if x - self.scroll > self.width - 1. {
                self.scroll = x - self.width + 1.;
            } else if x < self.scroll {
                self.scroll = x;
            }
            self.scroll = self.scroll.clamp(0., max);
        }

        /// Call after a caret-only change; returns true when the text must
        /// be redrawn because it scrolled.
        pub fn caret_moved(&mut self) -> bool {
            let before = self.scroll;
            self.follow_caret();
            before != self.scroll
        }

        /// The text position under `x` in the text box.
        pub fn hit(&self, x: f32) -> usize {
            let Some(text) = &self.display else {
                return 0;
            };
            let (mut trailing, mut inside) = (windows_core::BOOL(0), windows_core::BOOL(0));
            let mut metrics = DWRITE_HIT_TEST_METRICS::default();
            unsafe {
                let _ = text.layout.HitTestPoint(
                    x + self.scroll,
                    self.line_height / 2.,
                    &mut trailing,
                    &mut inside,
                    &mut metrics,
                );
            }
            let position = metrics.textPosition as usize
                + if trailing.as_bool() {
                    metrics.length as usize
                } else {
                    0
                };
            self.editor.text_position(position)
        }

        /// Rects `(left, width)` covering the text range `a..b`, scrolled.
        pub fn range_rects(&self, a: usize, b: usize) -> Vec<(f32, f32)> {
            let Some(text) = &self.display else {
                return Vec::new();
            };
            let mut buffer = [DWRITE_HIT_TEST_METRICS::default(); 16];
            let mut count = 0;
            let ok = unsafe {
                text.layout.HitTestTextRange(
                    a as u32,
                    (b - a) as u32,
                    0.,
                    0.,
                    Some(&mut buffer),
                    &mut count,
                )
            };
            if ok.is_err() {
                return Vec::new();
            }
            buffer[..(count as usize).min(16)]
                .iter()
                .map(|m| (m.left - self.scroll, m.width))
                .collect()
        }

        /// Draw text, selection (when focused) or the placeholder into a
        /// `w`×`h` box whose text line is centred vertically.
        pub fn draw(
            &self,
            dc: &ID2D1DeviceContext,
            t: &Tokens,
            (x, w, h): (f32, f32, f32),
            focused: bool,
        ) -> Result<()> {
            let y = ((h - self.line_height) / 2.).max(0.);
            unsafe { dc.PushAxisAlignedClip(&rect(x, 0., x + w, h), D2D1_ANTIALIAS_MODE_ALIASED) };
            let drawn = self.draw_clipped(dc, t, (x, y), focused);
            unsafe { dc.PopAxisAlignedClip() };
            drawn
        }

        fn draw_clipped(
            &self,
            dc: &ID2D1DeviceContext,
            t: &Tokens,
            (x, y): (f32, f32),
            focused: bool,
        ) -> Result<()> {
            let c = |name: &str| t.color(name);
            if self.editor.len() == 0 && !self.editor.composing() {
                if let Some(placeholder) = &self.placeholder_text {
                    gfx::text(dc, placeholder, x, y, c("text-faint"))?;
                }
                return Ok(());
            }
            let Some(text) = &self.display else {
                return Ok(());
            };
            let origin = x - self.scroll;
            gfx::text(dc, text, origin, y, c("text"))?;
            let (a, b) = self.editor.selection();
            if focused && a != b && !self.editor.composing() {
                // `::selection`: accent fill, accent ink over it.
                for (left, width) in self.range_rects(a, b) {
                    let r = rect(x + left, y, x + left + width, y + self.line_height);
                    selected(
                        dc,
                        text,
                        r,
                        (origin, y),
                        c("theme-accent"),
                        c("theme-accent-ink"),
                    )?;
                }
            }
            Ok(())
        }

        /// The caret box `(x, y, h)` in the field for a `h`-tall box.
        pub fn caret_box(&self, x: f32, h: f32) -> (f32, f32, f32) {
            let y = ((h - self.line_height) / 2.).max(0.);
            (x + self.caret_x(), y, self.line_height)
        }

        /// Apply an edit result: relayout on text changes, follow the caret.
        /// Returns true when the text node must redraw.
        pub fn apply(&mut self, g: &Gfx, t: &Tokens, change: Change) -> Result<bool> {
            let selection = self.editor.selection();
            let before = std::mem::replace(&mut self.drawn_selection, selection);
            if change.text {
                self.relayout(g, t, self.width)?;
                return Ok(true);
            }
            if !change.caret {
                return Ok(false);
            }
            // A collapsed caret moves on the compositor; only a scroll or a
            // drawn selection repaints the text.
            let scrolled = self.caret_moved();
            Ok(scrolled
                || (before != selection && (before.0 != before.1 || selection.0 != selection.1)))
        }
    }

    fn selected(
        dc: &ID2D1DeviceContext,
        text: &Text,
        r: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
        (x, y): (f32, f32),
        fill: D2D1_COLOR_F,
        ink: D2D1_COLOR_F,
    ) -> Result<()> {
        gfx::fill(dc, r, fill)?;
        unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) };
        let drawn = gfx::text(dc, text, x, y, ink);
        unsafe { dc.PopAxisAlignedClip() };
        drawn
    }

    /// Put `text` on the clipboard as `CF_UNICODETEXT`.
    pub fn copy(hwnd: HWND, text: &str) {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        unsafe {
            if OpenClipboard(Some(hwnd)).is_err() {
                return;
            }
            let _ = EmptyClipboard();
            if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) {
                let target = GlobalLock(memory) as *mut u16;
                if !target.is_null() {
                    std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
                    let _ = GlobalUnlock(memory);
                    if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0))).is_err() {
                        let _ = GlobalFree(Some(memory));
                    }
                }
            }
            let _ = CloseClipboard();
        }
    }

    /// The clipboard's text, if any.
    pub fn paste(hwnd: HWND) -> Option<String> {
        unsafe {
            OpenClipboard(Some(hwnd)).ok()?;
            let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|handle| {
                let memory = HGLOBAL(handle.0);
                let source = GlobalLock(memory) as *const u16;
                if source.is_null() {
                    return None;
                }
                let mut length = 0;
                while *source.add(length) != 0 {
                    length += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(source, length));
                let _ = GlobalUnlock(memory);
                Some(text)
            });
            let _ = CloseClipboard();
            text
        }
    }

    /// Place the IME composition and candidate windows at the caret
    /// (`caret` is `(x, y, h)` in client pixels); the candidates avoid it.
    pub fn ime_position(hwnd: HWND, (x, y, h): (i32, i32, i32)) {
        unsafe {
            let context = ImmGetContext(hwnd);
            if context.is_invalid() {
                return;
            }
            let composition = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: POINT { x, y },
                rcArea: RECT::default(),
            };
            let _ = ImmSetCompositionWindow(context, &composition);
            let candidate = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: POINT { x, y },
                rcArea: RECT {
                    left: x,
                    top: y,
                    right: x + 1,
                    bottom: y + h,
                },
            };
            let _ = ImmSetCandidateWindow(context, &candidate);
            let _ = ImmReleaseContext(hwnd, context);
        }
    }

    /// One `GCS_*` string of the current composition.
    fn ime_string(context: HIMC, kind: IME_COMPOSITION_STRING) -> Vec<u16> {
        unsafe {
            let bytes = ImmGetCompositionStringW(context, kind, None, 0);
            if bytes <= 0 {
                return Vec::new();
            }
            let mut units = vec![0u16; bytes as usize / 2];
            ImmGetCompositionStringW(context, kind, Some(units.as_mut_ptr().cast()), bytes as u32);
            units
        }
    }

    /// `WM_IME_COMPOSITION`: commit a result string and show the new
    /// composition string. Returns what changed.
    pub fn ime_composition(hwnd: HWND, flags: u32, editor: &mut Editor) -> Change {
        let mut change = Change::default();
        unsafe {
            let context = ImmGetContext(hwnd);
            if context.is_invalid() {
                return change;
            }
            if flags & GCS_RESULTSTR.0 != 0 {
                let result = ime_string(context, GCS_RESULTSTR);
                let c = editor.commit_composition(&result);
                change.text |= c.text;
                change.caret |= c.caret;
            }
            if flags & GCS_COMPSTR.0 != 0 {
                let text = ime_string(context, GCS_COMPSTR);
                let cursor = if flags & GCS_CURSORPOS.0 != 0 {
                    ImmGetCompositionStringW(context, GCS_CURSORPOS, None, 0).max(0) as usize
                } else {
                    text.len()
                };
                let c = editor.set_composition(&text, cursor);
                change.text |= c.text;
                change.caret |= c.caret;
            }
            let _ = ImmReleaseContext(hwnd, context);
        }
        change
    }

    /// Finish an open composition (a click moved the caret).
    pub fn ime_complete(hwnd: HWND) {
        unsafe {
            let context = ImmGetContext(hwnd);
            if context.is_invalid() {
                return;
            }
            let _ = ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
            let _ = ImmReleaseContext(hwnd, context);
        }
    }

    /// The IME is enabled only while a text field has focus, so keys reach
    /// other controls (switches, recorders) undisturbed.
    pub fn ime_enable(hwnd: HWND, enabled: bool) {
        unsafe {
            let flags = if enabled { IACE_DEFAULT } else { 0 };
            let _ = ImmAssociateContextEx(hwnd, HIMC::default(), flags);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Editor {
        fn text(&self) -> String {
            String::from_utf16_lossy(self.units())
        }
    }

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.set_text(text);
        editor
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut e = editor("show cursor");
        e.select(5, 11);
        e.insert("clicks");
        assert_eq!(e.text(), "show clicks");
        assert_eq!(e.selection(), (11, 11));
    }

    #[test]
    fn ctrl_arrows_stop_at_word_starts_like_windows() {
        let e = editor("freeze  screen, now");
        assert_eq!(e.word_right(0), 8);
        assert_eq!(e.word_right(8), 16);
        assert_eq!(e.word_left(19), 16);
        assert_eq!(e.word_left(16), 8);
        assert_eq!(e.word_left(3), 0);
    }

    #[test]
    fn shift_extends_and_plain_arrows_collapse() {
        let mut e = editor("abc def");
        e.key(Key::Home, false, false);
        e.key(Key::Right, true, true);
        assert_eq!(e.selected_text(), "abc ");
        e.key(Key::Right, false, false);
        assert_eq!(e.selection(), (4, 4));
        e.key(Key::End, true, false);
        assert_eq!(e.selected_text(), "def");
        e.key(Key::Left, false, false);
        assert_eq!(e.selection(), (4, 4));
    }

    #[test]
    fn deletion_by_character_word_and_selection() {
        let mut e = editor("one two three");
        e.key(Key::Backspace, false, true);
        assert_eq!(e.text(), "one two ");
        e.key(Key::Backspace, false, false);
        assert_eq!(e.text(), "one two");
        e.key(Key::Home, false, false);
        e.key(Key::Delete, false, true);
        assert_eq!(e.text(), "two");
        e.select_all();
        e.key(Key::Delete, false, false);
        assert_eq!(e.text(), "");
        assert_eq!(e.key(Key::Backspace, false, false), Change::default());
    }

    #[test]
    fn surrogate_pairs_move_and_delete_as_one() {
        let mut e = editor("a😀b");
        assert_eq!(e.len(), 4);
        e.key(Key::Left, false, false);
        e.key(Key::Left, false, false);
        assert_eq!(e.caret(), 1);
        e.key(Key::Delete, false, false);
        assert_eq!(e.text(), "ab");
        // A position inside a pair snaps to its start.
        let mut e = editor("😀");
        e.move_to(1, false);
        assert_eq!(e.caret(), 0);
    }

    #[test]
    fn double_click_takes_the_word_and_its_trailing_spaces() {
        let e = editor("show  cursor in");
        assert_eq!(e.word_at(1), (0, 6));
        assert_eq!(e.word_at(4), (4, 6));
        assert_eq!(e.word_at(8), (6, 13));
        assert_eq!(e.word_at(99), (13, 15));
    }

    #[test]
    fn pasted_line_breaks_are_dropped() {
        let mut e = editor("");
        e.insert("free\r\nze");
        assert_eq!(e.text(), "freeze");
    }

    #[test]
    fn composition_shows_at_the_caret_and_commits() {
        let mut e = editor("ab");
        e.select(0, 1);
        let wide: Vec<u16> = "にほ".encode_utf16().collect();
        e.set_composition(&wide, 1);
        // Starting a composition replaces the selection.
        assert_eq!(e.text(), "b");
        assert_eq!(String::from_utf16_lossy(&e.display()), "にほb");
        assert_eq!(e.composition(), Some((0, 2)));
        assert_eq!(e.display_caret(), 1);
        assert_eq!(e.text_position(2), 0);
        assert_eq!(e.text_position(3), 1);
        let result: Vec<u16> = "日本".encode_utf16().collect();
        e.commit_composition(&result);
        assert_eq!(e.text(), "日本b");
        assert!(!e.composing());
        assert_eq!(e.caret(), 2);
    }

    #[test]
    fn undo_joins_typing_and_redo_restores() {
        let mut e = editor("");
        for ch in ["c", "u", "r"] {
            e.insert(ch);
        }
        e.key(Key::Backspace, false, false);
        assert_eq!(e.text(), "cu");
        e.key(Key::Undo, false, false);
        assert_eq!(e.text(), "cur");
        e.key(Key::Undo, false, false);
        assert_eq!(e.text(), "");
        e.key(Key::Redo, false, false);
        assert_eq!(e.text(), "cur");
    }

    #[test]
    fn max_len_cuts_typed_and_pasted_text_like_html() {
        let mut e = editor("");
        e.max_len = Some(7);
        e.insert("#32d3ff00");
        assert_eq!(e.text(), "#32d3ff");
        e.insert("a");
        assert_eq!(e.text(), "#32d3ff");
        // Replacing a selection frees its room.
        e.select(1, 3);
        e.insert("ABC");
        assert_eq!(e.text(), "#ABd3ff");
        e.select_all();
        e.insert("#12345😀");
        assert_eq!(e.text(), "#12345");
    }

    #[test]
    fn cut_returns_and_removes_the_selection() {
        let mut e = editor("freeze screen");
        assert_eq!(e.cut(), None);
        e.select(0, 7);
        assert_eq!(e.cut().as_deref(), Some("freeze "));
        assert_eq!(e.text(), "screen");
    }
}
