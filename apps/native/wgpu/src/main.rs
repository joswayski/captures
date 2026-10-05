mod accessibility;
mod capture_controls;
mod capture_error;
mod clipboard_input;
mod clipboard_revision;
mod compare_overlay;
mod countdown;
mod diagnostics;
mod editor;
mod effects;
mod feedback;
mod glass_tooltip;
mod history;
mod live;
mod media_tools;
mod mini_preview;
mod motion;
mod onboarding;
mod options;
mod outbound_drag;
mod preferences;
mod preferences_widgets;
mod preferences_window;
mod primitives;
mod recording;
mod recording_editor;
mod recording_hud;
mod recording_recovery;
mod recording_region;
mod recording_saved_notice;
mod reveal;
#[cfg(any(target_os = "windows", target_os = "linux", test))]
mod root_repaint;
mod selector;
mod sharing;
mod shortcut_input;
mod startup_notice;
mod tokens;
mod tray;
mod ui_fonts;
mod update_notice;
mod window_selector;
#[cfg(target_os = "windows")]
mod windows_drag;
mod work_area;
mod workbench;

use eframe::egui;
use options::{Options, Scene};
use serde_json::json;
use std::{cell::RefCell, rc::Rc, sync::Weak, time::Instant};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{ModifiersState, PhysicalKey},
    window::WindowId,
};

#[derive(Default)]
struct RootState {
    egui_ctx: Option<egui::Context>,
    root_window_id: Option<WindowId>,
    root_window: Option<Weak<winit::window::Window>>,
}

fn emit(event: &str, detail: serde_json::Value) {
    println!(
        "{}",
        json!({"schema": 1, "pid": std::process::id(), "event": event, "detail": detail})
    );
}

struct InputApplication<'a> {
    inner: eframe::EframeWinitApplication<'a>,
    outbound_drag: outbound_drag::Bridge,
    paste_input: clipboard_input::PasteInput,
    shortcut_input: shortcut_input::Bridge,
    shortcuts: workbench::ShortcutOwner,
    root_state: Option<Rc<RefCell<RootState>>>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    root_repaints: root_repaint::Pending,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    root_suspended: bool,
    /// The Captures window that owns keyboard focus. Shortcut recording
    /// happens in the Preferences window, which is not the root.
    focused_window: Option<WindowId>,
    root_id: Rc<std::cell::Cell<Option<WindowId>>>,
    modifiers: ModifiersState,
}

