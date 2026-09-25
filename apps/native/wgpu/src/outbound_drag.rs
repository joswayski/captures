//! Bridges a child viewport gesture to the owning native event loop. File I/O
//! happens on a worker; a released/replaced press can never start a late drag.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

use captures_app::preview::{PreviewDragOutcome, PreviewDropLanding, preview_drag_outcome};
use eframe::egui;
use winit::{
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    window::WindowId,
};

type Completion = Box<dyn FnOnce(Result<PreviewDragOutcome, String>) + Send>;

struct Prepared {
    source: WindowId,
    press: u64,
    viewport: egui::ViewportId,
    path: Result<PathBuf, String>,
    #[cfg(target_os = "windows")]
    icon: Vec<u8>,
    completion: Completion,
    ctx: egui::Context,
}

#[derive(Default)]
struct State {
    redrawing: Option<WindowId>,
    presses: HashMap<WindowId, u64>,
    next_press: u64,
    preparing: bool,
    reset_pointer: HashSet<egui::ViewportId>,
    #[cfg(target_os = "windows")]
    window: Option<std::sync::Weak<winit::window::Window>>,
}

#[derive(Clone)]
pub struct Bridge {
    state: Arc<Mutex<State>>,
    tx: mpsc::Sender<Prepared>,
    rx: Arc<Mutex<mpsc::Receiver<Prepared>>>,
}

impl Default for Bridge {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            state: Arc::default(),
            tx,
            rx: Arc::new(Mutex::new(rx)),
        }
    }
}

fn key() -> egui::Id {
    egui::Id::unique("native-outbound-file-drag")
}

impl Bridge {
    pub fn install(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(key(), self.clone()));
    }

    #[cfg(target_os = "windows")]
    pub fn set_window(&self, window: &Arc<winit::window::Window>) {
        self.state.lock().unwrap().window = Some(Arc::downgrade(window));
    }

    pub fn begin_event(&self, window: WindowId, event: &WindowEvent) {
        let mut state = self.state.lock().unwrap();
        match event {
            WindowEvent::RedrawRequested => state.redrawing = Some(window),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                state.next_press += 1;
                let press = state.next_press;
                state.presses.insert(window, press);
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            }
            | WindowEvent::Destroyed => {
                state.presses.remove(&window);
            }
            _ => {}
        }
    }

    pub fn end_event(&self) {
        self.state.lock().unwrap().redrawing = None;
    }

    pub fn service(&self, _event_loop: &ActiveEventLoop) {
        loop {
            // Do not retain a lock across an OS nested drag loop or callbacks.
            let Ok(prepared) = ({ self.rx.lock().unwrap().try_recv() }) else {
                break;
            };
            let current = {
                let mut state = self.state.lock().unwrap();
                state.preparing = false;
                state.presses.get(&prepared.source) == Some(&prepared.press)
            };
            if !current {
                (prepared.completion)(Ok(PreviewDragOutcome::Retain));
                continue;
            }
            let path = match prepared.path {
                Ok(path) => path,
                Err(error) => {
                    (prepared.completion)(Err(error));
                    continue;
                }
            };
            let state = self.state.clone();
            let wake = prepared.ctx.clone();
            let completion: Completion = Box::new(move |result| {
                {
                    let mut state = state.lock().unwrap();
                    state.presses.remove(&prepared.source);
                    state.reset_pointer.insert(prepared.viewport);
                }
                (prepared.completion)(result);
                wake.request_repaint_of(prepared.viewport);
            });
            let completion = Arc::new(Mutex::new(Some(completion)));
            let returned = completion.clone();
            let ctx = prepared.ctx;
            #[cfg(target_os = "windows")]
            let result = {
                let window = self
                    .state
                    .lock()
                    .unwrap()
                    .window
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade);
                let rectangles = ctx.input(|input| {
                    input
                        .raw
                        .viewports
                        .iter()
                        .filter_map(|(id, viewport)| {
                            (viewport.minimized != Some(true))
                                .then_some(viewport.outer_rect?)
                                .map(|rect| {
                                    let scale = viewport.native_pixels_per_point.unwrap_or(1.);
                                    (
                                        *id,
                                        egui::Rect::from_min_max(
                                            rect.min * scale,
                                            rect.max * scale,
                                        ),
                                    )
                                })
                        })
                        .collect::<Vec<_>>()
                });
                window
                    .ok_or_else(|| "Native window is unavailable.".to_owned())
                    .and_then(|window| {
                        crate::windows_drag::start(
                            &window,
                            path,
                            prepared.icon,
                            Box::new(move |accepted, point| {
                                let own = rectangles.iter().any(|(_, rect)| {
                                    rect.contains(egui::pos2(point.x as f32, point.y as f32))
                                });
                                let same_source = rectangles.iter().any(|(id, rect)| {
                                    *id == prepared.viewport
                                        && rect.contains(egui::pos2(point.x as f32, point.y as f32))
                                });
                                if let Some(done) = returned.lock().unwrap().take() {
                                    done(Ok(outcome(accepted, own, same_source)));
                                }
                                ctx.request_repaint();
                            }),
                        )
                    })
            };
            #[cfg(not(target_os = "windows"))]
            let result = start(
                _event_loop,
                prepared.source,
                path,
                Box::new(move |accepted, own, same_source| {
                    if let Some(done) = returned.lock().unwrap().take() {
                        done(Ok(outcome(accepted, own, same_source)));
                    }
                    ctx.request_repaint();
                }),
            );
            if let Err(error) = result
                && let Some(done) = completion.lock().unwrap().take()
            {
                done(Err(error));
            }
        }
    }
}

