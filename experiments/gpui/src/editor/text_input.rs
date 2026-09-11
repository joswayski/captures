//! Small GPUI-native text field used by the screenshot editor.
//! Based on GPUI 0.2.2's `examples/input.rs` EntityInputHandler example.

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Render,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, actions, div, fill, point,
    prelude::*, px, relative, size,
};
use std::ops::Range;

actions!(
    editor_text_input,
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
        Paste,
        Cut,
        Copy
    ]
);

pub struct TextInput {
    focus: FocusHandle,
    content: SharedString,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    layout: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: "".into(),
            selection: 0..0,
            reversed: false,
            marked: None,
            layout: None,
            bounds: None,
            selecting: false,
        }
    }

    pub fn set(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = value.into();
        self.selection = self.content.len()..self.content.len();
        self.marked = None;
        cx.notify();
    }

    pub fn value(&self) -> &str {
        &self.content
    }

    pub fn focus(&self, window: &mut Window) {
        self.focus.focus(window);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    fn previous(&self, offset: usize) -> usize {
        self.content[..self.boundary(offset)]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, offset: usize) -> usize {
        let offset = self.boundary(offset);
        self.content[offset..]
            .char_indices()
            .nth(1)
            .map_or(self.content.len(), |(i, _)| offset + i)
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selection = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selection.start = offset
        } else {
            self.selection.end = offset
        }
        if self.selection.end < self.selection.start {
            self.reversed = !self.reversed;
            self.selection = self.selection.end..self.selection.start;
        }
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selection.is_empty() {
            self.previous(self.cursor())
        } else {
            self.selection.start
        };
        self.move_to(at, cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selection.is_empty() {
            self.next(self.cursor())
        } else {
            self.selection.end
        };
        self.move_to(at, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous(self.cursor()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next(self.cursor()), cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = 0..self.content.len();
        cx.notify();
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }
    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            self.select_to(self.previous(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            self.select_to(self.next(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace(['\n', '\r'], " "), window, cx);
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selection.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        if !self.selection.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
        }
    }
    fn index_at(&self, position: gpui::Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.bounds, self.layout.as_ref()) else {
            return 0;
        };
        if position.x <= bounds.left() {
            0
        } else if position.x >= bounds.right() {
            self.content.len()
        } else {
            line.closest_index_for_x(position.x - bounds.left())
        }
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        cx.stop_propagation();
        self.selecting = true;
        let index = self.index_at(event.position);
        if event.modifiers.shift {
            self.select_to(index, cx)
        } else {
            self.move_to(index, cx)
        }
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.index_at(event.position), cx);
        }
    }
    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }
    fn to_utf16(&self, offset: usize) -> usize {
        self.content[..self.boundary(offset)].encode_utf16().count()
    }
    fn boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
    fn range_from_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
    fn range_to_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.to_utf16(range.start)..self.to_utf16(range.end)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(range);
        actual.replace(self.range_to_utf16(range.clone()));
        Some(self.content[range].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(self.selection.clone()),
            reversed: self.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|r| self.range_to_utf16(r))
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let range = self.boundary(range.start)..self.boundary(range.end);
        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            text,
            &self.content[range.end..]
        )
        .into();
        self.selection = range.start + text.len()..range.start + text.len();
        self.marked = None;
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = self.boundary(
            range
                .as_ref()
                .map(|r| self.offset_from_utf16(r.start))
                .or_else(|| self.marked.as_ref().map(|r| r.start))
                .unwrap_or(self.selection.start),
        );
        self.replace_text_in_range(range, text, window, cx);
        let end = (start + text.len()).min(self.content.len());
        self.marked = (!text.is_empty()).then_some(start..end);
        if let Some(selected) = selected {
            let utf16_to_inserted = |offset: usize| {
                text.chars()
                    .scan(0, |count, ch| {
                        let before = *count;
                        *count += ch.len_utf16();
                        Some((before, ch.len_utf8()))
                    })
                    .take_while(|(before, _)| *before < offset)
                    .map(|(_, bytes)| bytes)
                    .sum::<usize>()
                    .min(text.len())
            };
            self.selection =
                start + utf16_to_inserted(selected.start)..start + utf16_to_inserted(selected.end);
        }
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.layout.as_ref()?;
        let range = self.range_from_utf16(range);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.to_utf16(self.index_at(point)))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}
struct Prepaint {
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}
impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let input = self.input.read(cx);
        let display: SharedString = if input.content.is_empty() {
            "Type text…".into()
        } else {
            input.content.clone()
        };
        let style = window.text_style();
        let run = TextRun {
            len: display.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            display,
            style.font_size.to_pixels(window.rem_size()),
            &[run],
            None,
        );
        let cursor_x = line.x_for_index(input.cursor());
        let (selection, cursor) = if input.selection.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, bounds.top()),
                        size(px(2.), bounds.size.height),
                    ),
                    gpui::blue(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(input.selection.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(input.selection.end),
                            bounds.bottom(),
                        ),
                    ),
                    gpui::rgba(0x3388ff66),
                )),
                None,
            )
        };
        Prepaint {
            line,
            cursor,
            selection,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = state.selection.take() {
            window.paint_quad(selection);
        }
        state
            .line
            .paint(bounds.origin, window.line_height(), window, cx)
            .ok();
        if focus.is_focused(window)
            && let Some(cursor) = state.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.layout = Some(state.line.clone());
            input.bounds = Some(bounds);
        });
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("EditorTextInput")
            .track_focus(&self.focus)
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
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .h(px(36.))
            .w_full()
            .px(px(8.))
            .flex()
            .items_center()
            .child(TextElement { input: cx.entity() })
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("backspace", Backspace, Some("EditorTextInput")),
        gpui::KeyBinding::new("delete", Delete, Some("EditorTextInput")),
        gpui::KeyBinding::new("left", Left, Some("EditorTextInput")),
        gpui::KeyBinding::new("right", Right, Some("EditorTextInput")),
        gpui::KeyBinding::new("shift-left", SelectLeft, Some("EditorTextInput")),
        gpui::KeyBinding::new("shift-right", SelectRight, Some("EditorTextInput")),
        gpui::KeyBinding::new("ctrl-a", SelectAll, Some("EditorTextInput")),
        gpui::KeyBinding::new("home", Home, Some("EditorTextInput")),
        gpui::KeyBinding::new("end", End, Some("EditorTextInput")),
        gpui::KeyBinding::new("ctrl-v", Paste, Some("EditorTextInput")),
        gpui::KeyBinding::new("ctrl-x", Cut, Some("EditorTextInput")),
        gpui::KeyBinding::new("ctrl-c", Copy, Some("EditorTextInput")),
    ]);
}
