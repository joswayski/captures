//! Single-line GPUI text input adapted from GPUI 0.2.2's published input example.
//! It uses `EntityInputHandler`, so composition/IME, selection, and UTF-16
//! platform ranges work rather than treating key presses as ASCII text.
use crate::ui::{metric, theme};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions,
    fill, point, prelude::*, px, relative, size,
};
use std::{ops::Range, sync::Once};

actions!(
    captures_preferences_input,
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
        Copy,
        Submit
    ]
);

static BINDINGS: Once = Once::new();

pub fn bind_keys(cx: &mut App) {
    BINDINGS.call_once(|| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("CapturesInput")),
            KeyBinding::new("delete", Delete, Some("CapturesInput")),
            KeyBinding::new("left", Left, Some("CapturesInput")),
            KeyBinding::new("right", Right, Some("CapturesInput")),
            KeyBinding::new("shift-left", SelectLeft, Some("CapturesInput")),
            KeyBinding::new("shift-right", SelectRight, Some("CapturesInput")),
            KeyBinding::new("ctrl-a", SelectAll, Some("CapturesInput")),
            KeyBinding::new("ctrl-v", Paste, Some("CapturesInput")),
            KeyBinding::new("ctrl-c", Copy, Some("CapturesInput")),
            KeyBinding::new("ctrl-x", Cut, Some("CapturesInput")),
            KeyBinding::new("home", Home, Some("CapturesInput")),
            KeyBinding::new("end", End, Some("CapturesInput")),
            KeyBinding::new("enter", Submit, Some("CapturesInput")),
        ]);
    });
}

#[derive(Clone)]
pub enum InputEvent {
    Changed(String),
    Submitted(String),
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

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
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    pub fn content(&self) -> String {
        self.content.to_string()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content = "".into();
        self.selected_range = 0..0;
        self.marked_range = None;
        cx.emit(InputEvent::Changed(String::new()));
        cx.notify();
    }

    pub fn handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }
    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submitted(self.content()));
    }
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace(['\n', '\r'], " "), window, cx);
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
        self.copy(&Copy, window, cx);
        if !self.selected_range.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
        }
    }
    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(index, cx)
        } else {
            self.move_to(index, cx)
        }
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }
    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }
    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }
    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }
    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }
    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .char_indices()
            .nth(1)
            .map_or(self.content.len(), |(index, _)| offset + index)
    }
    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (&self.last_bounds, &self.last_layout) else {
            return 0;
        };
        if position.y < bounds.top() {
            0
        } else if position.y > bounds.bottom() {
            self.content.len()
        } else {
            line.closest_index_for_x(position.x - bounds.left())
                .min(self.content.len())
        }
    }
    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for character in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += character.len_utf16();
            utf8 += character.len_utf8();
        }
        utf8
    }
    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset].encode_utf16().count()
    }
    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }
    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
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
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        let end = range.start + new_text.len();
        self.selected_range = end..end;
        self.marked_range = None;
        cx.emit(InputEvent::Changed(self.content()));
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selected_range = selected
            .as_ref()
            .map(|selection| self.range_from_utf16(selection))
            .map(|selection| range.start + selection.start..range.start + selection.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.emit(InputEvent::Changed(self.content()));
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        line.index_for_x(point.x - bounds.left())
            .map(|index| self.offset_to_utf16(index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}
struct PrepaintState {
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
    type PrepaintState = PrepaintState;
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
    ) -> PrepaintState {
        let input = self.input.read(cx);
        let content = if input.content.is_empty() {
            input.placeholder.clone()
        } else {
            input.content.clone()
        };
        let color = if input.content.is_empty() {
            theme(cx).muted()
        } else {
            window.text_style().color
        };
        let run = TextRun {
            len: content.len(),
            font: window.text_style().font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = &input.marked_range {
            vec![
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: content.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };
        let line = window.text_system().shape_line(
            content,
            window.text_style().font_size.to_pixels(window.rem_size()),
            &runs,
            None,
        );
        let selection = (!input.selected_range.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(
                        bounds.left() + line.x_for_index(input.selected_range.start),
                        bounds.top(),
                    ),
                    point(
                        bounds.left() + line.x_for_index(input.selected_range.end),
                        bounds.bottom(),
                    ),
                ),
                theme(cx).accent.opacity(0.28),
            )
        });
        let cursor = input.selected_range.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(
                        bounds.left() + line.x_for_index(input.cursor_offset()),
                        bounds.top(),
                    ),
                    size(px(1.5), bounds.size.height),
                ),
                theme(cx).accent,
            )
        });
        PrepaintState {
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
        state: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
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
            .expect("paint input text");
        if focus.is_focused(window)
            && let Some(cursor) = state.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = Some(state.line.clone());
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme(cx);
        gpui::div()
            .key_context("CapturesInput")
            .track_focus(&self.focus_handle)
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
            .on_action(cx.listener(Self::submit))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .h(metric("--h-md"))
            .px(metric("--s-4"))
            .flex()
            .items_center()
            .rounded(metric("--r-sm"))
            .border_1()
            .border_color(colors.border())
            .bg(colors.canvas())
            .child(TextElement { input: cx.entity() })
    }
}
impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
