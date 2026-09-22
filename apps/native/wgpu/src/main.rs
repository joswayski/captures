mod capture_controls;
mod clipboard_input;
mod countdown;
mod editor;
mod feedback;
mod live;
mod mini_preview;
mod options;
mod preferences;
mod recording;
mod recording_hud;
mod recording_region;
mod recording_saved_notice;
mod selector;
mod shortcut_input;
mod tokens;
mod tray;
mod window_selector;
mod work_area;
mod workbench;

use eframe::egui;
use options::{Options, Scene};
use serde_json::json;
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{ModifiersState, PhysicalKey},
    window::WindowId,
};

fn emit(event: &str, detail: serde_json::Value) {
    println!(
        "{}",
        json!({"schema": 1, "pid": std::process::id(), "event": event, "detail": detail})
    );
}

struct InputApplication<'a> {
    inner: eframe::EframeWinitApplication<'a>,
    paste_input: clipboard_input::PasteInput,
    shortcut_input: shortcut_input::Bridge,
    shortcuts: workbench::ShortcutOwner,
    root_window: Option<WindowId>,
    root_focused: bool,
    modifiers: ModifiersState,
}

impl ApplicationHandler<eframe::UserEvent> for InputApplication<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.resumed(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let root_window = *self.root_window.get_or_insert(window_id);
        if window_id == root_window {
            match &event {
                WindowEvent::Focused(focused) => {
                    self.root_focused = *focused;
                    if !focused {
                        self.shortcut_input.blur();
                        self.shortcuts.resume_after_root_blur();
                    }
                }
                WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
                WindowEvent::KeyboardInput { event, .. }
                    if self.root_focused && self.shortcut_input.is_active() =>
                {
                    let code = match event.physical_key {
                        PhysicalKey::Code(code) => shortcut_input::physical_code(code),
                        PhysicalKey::Unidentified(_) => "Unidentified".into(),
                    };
                    self.shortcut_input.key(
                        code,
                        event.state,
                        event.repeat,
                        shortcut_input::Modifiers {
                            ctrl: self.modifiers.control_key(),
                            shift: self.modifiers.shift_key(),
                            alt: self.modifiers.alt_key(),
                            meta: self.modifiers.super_key(),
                        },
                    );
                }
                _ => {}
            }
        }
        self.paste_input.begin_event(window_id, &event);
        self.inner.window_event(event_loop, window_id, event);
        self.paste_input.end_event();
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.inner.new_events(event_loop, cause);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: eframe::UserEvent) {
        self.inner.user_event(event_loop, event);
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: DeviceId,
        event: DeviceEvent,
    ) {
        self.inner.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.memory_warning(event_loop);
    }
}

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--font-license") {
        print!("{}", captures_app::editor_fonts::NOTICE);
        return Ok(());
    }
    let options = Options::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}\n{}", options::USAGE);
        std::process::exit(2);
    });
    let floating = options.floating;
    let idle = options.scene == Scene::Idle;
    let size = if floating && options.scene == Scene::Hud {
        [430., 102.]
    } else if floating {
        [640., 620.]
    } else if options.scene == Scene::CaptureControls && options.capture_controls_recording {
        [1280., 900.]
    } else {
        [1000., 720.]
    };
    let minimum_size = if options.scene == Scene::CaptureControls {
        [640., 480.]
    } else {
        size
    };
    let native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title(if options.live {
                "Captures"
            } else {
                "Captures — wgpu fixture workbench"
            })
            .with_inner_size(size)
            .with_min_inner_size(minimum_size)
            .with_visible(!idle)
            // eframe's wgpu painter takes its alpha capability from the root,
            // including for the transparent countdown child viewport.
            .with_transparent(floating || options.live)
            .with_decorations(!floating),
        ..Default::default()
    };
    emit(
        "starting",
        json!({"scene": if options.live { "live" } else { options.scene.name() },
        "phase": "before native event loop and renderer initialization"}),
    );
    let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    let shortcut_input = shortcut_input::Bridge::default();
    let workbench_input = shortcut_input.clone();
    let paste_input = clipboard_input::PasteInput::default();
    let workbench_paste_input = paste_input.clone();
    let shortcuts = workbench::ShortcutOwner::default();
    let workbench_shortcuts = shortcuts.clone();
    let inner = eframe::create_native(
        "Captures renderer experiment",
        native,
        Box::new(move |cc| {
            Ok(Box::new(workbench::Workbench::new(
                cc,
                options,
                workbench_input,
                workbench_shortcuts,
                workbench_paste_input,
            )))
        }),
        &event_loop,
    );
    let mut application = InputApplication {
        inner,
        paste_input,
        shortcut_input,
        shortcuts,
        root_window: None,
        root_focused: false,
        modifiers: ModifiersState::default(),
    };
    event_loop.run_app(&mut application)?;
    Ok(())
}
