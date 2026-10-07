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

#[cfg(target_os = "linux")]
pub const FOCUS_EPOCH: &str = "native-paste-focus-epoch";

#[derive(Clone, Default)]
pub struct PasteInput(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    modifiers: HashMap<WindowId, ModifiersState>,
    pending: HashMap<WindowId, egui::Modifiers>,
    redrawing: Option<WindowId>,
    #[cfg(target_os = "linux")]
    focus_epoch: u64,
}

impl PasteInput {
    pub fn begin_event(&self, window: WindowId, event: &WindowEvent) {
        let mut state = self.0.lock().unwrap();
        // A blur/refocus pair may arrive between redraws. RawInput.focused
        // alone cannot detect that an asynchronous paste lost its owner.
        #[cfg(target_os = "linux")]
        if matches!(event, WindowEvent::Focused(_) | WindowEvent::Destroyed) {
            state.focus_epoch = state.focus_epoch.wrapping_add(1);
        }
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

    #[cfg(target_os = "linux")]
    pub fn focus_epoch(&self) -> u64 {
        self.0.lock().unwrap().focus_epoch
    }

    pub fn append(&self, input: &mut egui::RawInput) {
        let mut state = self.0.lock().unwrap();
        let Some(window) = state.redrawing else {
            return;
        };
        if let Some(modifiers) = state.pending.remove(&window)
            && input.focused
        {
            // A quick tap can press and release before the next redraw. Keep
            // our recovered down before winit's normal up, avoiding stuck V.
            let index = input
                .events
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::V,
                            pressed: false,
                            ..
                        }
                    )
                })
                .unwrap_or(input.events.len());
            input.events.insert(
                index,
                egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn native_blur_refocus_between_redraws_changes_the_paste_epoch() {
        let bridge = PasteInput::default();
        let window = WindowId::from(1);
        let before = bridge.focus_epoch();
        bridge.begin_event(window, &WindowEvent::Focused(false));
        bridge.end_event();
        bridge.begin_event(window, &WindowEvent::Focused(true));
        bridge.end_event();
        bridge.begin_event(window, &WindowEvent::RedrawRequested);
        assert_ne!(bridge.focus_epoch(), before);
        assert_eq!(bridge.focus_epoch(), 2);
        bridge.end_event();
    }

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
        input.events.push(egui::Event::Key {
            key: egui::Key::V,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        });
        bridge.begin_event(first, &WindowEvent::RedrawRequested);
        bridge.append(&mut input);
        bridge.append(&mut input);
        assert_eq!(input.events.len(), 2);
        assert!(matches!(
            input.events[0],
            egui::Event::Key {
                key: egui::Key::V,
                pressed: true,
                ..
            }
        ));
        assert!(matches!(
            input.events[1],
            egui::Event::Key { pressed: false, .. }
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
