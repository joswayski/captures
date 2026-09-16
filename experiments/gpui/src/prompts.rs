//! Keyboard-capable Linux fallback. Other platforms retain their native dialogs.
use crate::theme::Theme;
use gpui::*;

pub fn install(cx: &mut App) {
    cx.set_prompt_builder(|_, message, detail, buttons, handle, window, cx| {
        let view = cx.new(|cx| Confirmation {
            message: message.to_owned(),
            detail: detail.map(str::to_owned),
            buttons: buttons.to_vec(),
            selected: cancel_index(buttons).unwrap_or(0),
            keyboard_selection: false,
            focus: cx.focus_handle(),
        });
        handle.with_view(view, window, cx)
    });
}

fn cancel_index(buttons: &[PromptButton]) -> Option<usize> {
    buttons
        .iter()
        .position(|button| matches!(button, PromptButton::Cancel(_)))
}

#[derive(Debug, PartialEq)]
enum Command {
    Ignore,
    Select(usize),
    Answer(usize),
}

fn command(buttons: &[PromptButton], selected: usize, key: &str, shift: bool) -> Command {
    if buttons.is_empty() {
        return Command::Ignore;
    }
    match key {
        "escape" => cancel_index(buttons).map_or(Command::Ignore, Command::Answer),
        "enter" | "space" => Command::Answer(selected),
        "tab" | "up" | "down" | "left" | "right" => {
            let backwards = matches!(key, "up" | "left") || (key == "tab" && shift);
            let delta = if backwards { buttons.len() - 1 } else { 1 };
            Command::Select((selected + delta) % buttons.len())
        }
        _ => Command::Ignore,
    }
}

struct Confirmation {
    message: String,
    detail: Option<String>,
    buttons: Vec<PromptButton>,
    selected: usize,
    keyboard_selection: bool,
    focus: FocusHandle,
}

impl EventEmitter<PromptResponse> for Confirmation {}

impl Focusable for Confirmation {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Confirmation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Retain the compact vertical fallback layout in narrow floating windows.
        let t = Theme::new(true);
        let mut scrim = t.subtle;
        scrim.a = 0.6;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(scrim)
            .child(
                div()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        let mods = &event.keystroke.modifiers;
                        if mods.control || mods.alt || mods.platform {
                            return;
                        }
                        match command(
                            &this.buttons,
                            this.selected,
                            &event.keystroke.key,
                            mods.shift,
                        ) {
                            Command::Ignore => return,
                            Command::Select(index) => {
                                this.selected = index;
                                this.keyboard_selection = true;
                                cx.notify();
                            }
                            Command::Answer(index) => cx.emit(PromptResponse(index)),
                        }
                        cx.stop_propagation();
                    }))
                    .cursor_default()
                    .w_72()
                    .rounded_lg()
                    .p_3()
                    .bg(t.raised)
                    .text_color(t.text)
                    .text_center()
                    .child(div().child(self.message.clone()))
                    .children(
                        self.detail
                            .clone()
                            .map(|detail| div().text_sm().mb_2().child(detail)),
                    )
                    .children(self.buttons.iter().enumerate().map(|(index, button)| {
                        div()
                            .id(index)
                            .border_1()
                            .border_color(if self.keyboard_selection && self.selected == index {
                                t.accent
                            } else {
                                t.border_strong
                            })
                            .mt_1()
                            .rounded_xs()
                            .cursor_pointer()
                            .text_sm()
                            .child(button.label().clone())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(PromptResponse(index));
                            }))
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, cancel_index, command};
    use gpui::PromptButton;

    fn buttons() -> Vec<PromptButton> {
        vec![
            PromptButton::Ok("Delete".into()),
            PromptButton::Other("Show file".into()),
            PromptButton::Cancel("Keep".into()),
        ]
    }

    #[test]
    fn cancel_is_typed_not_label_or_position_and_enter_defaults_to_it() {
        let buttons = buttons();
        let selected = cancel_index(&buttons).unwrap();
        assert_eq!(selected, 2);
        assert_eq!(command(&buttons, 0, "escape", false), Command::Answer(2));
        assert_eq!(
            command(&buttons, selected, "enter", false),
            Command::Answer(2)
        );
        assert_eq!(command(&buttons, 1, "space", false), Command::Answer(1));
    }

    #[test]
    fn navigation_wraps_in_both_directions_without_answering() {
        let buttons = buttons();
        assert_eq!(command(&buttons, 2, "tab", false), Command::Select(0));
        assert_eq!(command(&buttons, 0, "tab", true), Command::Select(2));
        assert_eq!(command(&buttons, 1, "up", false), Command::Select(0));
        assert_eq!(command(&buttons, 1, "down", true), Command::Select(2));
        assert_eq!(command(&buttons, 0, "left", false), Command::Select(2));
        assert_eq!(command(&buttons, 2, "right", false), Command::Select(0));
    }

    #[test]
    fn escape_never_confirms_when_there_is_no_cancel_button() {
        assert_eq!(
            command(&buttons()[..2], 0, "escape", false),
            Command::Ignore
        );
        assert_eq!(command(&[], 0, "tab", false), Command::Ignore);
        assert_eq!(command(&buttons(), 0, "delete", false), Command::Ignore);
    }
}
