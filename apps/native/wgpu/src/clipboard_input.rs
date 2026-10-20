//! Match each native Paste occurrence to its owned acquisition, before redraw
//! and before a busy editor can defer document mutation.
use crate::editor_clipboard::{self, Copies, Decoded};
use eframe::egui;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, Weak},
};
use winit::{
    event::{ElementState, WindowEvent},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

pub enum Captured {
    Layer(Arc<captures_app::editor_session::CopiedLayer>),
    Images(std::sync::mpsc::Receiver<Decoded>),
}

struct InputWindow {
    window: Weak<Window>,
    copies: Arc<Mutex<Copies>>,
    ctx: egui::Context,
    viewport: egui::ViewportId,
    pending: VecDeque<Captured>,
}

#[derive(Default)]
struct State {
    windows: HashMap<WindowId, InputWindow>,
    modifiers: HashMap<WindowId, ModifiersState>,
}

fn key() -> egui::Id {
    egui::Id::unique("native-paste-input")
}
fn batch_key(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique(("native-paste-batch", ctx.viewport_id()))
}
type Batch = Arc<Mutex<VecDeque<Captured>>>;

pub fn register(ctx: &egui::Context, copies: Arc<Mutex<Copies>>) {
    let Some(bridge) = ctx.data(|data| data.get_temp::<PasteInput>(key())) else {
        return;
    };
    let window = ctx
        .data(|data| {
            data.get_temp::<Weak<Window>>(egui::Id::unique((
                "eframe-winit-window",
                ctx.viewport_id(),
            )))
        })
        .and_then(|window| window.upgrade());
    if let Some(window) = window {
        #[cfg(target_os = "linux")]
        {
            use winit::platform::wayland::WindowExtWayland;
            window.set_clipboard_paste_retention(Some(eframe::is_paste_key));
        }
        bridge
            .state
            .lock()
            .unwrap()
            .windows
            .entry(window.id())
            .or_insert_with(|| InputWindow {
                window: Arc::downgrade(&window),
                copies,
                ctx: ctx.clone(),
                viewport: ctx.viewport_id(),
                pending: VecDeque::new(),
            });
    }
}

pub fn take(ctx: &egui::Context) -> Option<Captured> {
    ctx.data(|data| data.get_temp::<Batch>(batch_key(ctx)))
        .and_then(|batch| batch.lock().unwrap().pop_front())
}

pub fn clear(ctx: &egui::Context) {
    if let Some(batch) = ctx.data(|data| data.get_temp::<Batch>(batch_key(ctx))) {
        batch.lock().unwrap().clear();
    }
}

#[cfg(target_os = "linux")]
pub const FOCUS_EPOCH: &str = "native-paste-focus-epoch";

#[derive(Clone, Default)]
pub struct PasteInput {
    state: Arc<Mutex<State>>,
    #[cfg(target_os = "linux")]
    focus_epoch: Arc<AtomicU64>,
}

impl PasteInput {
    pub fn begin_event(&self, window_id: WindowId, event: &WindowEvent) {
        // A blur/refocus pair may arrive between redraws. RawInput.focused
        // alone cannot detect that an asynchronous paste lost its owner.
        #[cfg(target_os = "linux")]
        if matches!(event, WindowEvent::Focused(_) | WindowEvent::Destroyed) {
            self.focus_epoch.fetch_add(1, Ordering::Relaxed);
        }
        let mut state = self.state.lock().unwrap();
        match event {
            WindowEvent::ModifiersChanged(value) => {
                state.modifiers.insert(window_id, value.state());
            }
            WindowEvent::Focused(false) => {
                state.modifiers.remove(&window_id);
            }
            WindowEvent::Destroyed => {
                state.modifiers.remove(&window_id);
                state.windows.remove(&window_id);
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => {
                let modifiers = state.modifiers.get(&window_id).copied().unwrap_or_default();
                if !eframe::is_paste_key(event, modifiers) {
                    return;
                }
                let Some(input) = state.windows.get_mut(&window_id) else {
                    return;
                };
                if input.window.strong_count() == 0 {
                    return;
                }
                #[cfg(target_os = "linux")]
                if let Some(window) = input.window.upgrade() {
                    use winit::platform::wayland::WindowExtWayland;
                    if let Some(offer) = window.take_clipboard_paste() {
                        let captured = match offer {
                            Ok(offer) => {
                                let layer = offer.marker.as_ref().and_then(|marker| {
                                    input.copies.lock().unwrap().find_marker(marker)
                                });
                                if let Some(layer) = layer {
                                    Captured::Layer(layer)
                                } else {
                                    Captured::Images(editor_clipboard::decode_async(
                                        Ok(editor_clipboard::Snapshot::from_wayland(offer)),
                                        &input.ctx,
                                        input.viewport,
                                    ))
                                }
                            }
                            Err(error) => Captured::Images(editor_clipboard::decode_async(
                                Err(error),
                                &input.ctx,
                                input.viewport,
                            )),
                        };
                        input.pending.push_back(captured);
                        return;
                    }
                }
                let layer = input.copies.lock().unwrap().retained();
                input.pending.push_back(if let Some(layer) = layer {
                    Captured::Layer(layer)
                } else {
                    Captured::Images(editor_clipboard::decode_async(
                        editor_clipboard::capture_native(),
                        &input.ctx,
                        input.viewport,
                    ))
                });
            }
            _ => {}
        }
    }

    pub fn publish(&self, ctx: &eframe::egui::Context, viewport: egui::ViewportId) {
        ctx.data_mut(|data| data.insert_temp(key(), self.clone()));
        #[cfg(target_os = "linux")]
        ctx.data_mut(|data| {
            data.insert_temp(
                eframe::egui::Id::unique(FOCUS_EPOCH),
                self.focus_epoch.load(Ordering::Relaxed),
            )
        });
        let window = ctx
            .data(|data| {
                data.get_temp::<Weak<Window>>(egui::Id::unique(("eframe-winit-window", viewport)))
            })
            .and_then(|window| window.upgrade());
        if let Some(window) = window {
            let mut state = self.state.lock().unwrap();
            state
                .windows
                .retain(|_, input| input.window.strong_count() != 0);
            let pending = state
                .windows
                .get_mut(&window.id())
                .map(|input| std::mem::take(&mut input.pending))
                .unwrap_or_default();
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::unique(("native-paste-batch", viewport)),
                    Arc::new(Mutex::new(pending)),
                )
            });
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn native_blur_refocus_between_redraws_changes_the_paste_epoch() {
        let bridge = PasteInput::default();
        let window = WindowId::from(1);
        let ctx = eframe::egui::Context::default();
        bridge.begin_event(window, &WindowEvent::Focused(false));
        bridge.begin_event(window, &WindowEvent::Focused(true));
        bridge.begin_event(window, &WindowEvent::RedrawRequested);
        bridge.publish(&ctx, egui::ViewportId::ROOT);
        assert_eq!(
            ctx.data(|data| data.get_temp::<u64>(eframe::egui::Id::unique(FOCUS_EPOCH))),
            Some(2)
        );
    }
}
