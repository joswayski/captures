//! Text editor adapted from GPUI 0.2.2's Apache-2.0 `examples/input.rs`.
use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Style,
    TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill,
    hsla, point, prelude::*, px, relative, rgba, size,
};

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Up,
        Down,
        Enter,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Vec<LaidOutLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_y: Pixels,
    is_selecting: bool,
    multiline: bool,
    max_len: Option<usize>,
    chrome: bool,
    disabled: bool,
    height: Option<Pixels>,
}

impl TextInput {
    pub fn new(
        value: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        let content = value.into();
        let end = content.len();
        Self {
            focus_handle: cx.focus_handle(),
            content,
            placeholder: placeholder.into(),
            selected_range: end..end,
            selection_reversed: false,
            marked_range: None,
            last_layout: Vec::new(),
            last_bounds: None,
            scroll_y: Pixels::ZERO,
            is_selecting: false,
            multiline: false,
            max_len: None,
            chrome: false,
            disabled: false,
            height: None,
        }
    }

    /// Inline field inside an already bordered editor toolbar or filename group.
    pub fn chrome(mut self) -> Self {
        self.chrome = true;
        self
    }

    /// Configures this editor for paragraph input without changing the public
    /// constructor/API used by the screenshot and recording editors.
    pub fn multiline(mut self, max_len: usize) -> Self {
        self.multiline = true;
        self.max_len = Some(max_len);
        self
    }