fn outcome(accepted: bool, own: bool, same_source: bool) -> PreviewDragOutcome {
    preview_drag_outcome(
        accepted,
        if same_source {
            PreviewDropLanding::PreviewStack
        } else if own {
            PreviewDropLanding::AppWindow
        } else {
            PreviewDropLanding::External
        },
    )
}

pub fn request(
    ctx: &egui::Context,
    root: PathBuf,
    artifact: String,
    completion: Completion,
) -> bool {
    let Some(bridge) = ctx.data(|data| data.get_temp::<Bridge>(key())) else {
        return false;
    };
    let (source, press) = {
        let mut state = bridge.state.lock().unwrap();
        let Some(source) = state.redrawing else {
            return false;
        };
        let Some(&press) = state.presses.get(&source) else {
            return false;
        };
        if state.preparing {
            return false;
        }
        state.preparing = true;
        (source, press)
    };
    let ctx = ctx.clone();
    let viewport = ctx.viewport_id();
    std::thread::spawn(move || {
        let path = captures_app::preview_drag::prepare(&root, &artifact)
            .map_err(|error| error.to_string());
        #[cfg(target_os = "windows")]
        let icon = captures_history::entry_directory(&root, &artifact)
            .ok()
            .and_then(|dir| std::fs::read(dir.join(captures_history::HISTORY_PREVIEW_FILE)).ok())
            .unwrap_or_default();
        let _ = bridge.tx.send(Prepared {
            source,
            press,
            viewport,
            path,
            completion,
            ctx: ctx.clone(),
            #[cfg(target_os = "windows")]
            icon,
        });
        ctx.request_repaint_of(viewport);
    });
    true
}

pub fn append(ctx: &egui::Context, input: &mut egui::RawInput) {
    if let Some(bridge) = ctx.data(|data| data.get_temp::<Bridge>(key()))
        && bridge
            .state
            .lock()
            .unwrap()
            .reset_pointer
            .remove(&input.viewport_id)
    {
        // Native drag loops can consume the initiating release. End egui's
        // gesture before any newly queued press, without synthesizing a click.
        input.events.insert(0, egui::Event::PointerGone);
        input.events.insert(
            0,
            egui::Event::PointerButton {
                pos: egui::pos2(-1., -1.),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        );
    }
}

#[cfg(target_os = "linux")]
fn start(
    event_loop: &ActiveEventLoop,
    source: WindowId,
    path: PathBuf,
    done: Box<dyn FnOnce(bool, bool, bool) + Send>,
) -> Result<(), String> {
    use winit::platform::{wayland::ActiveEventLoopExtWayland, x11::ActiveEventLoopExtX11};
    if event_loop.is_wayland() {
        ActiveEventLoopExtWayland::start_file_drag(event_loop, source, path, done)
    } else {
        ActiveEventLoopExtX11::start_file_drag(event_loop, source, path, done)
    }
    .map_err(str::to_owned)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn start(
    _: &ActiveEventLoop,
    _: WindowId,
    _: PathBuf,
    _: Box<dyn FnOnce(bool, bool, bool) + Send>,
) -> Result<(), String> {
    Err("Native file dragging is not connected on this host yet.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accepted_external_copy_dismisses() {
        assert_eq!(outcome(true, false, false), PreviewDragOutcome::Dismiss);
        assert_eq!(outcome(true, true, false), PreviewDragOutcome::Retain);
        assert_eq!(outcome(true, true, true), PreviewDragOutcome::Reject);
        assert_eq!(outcome(false, true, true), PreviewDragOutcome::Retain);
        assert_eq!(outcome(false, false, false), PreviewDragOutcome::Retain);
    }

    #[test]
    fn native_release_resets_only_source_before_next_input_once() {
        let ctx = egui::Context::default();
        let bridge = Bridge::default();
        bridge.install(&ctx);
        let source = egui::ViewportId::from_hash_of("source");
        bridge.state.lock().unwrap().reset_pointer.insert(source);
        let mut other = egui::RawInput::default();
        append(&ctx, &mut other);
        assert!(other.events.is_empty());
        let next = egui::Event::PointerMoved(egui::pos2(40., 70.));
        let mut input = egui::RawInput {
            viewport_id: source,
            events: vec![next.clone()],
            ..Default::default()
        };
        append(&ctx, &mut input);
        assert!(matches!(
            input.events[0],
            egui::Event::PointerButton { pressed: false, .. }
        ));
        assert_eq!(input.events[1], egui::Event::PointerGone);
        assert_eq!(input.events[2], next);
        append(&ctx, &mut input);
        assert_eq!(input.events.len(), 3);
    }
}