impl ApplicationHandler<eframe::UserEvent> for InputApplication<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            self.root_suspended = false;
        }
        self.inner.resumed(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let _span = diagnostics::span("window-event");
        diagnostics::event("window-event-kind", || {
            let (kind, value) = match &event {
                WindowEvent::RedrawRequested => ("RedrawRequested", json!(null)),
                WindowEvent::Resized(size) => {
                    ("Resized", json!({"width":size.width,"height":size.height}))
                }
                WindowEvent::Focused(focused) => ("Focused", json!(focused)),
                WindowEvent::Occluded(occluded) => ("Occluded", json!(occluded)),
                WindowEvent::Destroyed => ("Destroyed", json!(null)),
                _ => ("Other", json!(null)),
            };
            json!({"window":format!("{window_id:?}"),"kind":kind,"value":value})
        });
        match &event {
            WindowEvent::Focused(true) => {
                // Focus can arrive before the previous window's blur.
                if self
                    .focused_window
                    .is_some_and(|focused| focused != window_id)
                {
                    self.shortcut_input.blur();
                    self.shortcuts.resume_after_root_blur();
                }
                self.focused_window = Some(window_id);
                preferences_window::set_native_focus(true);
            }
            WindowEvent::Focused(false) | WindowEvent::Destroyed
                if self.focused_window == Some(window_id) =>
            {
                self.focused_window = None;
                preferences_window::set_native_focus(false);
                self.shortcut_input.blur();
                self.shortcuts.resume_after_root_blur();
            }
            WindowEvent::ModifiersChanged(modifiers) if self.focused_window == Some(window_id) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. }
                if self.focused_window == Some(window_id) && self.shortcut_input.is_active() =>
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
        self.outbound_drag.begin_event(window_id, &event);
        self.paste_input.begin_event(window_id, &event);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if matches!(event, WindowEvent::Destroyed)
            && let Some(state) = &self.root_state
            && state.borrow().root_window_id == Some(window_id)
        {
            state.borrow_mut().root_window = None;
            self.root_repaints.clear();
        }
        preferences_window::during_window_event(|| {
            self.inner.window_event(event_loop, window_id, event);
        });
        self.paste_input.end_event();
        self.outbound_drag.end_event();
        self.outbound_drag.service(event_loop);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.service_root_repaint(event_loop);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        let _span = diagnostics::span("new-events");
        self.inner.new_events(event_loop, cause);
        self.dispatch_requested_root_pass(event_loop);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.service_root_repaint(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: eframe::UserEvent) {
        let _span = diagnostics::span("user-event");
        let root_repaint = match &event {
            eframe::UserEvent::RequestRepaint {
                when,
                cumulative_pass_nr,
                viewport_id: egui::ViewportId::ROOT,
            } => Some((*when, *cumulative_pass_nr)),
            _ => None,
        };
        if let eframe::UserEvent::RequestRepaint {
            when,
            cumulative_pass_nr,
            viewport_id,
        } = &event
            && *viewport_id == egui::ViewportId::ROOT
        {
            self.trace_root_repaint("before", *when, *cumulative_pass_nr, event_loop);
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if let Some((when, requested_pass)) = root_repaint
            && let Some((ctx, _)) = self.repaint_root(event_loop)
        {
            self.root_repaints.request(
                ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT),
                requested_pass,
                when,
            );
        }
        self.inner.user_event(event_loop, event);
        self.dispatch_requested_root_pass(event_loop);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.service_root_repaint(event_loop);
        if let Some((when, cumulative_pass_nr)) = root_repaint {
            self.trace_root_repaint("after", when, cumulative_pass_nr, event_loop);
        }
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
        let _span = diagnostics::span("about-to-wait");
        self.inner.about_to_wait(event_loop);
        self.dispatch_requested_root_pass(event_loop);
        self.outbound_drag.service(event_loop);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            self.service_root_repaint(event_loop);
            if !event_loop.exiting()
                && (cfg!(target_os = "windows")
                    || self
                        .repaint_root(event_loop)
                        .is_some_and(|(_, window)| window.is_visible() == Some(false)))
                && let Some(deadline) = self.root_repaints.next_deadline()
            {
                use winit::event_loop::ControlFlow;
                event_loop.set_control_flow(match event_loop.control_flow() {
                    ControlFlow::Wait => ControlFlow::WaitUntil(deadline),
                    ControlFlow::WaitUntil(inner) => ControlFlow::WaitUntil(inner.min(deadline)),
                    ControlFlow::Poll => ControlFlow::Poll,
                });
            }
        }
        diagnostics::event(
            "control-flow",
            || json!({"flow":format!("{:?}",event_loop.control_flow())}),
        );
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            self.root_suspended = true;
            self.root_repaints.clear();
        }
        self.inner.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.root_repaints.clear();
        self.inner.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.memory_warning(event_loop);
    }
}

