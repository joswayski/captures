//! Preserve capture-menu input ownership before native events become a batch.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};

use captures_app::shortcuts::{self, SelectorInputScope};
use eframe::egui;
use winit::{
    event::{ElementState, MouseButton, TouchPhase, WindowEvent},
    keyboard::{Key, NamedKey},
    window::WindowId,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Primary(bool),
    Enter,
}

fn kind(event: &egui::Event) -> Option<Kind> {
    match event {
        egui::Event::PointerButton {
            button: egui::PointerButton::Primary,
            pressed,
            ..
        } => Some(Kind::Primary(*pressed)),
        egui::Event::Key {
            key: egui::Key::Enter,
            pressed: true,
            ..
        } => Some(Kind::Enter),
        _ => None,
    }
}

#[derive(Clone, Default)]
pub struct Input(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    pending: HashMap<WindowId, VecDeque<(Kind, Option<SelectorInputScope>)>>,
    gestures: HashMap<WindowId, Option<SelectorInputScope>>,
    pointer_present: HashSet<WindowId>,
    pointer_touch: HashMap<WindowId, u64>,
    applied: Option<SelectorInputScope>,
}

#[derive(Clone, Default)]
struct Batch(Vec<bool>);

fn id(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique(("selector-input", ctx.viewport_id()))
}

pub fn eligibility(ctx: &egui::Context) -> Option<Vec<bool>> {
    let id = id(ctx);
    ctx.data(|data| data.get_temp::<Batch>(id))
        .map(|batch| batch.0)
}

