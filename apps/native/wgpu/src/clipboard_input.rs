//! Preserve native paste key-down even when egui-winit finds no OS clipboard
//! payload. The raw-input hook runs synchronously inside the window redraw.
//! Text editors still receive egui-winit's normal Paste event unchanged.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use eframe::egui;
use winit::{
    event::{ElementState, WindowEvent},
    keyboard::{Key, ModifiersState},
    window::WindowId,
};

#[derive(Clone, Default)]
pub struct PasteInput(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    modifiers: HashMap<WindowId, ModifiersState>,
    pending: HashMap<WindowId, egui::Modifiers>,
    redrawing: Option<WindowId>,
}

impl PasteInput {
    pub fn begin_event(&self, window: WindowId, event: &WindowEvent) {
        let mut state = self.0.lock().unwrap();
        match event {
            WindowEvent::RedrawRequested => state.redrawing = Some(window),
            WindowEvent::ModifiersChanged(value) => {
                state.modifiers.insert(window, value.state());
            }
            WindowEvent::Focused(false) | WindowEvent::Destroyed => {
                state.pending.remove(&window);
                state.modifiers.remove(&window);
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed && !event.repeat => {
                let modifiers = state.modifiers.get(&window).copied().unwrap_or_default();
                if (modifiers.control_key() || modifiers.super_key())
                    && matches!(&event.logical_key, Key::Character(key) if key.eq_ignore_ascii_case("v"))
                {
                    state.pending.insert(
                        window,
                        egui::Modifiers {
                            ctrl: modifiers.control_key(),
                            command: modifiers.control_key() || modifiers.super_key(),
                            mac_cmd: modifiers.super_key(),
                            alt: modifiers.alt_key(),
                            shift: modifiers.shift_key(),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    pub fn end_event(&self) {
        self.0.lock().unwrap().redrawing = None;
    }

    pub fn append(&self, input: &mut egui::RawInput) {
        let mut state = self.0.lock().unwrap();
        let Some(window) = state.redrawing else {
            return;
        };
        if let Some(modifiers) = state.pending.remove(&window)
            && input.focused
        {
            input.events.push(egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_is_window_scoped_once_and_cleared_on_blur() {
        let bridge = PasteInput::default();
        let first = WindowId::from(1);
        let second = WindowId::from(2);
        bridge
            .0
            .lock()
            .unwrap()
            .pending
            .insert(first, egui::Modifiers::CTRL);
        let mut input = egui::RawInput::default();
        bridge.begin_event(second, &WindowEvent::RedrawRequested);
        bridge.append(&mut input);
        assert!(input.events.is_empty());
        bridge.end_event();
        bridge.begin_event(first, &WindowEvent::RedrawRequested);
        bridge.append(&mut input);
        bridge.append(&mut input);
        assert_eq!(input.events.len(), 1);
        assert!(matches!(
            input.events[0],
            egui::Event::Key {
                key: egui::Key::V,
                pressed: true,
                ..
            }
        ));
        bridge.end_event();
        bridge
            .0
            .lock()
            .unwrap()
            .pending
            .insert(first, egui::Modifiers::CTRL);
        bridge.begin_event(first, &WindowEvent::Focused(false));
        bridge.end_event();
        bridge.begin_event(first, &WindowEvent::RedrawRequested);
        input.events.clear();
        bridge.append(&mut input);
        assert!(input.events.is_empty());
    }
}