impl InputApplication<'_> {
    /// eframe repaints a hidden root outside window events, where it cannot
    /// create the immediate Preferences window. Dispatch that root pass as a
    /// redraw event instead when the window asks for one.
    fn dispatch_requested_root_pass(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() || !preferences_window::take_root_pass_request() {
            return;
        }
        let Some(root) = self.root_id.get() else {
            return;
        };
        diagnostics::event("preferences-root-pass", || json!({}));
        self.paste_input
            .begin_event(root, &WindowEvent::RedrawRequested);
        preferences_window::during_window_event(|| {
            self.inner
                .window_event(event_loop, root, WindowEvent::RedrawRequested);
        });
        self.paste_input.end_event();
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn repaint_root(
        &self,
        event_loop: &ActiveEventLoop,
    ) -> Option<(egui::Context, std::sync::Arc<winit::window::Window>)> {
        if self.root_suspended || event_loop.exiting() {
            return None;
        }
        let state = self.root_state.as_ref()?.borrow();
        let window = state.root_window.as_ref()?.upgrade()?;
        let ctx = state.egui_ctx.clone()?;
        // Hidden/minimized roots already use eframe's throttled direct dispatch.
        #[cfg(target_os = "windows")]
        if window.is_visible() != Some(true) || window.is_minimized() == Some(true) {
            return None;
        }
        #[cfg(target_os = "linux")]
        if ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) != Some(true)
        {
            return None;
        }
        Some((ctx, window))
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn service_root_repaint(&mut self, event_loop: &ActiveEventLoop) {
        let Some((ctx, window)) = self.repaint_root(event_loop) else {
            self.root_repaints.clear();
            return;
        };
        #[cfg(target_os = "linux")]
        if window.is_visible() != Some(false) {
            // Retain accepted requests across the asynchronous unmap ack. Winit
            // suppresses redraws in that gap; dispatch only after confirmed hide.
            self.root_repaints
                .prune(ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT));
            return;
        }
        if self.root_repaints.take_due(
            ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT),
            Instant::now(),
        ) {
            // winit 0.30.13 can repeatedly deliver a continuously animating
            // child's WM_PAINT while starving a visible ROOT. eframe 0.36.2
            // already runs this same renderer synchronously on Windows resize.
            // Dispatch one accepted, due ROOT pass after the inner handler has
            // returned; never recurse or touch native WM_PAINT bookkeeping.
            // Wayland's acknowledged hidden root takes eframe's logic-only path:
            // no buffer is presented, and no portal-excluded window is remapped.
            diagnostics::event(
                "root-repaint-fallback",
                || json!({"pass":ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT)}),
            );
            self.paste_input
                .begin_event(window.id(), &WindowEvent::RedrawRequested);
            preferences_window::during_window_event(|| {
                self.inner
                    .window_event(event_loop, window.id(), WindowEvent::RedrawRequested);
            });
            self.paste_input.end_event();
            self.root_repaints
                .prune(ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT));
        }
    }

    fn trace_root_repaint(
        &self,
        phase: &'static str,
        when: Instant,
        requested_pass: u64,
        event_loop: &ActiveEventLoop,
    ) {
        let Some(state) = &self.root_state else {
            return;
        };
        diagnostics::event("root-repaint-event", || {
            let state = state.borrow();
            let window = state.root_window.as_ref().and_then(Weak::upgrade);
            let now = Instant::now();
            json!({
                "phase":phase,
                "requestedRootPass":requested_pass,
                "currentRootPass":state.egui_ctx.as_ref().map(|ctx| ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT)),
                "due":when <= now,
                "overdueUs":now.checked_duration_since(when).map(|duration| duration.as_micros()),
                "rootWindowId":state.root_window_id.map(|id| format!("{id:?}")),
                "weakUpgrade":window.is_some(),
                "nativeVisible":window.as_ref().and_then(|window| window.is_visible()),
                "nativeMinimized":window.as_ref().and_then(|window| window.is_minimized()),
                "rootRepaintRequested":state.egui_ctx.as_ref().map(|ctx| ctx.has_requested_repaint_for(&egui::ViewportId::ROOT)),
                "controlFlow":format!("{:?}", event_loop.control_flow()),
            })
        });
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
    // Election precedes the renderer, capture workers, tray and global keys.
    // Fixtures never acquire ownership of a live development profile.
    let instance = if options.live {
        use captures_app::instance::{Instance, Launch, OpenRequest};
        let result = OpenRequest::from_paths(options.open_media.clone()).and_then(|request| {
            Instance::start(
                &options
                    .history_root
                    .clone()
                    .unwrap_or_else(captures_app::default_history_root),
                request,
            )
        });
        match result {
            Ok(Launch::Primary(instance)) => Some(instance),
            Ok(Launch::Forwarded) => {
                emit("forwarded", json!({"paths": options.open_media.len()}));
                return Ok(());
            }
            Err(error) => {
                eprintln!("Captures could not start: {error}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    // Forwarded secondaries returned above; fixture scenes never touch markers.
    let crash = instance.as_ref().and_then(|_| {
        match captures_app::crash::Session::start(
            &options
                .history_root
                .clone()
                .unwrap_or_else(captures_app::default_history_root),
        ) {
            Ok(session) => Some(std::sync::Arc::new(session)),
            Err(error) => {
                eprintln!("{error} Capture remains available.");
                None
            }
        }
    });
    let floating = options.floating;
    let idle = options.scene == Scene::Idle;
    let size = if options.live {
        captures_app::app_windows::HISTORY.size()
    } else if floating && options.scene == Scene::Hud {
        [430., 102.]
    } else if floating {
        [640., 620.]
    } else if options.scene == Scene::Sharing {
        [480., 720.]
    } else if options.scene == Scene::CaptureControls && options.capture_controls_recording {
        [1280., 900.]
    } else {
        [1000., 720.]
    };
    let minimum_size = if options.live {
        captures_app::app_windows::HISTORY.min_size()
    } else if options.scene == Scene::Sharing {
        [380., 520.]
    } else if options.scene == Scene::CaptureControls {
        [640., 480.]
    } else {
        size
    };
    let native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            // The live root is the Capture History window; first-run setup
            // retitles and resizes it until setup completes (workbench.rs).
            .with_title(if options.live {
                captures_app::app_windows::HISTORY.title
            } else {
                "Captures — wgpu fixture workbench"
            })
            .with_inner_size(size)
            .with_min_inner_size(minimum_size)
            .with_resizable(true)
            // A live launch decides which window to show once settings load
            // (`app_windows::interactive_launch`); History stays hidden
            // unless setup or --open-history needs it.
            .with_visible(!idle && !options.live)
            .with_active(!idle && !options.live)
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
    diagnostics::install_eframe_logger();
    let native_state = Rc::new(RefCell::new(RootState::default()));
    let create_native_state = native_state.clone();
    let root_id = Rc::new(std::cell::Cell::new(None));
    let create_root_id = root_id.clone();
    let outbound_drag = outbound_drag::Bridge::default();
    let create_drag = outbound_drag.clone();
    let inner = eframe::create_native(
        "Captures renderer experiment",
        native,
        Box::new(move |cc| {
            create_drag.install(&cc.egui_ctx);
            let window = cc
                .winit_window()
                .expect("native creation context has a root window");
            #[cfg(target_os = "windows")]
            create_drag.set_window(window);
            create_root_id.set(Some(window.id()));
            *create_native_state.borrow_mut() = RootState {
                egui_ctx: Some(cc.egui_ctx.clone()),
                root_window_id: Some(window.id()),
                root_window: Some(std::sync::Arc::downgrade(window)),
            };
            Ok(Box::new(workbench::Workbench::new(
                cc,
                options,
                workbench_input,
                workbench_shortcuts,
                workbench_paste_input,
                instance,
                crash,
            )))
        }),
        &event_loop,
    );
    let root_state = (cfg!(any(target_os = "windows", target_os = "linux"))
        || diagnostics::is_enabled())
    .then_some(native_state);
    let mut application = InputApplication {
        inner,
        outbound_drag,
        paste_input,
        shortcut_input,
        shortcuts,
        root_state,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        root_repaints: root_repaint::Pending::default(),
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        root_suspended: false,
        focused_window: None,
        root_id,
        modifiers: ModifiersState::default(),
    };
    event_loop.run_app(&mut application)?;
    Ok(())
}
