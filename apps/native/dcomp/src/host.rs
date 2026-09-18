use crate::{
    options::{Options, Scene},
    renderer::Renderer,
    scene::{Action, State, Tokens},
};
use serde_json::json;
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::HWND,
    System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    platform::windows::WindowAttributesExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Theme, Window, WindowId},
};

fn emit(event: &str, detail: serde_json::Value) {
    println!(
        "{}",
        json!({"schema":1,"pid":std::process::id(),"event":event,"detail":detail})
    );
}
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse(std::env::args().skip(1))
        .map_err(|error| format!("{error}\n{}", crate::options::USAGE))?;
    if options.live || options.settings_file.is_some() {
        return Err("DirectComposition is a synthetic renderer probe; live/settings integration is not connected".into());
    }
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let mut app = Host {
        state: State::new(options),
        window: None,
        renderer: None,
        started: Instant::now(),
        cycle: 0,
        paints: 0,
        settled: 0,
        total_ms: 0.,
        max_ms: 0.,
        pointer: [0.; 2],
        drag: None,
        screenshot_requested: false,
        screenshot_saved: false,
        recovered: false,
        failure: None,
    };
    emit(
        "starting",
        json!({"phase":"before native event loop and renderer initialization"}),
    );
    EventLoop::new()?.run_app(&mut app)?;
    if let Some(error) = app.failure {
        return Err(error.into());
    }
    Ok(())
}
struct Host {
    state: State,
    window: Option<Window>,
    renderer: Option<Renderer>,
    started: Instant,
    cycle: usize,
    paints: usize,
    settled: usize,
    total_ms: f64,
    max_ms: f64,
    pointer: [f32; 2],
    drag: Option<[f32; 2]>,
    screenshot_requested: bool,
    screenshot_saved: bool,
    recovered: bool,
    failure: Option<String>,
}
impl Host {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl ToString) {
        self.failure = Some(error.to_string());
        emit("render-error", json!({"message":self.failure}));
        event_loop.exit();
    }
    fn renderer(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.renderer.is_none() {
            let window = self.window.as_ref().unwrap();
            let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
                unreachable!()
            };
            let size = window.inner_size();
            self.renderer = Some(Renderer::new(
                HWND(handle.hwnd.get() as *mut _),
                [size.width.max(1), size.height.max(1)],
            )?);
            if self.state.options.floating {
                self.renderer
                    .as_ref()
                    .unwrap()
                    .opacity(self.state.deleted, true, 0.)?;
            }
        }
        Ok(())
    }
    fn action(&mut self, action: Action) {
        self.state.activate(action);
        if matches!(action, Action::Search) {
            self.window.as_ref().unwrap().set_ime_cursor_area(
                LogicalPosition::new(260., 160.),
                LogicalSize::new(300., 32.),
            );
        }
        if self.state.options.floating && matches!(action, Action::Preview) {
            let tokens = Tokens::load(&self.state.options, false);
            if let Some(renderer) = &self.renderer
                && let Err(error) = renderer.opacity(
                    self.state.deleted,
                    self.state.options.reduced_motion,
                    tokens.number("dur-4") / 1000.,
                )
            {
                self.failure = Some(error.to_string());
            }
        } else if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    fn paint(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.options.scene == Scene::Idle {
            return;
        }
        if let Err(error) = self.renderer() {
            self.fail(event_loop, error);
            return;
        }
        let window = self.window.as_ref().unwrap();
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let scale = window.scale_factor() as f32;
        let start = Instant::now();
        let tokens = Tokens::load(&self.state.options, window.theme() == Some(Theme::Light));
        let nodes = self.state.nodes(
            size.width as f32 / scale,
            size.height as f32 / scale,
            &tokens,
        );
        let screenshot = if self.screenshot_requested && !self.screenshot_saved {
            self.state.options.screenshot.as_deref()
        } else {
            None
        };
        let renderer = self.renderer.as_mut().unwrap();
        let result = renderer
            .resize([size.width, size.height])
            .and_then(|_| renderer.paint(&nodes, scale, screenshot));
        if let Err(error) = result {
            // Release the complete graph (including the HWND target) before recreating.
            self.renderer = None;
            if self.recovered {
                self.fail(event_loop, error);
            } else {
                self.recovered = true;
                self.window.as_ref().unwrap().request_redraw();
            }
            return;
        }
        self.paints += 1;
        if self.started.elapsed() >= Duration::from_secs(2) {
            self.settled += 1;
        }
        let ms = start.elapsed().as_secs_f64() * 1000.;
        self.total_ms += ms;
        self.max_ms = self.max_ms.max(ms);
        if screenshot.is_some() {
            self.screenshot_saved = true;
            emit(
                "screenshot-saved",
                json!({"path":self.state.options.screenshot,"kind":"D2D surface readback, not final DWM composition"}),
            );
            event_loop.exit();
        }
    }
    fn exercise(&mut self) {
        let start = Instant::now();
        match self.state.options.scene {
            Scene::Preferences => self.action(Action::Appearance),
            Scene::History => {
                self.state.first_row = if self.cycle.is_multiple_of(2) {
                    self.state.options.history_count.saturating_sub(6)
                } else {
                    0
                };
                self.window.as_ref().unwrap().request_redraw();
            }
            Scene::Hud => self.action(Action::Pause),
            Scene::Preview => self.action(Action::Preview),
            Scene::Editor => {
                self.state.zoom = if self.cycle.is_multiple_of(2) {
                    1.5
                } else {
                    0.75
                };
                self.state.query = format!("Annotation {}", self.cycle + 1);
                self.window.as_ref().unwrap().request_redraw();
            }
            Scene::Idle | Scene::Countdown => unreachable!("options reject scripted actions"),
        }
        emit(
            "scripted-action",
            json!({"scene":self.state.options.scene.name(),"cycle":self.cycle,
            "milliseconds":start.elapsed().as_secs_f64()*1000.,"paused":self.state.paused,
            "appearance":self.state.options.appearance,"historyEnd":self.state.first_row>0,"zoom":self.state.zoom,
            "deleted":self.state.deleted,"compositorFade":self.state.options.floating && self.state.options.scene == Scene::Preview}),
        );
        self.cycle += 1;
    }
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let floating = self.state.options.floating;
        let size = if floating {
            LogicalSize::new(640., 620.)
        } else {
            LogicalSize::new(1000., 720.)
        };
        match event_loop.create_window(
            Window::default_attributes()
                .with_title("Captures — DirectComposition candidate")
                .with_inner_size(size)
                .with_min_inner_size(size)
                .with_transparent(true)
                .with_no_redirection_bitmap(true)
                .with_decorations(!floating)
                .with_visible(self.state.options.scene != Scene::Idle),
        ) {
            Ok(window) => {
                window.set_ime_allowed(true);
                self.window = Some(window);
            }
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        }
        if let Err(error) = self.renderer() {
            self.fail(event_loop, error);
            return;
        }
        emit(
            "ready",
            json!({"renderer":"DirectComposition/Direct2D","software":self.renderer.as_ref().unwrap().software,
            "scene":self.state.options.scene.name(),"readiness":"renderer initialized, not first presentation",
            "accessibility":"custom controls have no UI Automation provider yet",
            "input":"basic text and IME commit probe, not full editor/IME parity"}),
        );
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.paint(event_loop),
            WindowEvent::Resized(_) | WindowEvent::ThemeChanged(_) => {
                self.window.as_ref().unwrap().request_redraw()
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.renderer = None;
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.as_ref().unwrap().scale_factor();
                self.pointer = [(position.x / scale) as f32, (position.y / scale) as f32];
                if let Some(old) = self.drag {
                    for (index, value) in old.iter().enumerate() {
                        self.state.pan[index] += self.pointer[index] - value;
                    }
                    self.drag = Some(self.pointer);
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if state == ElementState::Released {
                    self.drag = None;
                    return;
                }
                let action = self
                    .state
                    .hits
                    .iter()
                    .find(|(rect, _)| rect.contains(self.pointer[0], self.pointer[1]))
                    .map(|(_, a)| *a);
                if let Some(action) = action {
                    self.action(action);
                } else if self.state.options.scene == Scene::Editor {
                    self.drag = Some(self.pointer);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 24.,
                };
                self.state.scroll(amount);
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::Tab) => self.action(Action::Search),
                    Key::Named(NamedKey::Backspace) if self.state.editing => {
                        self.state.query.pop();
                    }
                    Key::Named(NamedKey::Space) if self.state.options.scene == Scene::Preview => {
                        self.action(Action::Preview)
                    }
                    Key::Character(ref text) if self.state.editing => {
                        self.state.query.push_str(text)
                    }
                    _ => {}
                }
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) if self.state.editing => {
                self.state.query.push_str(&text);
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::Focused(false) => self.drag = None,
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(error) = self.failure.take() {
            self.fail(event_loop, error);
            return;
        }
        let elapsed = self.started.elapsed();
        if self.state.options.quit_after.is_some_and(|d| elapsed >= d) {
            if self.state.options.screenshot.is_some() && !self.screenshot_saved {
                self.fail(event_loop, "Screenshot missed quit deadline");
                return;
            }
            emit(
                "lifecycle-check",
                json!({"scene":self.state.options.scene.name(),"nativeVisible":self.window.as_ref().and_then(Window::is_visible)}),
            );
            event_loop.exit();
            return;
        }
        if self.state.options.exercise
            && self.cycle < 6
            && elapsed >= Duration::from_secs(2 + self.cycle as u64 * 4)
        {
            self.exercise();
        }
        if self.state.options.screenshot.is_some()
            && !self.screenshot_requested
            && elapsed >= self.state.options.screenshot_after
        {
            self.screenshot_requested = true;
            self.window.as_ref().unwrap().request_redraw();
        }
        let mut next = self.state.options.quit_after;
        if self.state.options.exercise && self.cycle < 6 {
            let action = Duration::from_secs(2 + self.cycle as u64 * 4);
            next = Some(next.map_or(action, |d| d.min(action)));
        }
        if self.state.options.screenshot.is_some() && !self.screenshot_requested {
            let shot = self.state.options.screenshot_after;
            next = Some(next.map_or(shot, |d| d.min(shot)));
        }
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, |deadline| {
            ControlFlow::WaitUntil(self.started + deadline)
        }));
    }
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.renderer = None;
        emit(
            "exit",
            json!({"elapsedSeconds":self.started.elapsed().as_secs_f64(),"uiPasses":self.paints,
            "uiPassesAfterTwoSeconds":self.settled,"totalUiConstructionWallMs":self.total_ms,
            "maxUiConstructionWallMs":self.max_ms,"scriptedActions":self.cycle,
            "note":"layout + D2D submission, not GPU presentation; excludes DWM"}),
        );
    }
}
