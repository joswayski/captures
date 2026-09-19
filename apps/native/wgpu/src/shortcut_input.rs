use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use eframe::egui;
use winit::{event::ElementState, keyboard::KeyCode};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Key {
        code: String,
        pressed: bool,
        repeat: bool,
        modifiers: Modifiers,
    },
    Blur,
}

#[derive(Default)]
struct State {
    context: Option<egui::Context>,
    events: VecDeque<Event>,
}

#[derive(Clone, Default)]
pub struct Bridge {
    active: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Bridge {
    pub fn attach(&self, context: egui::Context) {
        self.state.lock().unwrap().context = Some(context);
    }

    pub fn start(&self) {
        self.state.lock().unwrap().events.clear();
        self.active.store(true, Ordering::Release);
    }

    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
        self.state.lock().unwrap().events.clear();
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn key(&self, code: KeyCode, state: ElementState, repeat: bool, modifiers: Modifiers) {
        if !self.is_active() {
            return;
        }
        self.push(Event::Key {
            code: physical_code(code),
            pressed: state == ElementState::Pressed,
            repeat,
            modifiers,
        });
    }

    pub fn blur(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.push(Event::Blur);
        }
    }

    fn push(&self, event: Event) {
        let context = {
            let mut state = self.state.lock().unwrap();
            state.events.push_back(event);
            state.context.clone()
        };
        if let Some(context) = context {
            context.request_repaint();
        }
    }

    pub fn drain(&self) -> Vec<Event> {
        self.state.lock().unwrap().events.drain(..).collect()
    }
}

fn physical_code(code: KeyCode) -> String {
    match code {
        KeyCode::SuperLeft => "MetaLeft".into(),
        KeyCode::SuperRight => "MetaRight".into(),
        _ => format!("{code:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_codes_preserve_print_screen_keypad_and_super() {
        assert_eq!(physical_code(KeyCode::PrintScreen), "PrintScreen");
        assert_eq!(physical_code(KeyCode::Numpad7), "Numpad7");
        assert_eq!(physical_code(KeyCode::NumpadDivide), "NumpadDivide");
        assert_eq!(physical_code(KeyCode::Digit7), "Digit7");
        assert_eq!(physical_code(KeyCode::SuperLeft), "MetaLeft");
    }

    #[test]
    fn inactive_input_is_discarded_and_blur_stops_before_waking_ui() {
        let bridge = Bridge::default();
        bridge.key(
            KeyCode::KeyA,
            ElementState::Pressed,
            false,
            Modifiers::default(),
        );
        assert!(bridge.drain().is_empty());

        bridge.start();
        bridge.key(
            KeyCode::KeyA,
            ElementState::Pressed,
            false,
            Modifiers::default(),
        );
        bridge.blur();
        bridge.key(
            KeyCode::KeyB,
            ElementState::Pressed,
            false,
            Modifiers::default(),
        );
        assert_eq!(
            bridge.drain(),
            vec![
                Event::Key {
                    code: "KeyA".into(),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::default(),
                },
                Event::Blur,
            ]
        );
    }
}