impl Input {
    pub fn begin_event(&self, window: WindowId, event: &WindowEvent) {
        let mut state = self.0.lock().unwrap();
        match event {
            WindowEvent::CursorMoved { .. } => {
                state.pointer_present.insert(window);
            }
            WindowEvent::CursorLeft { .. } => {
                state.pointer_present.remove(&window);
            }
            _ => {}
        }
        // Match egui-winit's admission rules: buttons need a cached cursor
        // position and synthetic key presses are dropped. Return releases do
        // not confirm anything (and Windows IME can suppress those releases).
        let source = match event {
            WindowEvent::MouseInput {
                state: button_state,
                button: MouseButton::Left,
                ..
            } if state.pointer_present.contains(&window) => {
                Some(Kind::Primary(*button_state == ElementState::Pressed))
            }
            // egui-winit emulates primary input for only its first touch.
            WindowEvent::Touch(touch)
                if state
                    .pointer_touch
                    .get(&window)
                    .is_none_or(|id| *id == touch.id) =>
            {
                match touch.phase {
                    TouchPhase::Started => {
                        state.pointer_touch.insert(window, touch.id);
                        state.pointer_present.insert(window);
                        Some(Kind::Primary(true))
                    }
                    TouchPhase::Moved => {
                        state.pointer_present.insert(window);
                        None
                    }
                    TouchPhase::Ended => {
                        state.pointer_touch.remove(&window);
                        state
                            .pointer_present
                            .remove(&window)
                            .then_some(Kind::Primary(false))
                    }
                    TouchPhase::Cancelled => {
                        state.pointer_touch.remove(&window);
                        state.pointer_present.remove(&window);
                        None
                    }
                }
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.logical_key == Key::Named(NamedKey::Enter)
                && event.state == ElementState::Pressed =>
            {
                Some(Kind::Enter)
            }
            _ => None,
        };
        let scope = source.and_then(|_| shortcuts::selector_input_scope());
        if let Some(source) = source {
            state
                .pending
                .entry(window)
                .or_default()
                .push_back((source, scope));
        }
        if event == &WindowEvent::Destroyed {
            state.pending.remove(&window);
            state.gestures.remove(&window);
            state.pointer_present.remove(&window);
            state.pointer_touch.remove(&window);
        }
    }

    pub fn set_scope(&self, scope: Option<SelectorInputScope>) {
        self.0.lock().unwrap().applied = scope;
    }

    pub fn set_generation(&self, generation: Option<u64>) {
        let mut state = self.0.lock().unwrap();
        if state.applied.map(|scope| scope.generation) != generation {
            state.applied = generation.map(|generation| SelectorInputScope {
                generation,
                revision: 0,
            });
        }
    }

    /// The raw-input hook supplies its actual viewport's native window, which
    /// need not be the window whose redraw caused eframe to dispatch this pass.
    pub fn publish(&self, ctx: &egui::Context, input: &egui::RawInput, window: Option<WindowId>) {
        let batch_id = egui::Id::unique(("selector-input", input.viewport_id));
        let mut state = self.0.lock().unwrap();
        let Some(window) = window else {
            ctx.data_mut(|data| data.insert_temp(batch_id, Batch(vec![false; input.events.len()])));
            return;
        };
        let applied = state.applied;
        let mut pending = state.pending.remove(&window).unwrap_or_default();
        let batch = Batch(
            input
                .events
                .iter()
                .map(|event| {
                    if matches!(event, egui::Event::WindowFocused(false)) {
                        // egui retains queued input across blur. Cancel its
                        // gesture in order without losing those receipt stamps.
                        state.gestures.remove(&window);
                        return false;
                    }
                    let Some(kind) = kind(event) else {
                        return false;
                    };
                    let Some((source, scope)) = pending.pop_front() else {
                        return false;
                    };
                    let owned = match source {
                        Kind::Primary(true) => {
                            state.gestures.insert(window, scope);
                            true
                        }
                        Kind::Primary(false) => state.gestures.remove(&window) == Some(scope),
                        Kind::Enter => true,
                    };
                    source == kind && scope == applied && owned
                })
                .collect(),
        );
        // Preserve raw pointer-up for the inactive Region's owned drag.
        ctx.data_mut(|data| data.insert_temp(batch_id, batch));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_controls::{Action, CaptureControls, Target, View};
    use captures_app::shortcuts::CaptureShortcut;
    use captures_capture::{DisplayDescriptor, WindowDescriptor};

    fn pointer(x: f32, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(x, 230.),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn enter() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    // Supply receipt stamps independently of render time. The production
    // wrapper records these before forwarding each native event to eframe.
    fn frame(
        ctx: &egui::Context,
        controls: &mut CaptureControls,
        input: &Input,
        events: Vec<(egui::Event, Option<SelectorInputScope>)>,
        auto_start: bool,
    ) -> Option<Action> {
        let window = WindowId::from(1);
        input.0.lock().unwrap().pending.insert(
            window,
            events
                .iter()
                .filter_map(|(event, scope)| kind(event).map(|kind| (kind, *scope)))
                .collect(),
        );
        show_frame(
            ctx,
            controls,
            input,
            events.into_iter().map(|(event, _)| event).collect(),
            auto_start,
        )
    }

    fn show_frame(
        ctx: &egui::Context,
        controls: &mut CaptureControls,
        input: &Input,
        events: Vec<egui::Event>,
        auto_start: bool,
    ) -> Option<Action> {
        let window = WindowId::from(1);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 720.),
            )),
            events,
            ..Default::default()
        };
        input.publish(ctx, &raw, Some(window));
        ctx.begin_pass(raw);
        let display = DisplayDescriptor {
            id: "fixture".into(),
            name: "Fixture".into(),
            x: -100,
            y: 50,
            width: 1000,
            height: 720,
            scale_factor: 1.,
            is_primary: true,
        };
        let back = WindowDescriptor {
            id: "back".into(),
            title: "Back".into(),
            app_name: None,
            z_order: 10,
            x: 20,
            y: 140,
            width: 620,
            height: 420,
            display_id: "fixture".into(),
            corner_radius: None,
        };
        let windows = [
            back.clone(),
            WindowDescriptor {
                id: "front".into(),
                title: "Front".into(),
                z_order: 20,
                x: 380,
                y: 230,
                width: 330,
                height: 240,
                ..back
            },
        ];
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("input-contract"),
            egui::UiBuilder::new().max_rect(ctx.content_rect()),
        );
        let action = controls.show(
            &mut ui,
            &crate::tokens::load()["dark-mustard"],
            View {
                panel_id: egui::Id::unique("input-contract-toolbar"),
                frozen: None,
                display: &display,
                displays: std::slice::from_ref(&display),
                windows: &windows,
                auto_start,
                recording_available: false,
                recording_unavailable_reason: None,
                error: None,
            },
            |point| {
                // The surface reports display-local points; catalog bounds
                // are global, just like the live capture hit-test contract.
                let x = point.x + f64::from(display.x);
                let y = point.y + f64::from(display.y);
                windows.iter().rposition(|window| {
                    x >= window.x as f64
                        && x < (window.x + window.width as i32) as f64
                        && y >= window.y as f64
                        && y < (window.y + window.height as i32) as f64
                })
            },
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    #[test]
    fn admitted_native_pointer_input_survives_dropped_buttons_and_blur() {
        let window = WindowId::from(1);
        let device_id = winit::event::DeviceId::dummy();
        for (blur, reclick) in [(false, false), (true, false), (true, true)] {
            let ctx = egui::Context::default();
            let input = Input::default();
            let mut controls = CaptureControls::default();
            controls.apply_target_shortcut(CaptureShortcut::Window);
            show_frame(&ctx, &mut controls, &input, vec![], true);
            let mut events = vec![];
            // Without a cached position egui-winit drops both buttons. Their
            // nonexistent RawInput must not consume the fresh gesture's stamp.
            for state in [ElementState::Pressed, ElementState::Released] {
                input.begin_event(
                    window,
                    &WindowEvent::MouseInput {
                        device_id,
                        state,
                        button: MouseButton::Left,
                    },
                );
            }
            for pressed in [true, false] {
                let x = if pressed { 479. } else { 481. };
                input.begin_event(
                    window,
                    &WindowEvent::CursorMoved {
                        device_id,
                        position: winit::dpi::PhysicalPosition::new(x, 230.),
                    },
                );
                events.push(egui::Event::PointerMoved(egui::pos2(x as f32, 230.)));
                if !pressed && blur {
                    input.begin_event(window, &WindowEvent::Focused(false));
                    events.push(egui::Event::WindowFocused(false));
                }
                input.begin_event(
                    window,
                    &WindowEvent::MouseInput {
                        device_id,
                        state: if pressed {
                            ElementState::Pressed
                        } else {
                            ElementState::Released
                        },
                        button: MouseButton::Left,
                    },
                );
                events.push(pointer(x as f32, pressed));
            }
            if reclick {
                input.begin_event(window, &WindowEvent::Focused(true));
                events.push(egui::Event::WindowFocused(true));
                input.begin_event(
                    window,
                    &WindowEvent::CursorMoved {
                        device_id,
                        position: winit::dpi::PhysicalPosition::new(479., 230.),
                    },
                );
                events.push(egui::Event::PointerMoved(egui::pos2(479., 230.)));
                for state in [ElementState::Pressed, ElementState::Released] {
                    input.begin_event(
                        window,
                        &WindowEvent::MouseInput {
                            device_id,
                            state,
                            button: MouseButton::Left,
                        },
                    );
                    events.push(pointer(479., state == ElementState::Pressed));
                }
            }
            assert_eq!(
                show_frame(&ctx, &mut controls, &input, events, true),
                if reclick {
                    Some(Action::Capture(Target::Window(0)))
                } else {
                    (!blur).then_some(Action::Capture(Target::Window(1)))
                }
            );
        }
        let ctx = egui::Context::default();
        let input = Input::default();
        let mut controls = CaptureControls::default();
        controls.apply_target_shortcut(CaptureShortcut::Window);
        show_frame(&ctx, &mut controls, &input, vec![], true);
        // Touch also emulates primary input. A second simultaneous touch must
        // not insert a stamp for a pointer button egui-winit never emits.
        for (id, phase) in [
            (7, TouchPhase::Started),
            (8, TouchPhase::Started),
            (7, TouchPhase::Ended),
        ] {
            input.begin_event(
                window,
                &WindowEvent::Touch(winit::event::Touch {
                    device_id,
                    phase,
                    location: winit::dpi::PhysicalPosition::new(481., 230.),
                    force: None,
                    id,
                }),
            );
        }
        assert_eq!(
            show_frame(
                &ctx,
                &mut controls,
                &input,
                vec![pointer(481., true), pointer(481., false)],
                true
            ),
            Some(Action::Capture(Target::Window(1)))
        );
    }

    #[test]
    fn reselection_rejects_old_gestures_but_keeps_first_fresh_input_and_enter_order() {
        let old = Some(SelectorInputScope {
            generation: 9,
            revision: 1,
        });
        let display = Some(SelectorInputScope {
            generation: 9,
            revision: 2,
        });
        let fresh = Some(SelectorInputScope {
            generation: 9,
            revision: 3,
        });
        for auto_start in [false, true] {
            for split in [false, true] {
                let ctx = egui::Context::default();
                crate::motion::set_reduced(&ctx, true);
                let input = Input::default();
                let mut controls = CaptureControls::default();
                controls.apply_target_shortcut(CaptureShortcut::Window);
                input.set_scope(old);
                frame(&ctx, &mut controls, &input, vec![], auto_start);
                if split {
                    assert_eq!(
                        frame(
                            &ctx,
                            &mut controls,
                            &input,
                            vec![(pointer(479., true), old)],
                            auto_start
                        ),
                        None
                    );
                }
                controls.apply_target_shortcut(CaptureShortcut::Display);
                input.set_scope(display);
                assert_eq!(
                    frame(
                        &ctx,
                        &mut controls,
                        &input,
                        vec![(enter(), old)],
                        auto_start
                    ),
                    None,
                    "Return from the old target must not publish Full screen"
                );
                controls.apply_target_shortcut(CaptureShortcut::Window);
                input.set_scope(fresh);
                let stale = if split {
                    vec![(pointer(481., false), fresh)]
                } else {
                    vec![(pointer(479., true), old), (pointer(481., false), old)]
                };
                assert_eq!(
                    frame(&ctx, &mut controls, &input, stale, auto_start),
                    None,
                    "old or split gesture must not relatch/auto-capture"
                );
                assert_eq!(
                    frame(
                        &ctx,
                        &mut controls,
                        &input,
                        vec![(enter(), fresh)],
                        auto_start
                    ),
                    None
                );
                // Enter precedes a fresh press in its first frame; release is
                // two pixels across the Back/Front boundary in the next frame.
                assert_eq!(
                    frame(
                        &ctx,
                        &mut controls,
                        &input,
                        vec![(enter(), fresh), (pointer(479., true), fresh)],
                        auto_start
                    ),
                    None
                );
                let action = frame(
                    &ctx,
                    &mut controls,
                    &input,
                    vec![
                        (egui::Event::PointerMoved(egui::pos2(479., 230.)), fresh),
                        (pointer(481., false), fresh),
                        (egui::Event::PointerMoved(egui::pos2(920., 680.)), fresh),
                    ],
                    auto_start,
                );
                assert_eq!(
                    action,
                    auto_start.then_some(Action::Capture(Target::Window(1)))
                );
                if !auto_start {
                    assert_eq!(
                        frame(&ctx, &mut controls, &input, vec![(enter(), fresh)], false),
                        Some(Action::Capture(Target::Window(1)))
                    );
                }
            }
        }
        for enter_first in [false, true] {
            let ctx = egui::Context::default();
            let input = Input::default();
            let mut controls = CaptureControls::default();
            controls.apply_target_shortcut(CaptureShortcut::Display);
            input.set_scope(display);
            frame(&ctx, &mut controls, &input, vec![], false);
            controls.apply_target_shortcut(CaptureShortcut::Window);
            input.set_scope(fresh);
            let mut events = vec![(pointer(479., true), fresh), (pointer(481., false), fresh)];
            events.insert(if enter_first { 0 } else { events.len() }, (enter(), fresh));
            assert_eq!(
                frame(&ctx, &mut controls, &input, events, false),
                (!enter_first).then_some(Action::Capture(Target::Window(1)))
            );
            if enter_first {
                assert_eq!(
                    frame(&ctx, &mut controls, &input, vec![(enter(), fresh)], false),
                    Some(Action::Capture(Target::Window(1)))
                );
            }
        }
    }
}