    pub fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = Some(max_len);
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            cx.notify();
        }
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn value(&self) -> String {
        self.content.to_string()
    }

    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = value.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_y = Pixels::ZERO;
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.move_vertically(-1, cx);
        } else {
            cx.propagate();
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.move_vertically(1, cx);
        } else {
            cx.propagate();
        }
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.propagate();
        }
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Editor surfaces focus their canvas on background clicks. A field
        // must own this click rather than immediately losing focus to it.
        cx.stop_propagation();
        if self.disabled {
            return;
        }
        self.is_selecting = true;
        window.focus(&self.focus_handle);

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = if self.multiline {
                text
            } else {
                text.replace('\n', " ")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let local = position - bounds.origin;
        let line = self
            .last_layout
            .iter()
            .find(|line| local.y < line.y + line.height)
            .or(self.last_layout.last());
        line.map(|line| {
            line.start
                + line
                    .layout
                    .closest_index_for_position(point(local.x, local.y - line.y), line.line_height)
                    .unwrap_or_else(|index| index)
        })
        .unwrap_or(0)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        utf16_to_byte(&self.content, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        byte_to_utf16(&self.content, offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn move_vertically(&mut self, delta: isize, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let Some((line_index, position)) = self.position_for_offset(cursor) else {
            return;
        };
        let current = &self.last_layout[line_index];
        let target_y = position.y + current.line_height * delta as f32;
        let line = self
            .last_layout
            .iter()
            .find(|line| target_y >= line.y && target_y < line.y + line.height)
            .unwrap_or(if delta < 0 {
                &self.last_layout[0]
            } else {
                self.last_layout.last().unwrap()
            });
        let local_y = (target_y - line.y).clamp(Pixels::ZERO, line.height - px(1.));
        let index = line
            .layout
            .closest_index_for_position(point(position.x, local_y), line.line_height)
            .unwrap_or_else(|index| index);
        self.move_to(line.start + index, cx);
    }

    fn position_for_offset(&self, offset: usize) -> Option<(usize, Point<Pixels>)> {
        let (index, line) =
            self.last_layout.iter().enumerate().find(|(_, line)| {
                offset >= line.start && offset <= line.start + line.layout.len()
            })?;
        line.layout
            .position_for_index(offset - line.start, line.line_height)
            .map(|position| (index, position + point(Pixels::ZERO, line.y)))
    }
}

fn utf16_to_byte(text: &str, offset: usize) -> usize {
    text.char_indices()
        .find_map(|(byte, ch)| {
            (text[..byte].encode_utf16().count() + ch.len_utf16() > offset).then_some(byte)
        })
        .unwrap_or(text.len())
}

fn byte_to_utf16(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .chars()
        .map(char::len_utf16)
        .sum()
}

fn truncate_to_byte_limit(text: &str, limit: usize) -> &str {
    let end = text
        .char_indices()
        .map(|(index, _)| index)
        .chain([text.len()])
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    &text[..end]
}

#[derive(Clone)]
struct LaidOutLine {
    layout: WrappedLine,
    start: usize,
    y: Pixels,
    height: Pixels,
    line_height: Pixels,
}

fn position_in_lines(lines: &[LaidOutLine], offset: usize) -> Option<Point<Pixels>> {
    let line = lines
        .iter()
        .find(|line| offset >= line.start && offset <= line.start + line.layout.len())?;
    line.layout
        .position_for_index(offset - line.start, line.line_height)
        .map(|position| position + point(Pixels::ZERO, line.y))
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let new_text = if self.multiline {
            new_text.to_owned()
        } else {
            new_text.replace('\n', " ")
        };
        let available = self
            .max_len
            .map(|limit| limit.saturating_sub(self.content.len() - (range.end - range.start)));
        let new_text = available
            .map(|available| truncate_to_byte_limit(&new_text, available))
            .unwrap_or(&new_text);
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let new_text = if self.multiline {
            new_text.to_owned()
        } else {
            new_text.replace('\n', " ")
        };
        let available = self
            .max_len
            .map(|limit| limit.saturating_sub(self.content.len() - (range.end - range.start)));
        let new_text = available
            .map(|available| truncate_to_byte_limit(&new_text, available))
            .unwrap_or(&new_text);
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|relative| {
                range.start + utf16_to_byte(new_text, relative.start)
                    ..range.start + utf16_to_byte(new_text, relative.end)
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (_, start) = self.position_for_offset(range.start)?;
        let (_, end) = self.position_for_offset(range.end)?;
        Some(Bounds::from_corners(
            bounds.origin + start,
            bounds.origin + end + point(px(1.), window.line_height()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let utf8_index = self.index_for_mouse_position(point);
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<LaidOutLine>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = if self.input.read(cx).multiline {
            px(92.).into()
        } else {
            window.line_height().into()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0., 0.2))
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let shaped = window
            .text_system()
            .shape_text(
                display_text,
                font_size,
                &runs,
                input.multiline.then_some(bounds.size.width),
                None,
            )
            .unwrap_or_default();
        let line_height = window.line_height();
        let mut y = -input.scroll_y;
        let mut start = 0;
        let mut lines: Vec<_> = shaped
            .into_iter()
            .map(|layout| {
                let height = layout.size(line_height).height;
                let line = LaidOutLine {
                    start,
                    y,
                    height,
                    line_height,
                    layout,
                };
                start += line.layout.text.len() + 1;
                y += height;
                line
            })
            .collect();

        let cursor_position = position_in_lines(&lines, cursor);
        let shift = cursor_position.map_or(Pixels::ZERO, |p| {
            if p.y < Pixels::ZERO {
                p.y
            } else if p.y + line_height > bounds.size.height {
                p.y + line_height - bounds.size.height
            } else {
                Pixels::ZERO
            }
        });
        let scroll_y = (input.scroll_y + shift).max(Pixels::ZERO);
        for line in &mut lines {
            line.y -= shift;
        }
        self.input.update(cx, |input, _| input.scroll_y = scroll_y);
        let cursor_position = cursor_position.map(|p| p - point(Pixels::ZERO, shift));
        let (selection, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                cursor_position.map(|position| {
                    fill(
                        Bounds::new(bounds.origin + position, size(px(2.), line_height)),
                        gpui::blue(),
                    )
                }),
            )
        } else {
            let mut quads = Vec::new();
            for line in &lines {
                let local_start = selected_range
                    .start
                    .saturating_sub(line.start)
                    .min(line.layout.len());
                let local_end = selected_range
                    .end
                    .saturating_sub(line.start)
                    .min(line.layout.len());
                if local_start >= local_end {
                    continue;
                }
                let first_row = line
                    .layout
                    .position_for_index(local_start, line_height)
                    .unwrap_or_default()
                    .y;
                let last_row = line
                    .layout
                    .position_for_index(local_end, line_height)
                    .unwrap_or_default()
                    .y;
                let mut row = first_row;
                while row <= last_row {
                    let row_start = if row == first_row {
                        line.layout
                            .position_for_index(local_start, line_height)
                            .unwrap_or_default()
                            .x
                    } else {
                        Pixels::ZERO
                    };
                    let row_end = if row == last_row {
                        line.layout
                            .position_for_index(local_end, line_height)
                            .unwrap_or_default()
                            .x
                    } else {
                        bounds.size.width
                    };
                    quads.push(fill(
                        Bounds::new(
                            bounds.origin + point(row_start, line.y + row),
                            size((row_end - row_start).max(px(1.)), line_height),
                        ),
                        rgba(0x3311ff30),
                    ));
                    row += line_height;
                }
            }
            (quads, None)
        };
        PrepaintState {
            lines,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection)
        }
        for line in &prepaint.lines {
            line.layout
                .paint(
                    bounds.origin + point(Pixels::ZERO, line.y),
                    line.line_height,
                    TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                )
                .unwrap();
        }

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = std::mem::take(&mut prepaint.lines);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .overflow_hidden()
            .when(!self.chrome, |field| {
                field
                    .border_1()
                    .border_color(rgba(0x88888840))
                    .rounded(px(6.))
            })
            .line_height(px(20.))
            .text_size(px(if self.chrome { 12. } else { 13. }))
            .child(
                div()
                    .h(self.height.unwrap_or(px(if self.multiline {
                        100.
                    } else if self.chrome {
                        26.
                    } else {
                        30.
                    })))
                    .w_full()
                    .px(px(4.))
                    .py(px(if self.chrome { 3. } else { 4. }))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("up", Up, Some("TextInput")),
        KeyBinding::new("down", Down, Some("TextInput")),
        KeyBinding::new("enter", Enter, Some("TextInput")),
    ]);
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-a"), SelectAll, Some("TextInput")),
        KeyBinding::new(&format!("{modifier}-v"), Paste, Some("TextInput")),
        KeyBinding::new(&format!("{modifier}-c"), Copy, Some("TextInput")),
        KeyBinding::new(&format!("{modifier}-x"), Cut, Some("TextInput")),
    ]);
}

#[cfg(test)]
mod tests {
    use super::{byte_to_utf16, truncate_to_byte_limit, utf16_to_byte};

    #[test]
    fn converts_between_utf8_and_utf16_boundaries() {
        let text = "a😀é";
        assert_eq!(utf16_to_byte(text, 0), 0);
        assert_eq!(utf16_to_byte(text, 1), 1);
        // A UTF-16 offset inside a surrogate pair snaps to its character start.
        assert_eq!(utf16_to_byte(text, 2), 1);
        assert_eq!(utf16_to_byte(text, 3), 5);
        assert_eq!(byte_to_utf16(text, 5), 3);
        assert_eq!(byte_to_utf16(text, text.len()), 4);
    }

    #[test]
    fn truncation_never_splits_utf8() {
        assert_eq!(truncate_to_byte_limit("a😀b", 4), "a");
        assert_eq!(truncate_to_byte_limit("a😀b", 5), "a😀");
        assert_eq!(truncate_to_byte_limit("a😀b", 6), "a😀b");
    }
}
