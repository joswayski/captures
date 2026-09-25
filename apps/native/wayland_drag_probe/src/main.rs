use std::{
    num::NonZeroU32,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use softbuffer::{Context, Surface};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    platform::wayland::{ActiveEventLoopExtWayland, EventLoopBuilderExtWayland},
    window::{Window, WindowAttributes, WindowId},
};

enum Mode {
    Source(PathBuf),
    Receiver,
}

struct App {
    mode: Mode,
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    dragging: Arc<AtomicBool>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let title = match self.mode {
            Mode::Source(_) => "drag-source",
            Mode::Receiver => "drag-receiver",
        };
        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title(title)
                        .with_inner_size(winit::dpi::LogicalSize::new(300, 220)),
                )
                .unwrap(),
        );
        let context = Context::new(window.clone()).unwrap();
        let mut surface = Surface::new(&context, window.clone()).unwrap();
        let size = window.inner_size();
        surface
            .resize(
                NonZeroU32::new(size.width).unwrap(),
                NonZeroU32::new(size.height).unwrap(),
            )
            .unwrap();
        let mut buffer = surface.buffer_mut().unwrap();
        buffer.fill(if matches!(self.mode, Mode::Source(_)) {
            0xff224488
        } else {
            0xff228844
        });
        buffer.present().unwrap();
        self.surface = Some(surface);
        self.window = Some(window);
        println!("READY {title}");
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(
            event,
            WindowEvent::CursorEntered { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
        ) {
            println!("INPUT {event:?}");
        }
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if matches!(self.mode, Mode::Source(_))
                && !self.dragging.swap(true, Ordering::SeqCst) =>
            {
                let Mode::Source(path) = &self.mode else {
                    unreachable!()
                };
                let dragging = self.dragging.clone();
                let callback = Box::new(move |accepted, own, same_source| {
                    if own {
                        println!(
                            "FINISHED accepted={accepted} own={own} same_source={same_source}"
                        );
                    } else {
                        println!("FINISHED accepted={accepted} own={own}");
                    }
                    dragging.store(false, Ordering::SeqCst);
                });
                match event_loop.start_file_drag(id, path.clone(), callback) {
                    Ok(()) => println!("STARTED"),
                    Err(error) => {
                        self.dragging.store(false, Ordering::SeqCst);
                        println!("START_ERROR {error}");
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::HoveredFile(path) => println!("HOVER {}", path.display()),
            WindowEvent::HoveredFileCancelled => println!("HOVER_CANCELLED"),
            WindowEvent::DroppedFile(path) => {
                let bytes = std::fs::read(&path).unwrap();
                println!("DROPPED path={} bytes={}", path.display(), hex(&bytes));
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    if command.as_deref() == Some("inject") {
        inject(args.next().as_deref().unwrap_or("reject"));
        return;
    }
    let mode = match command.as_deref() {
        Some("source") => Mode::Source(PathBuf::from(args.next().expect("source path"))),
        Some("receiver") => Mode::Receiver,
        _ => panic!("usage: probe source PATH | receiver"),
    };
    let mut builder = EventLoop::builder();
    builder.with_wayland();
    let event_loop = builder.build().unwrap();
    let mut app = App {
        mode,
        window: None,
        surface: None,
        dragging: Arc::new(AtomicBool::new(false)),
    };
    event_loop.run_app(&mut app).unwrap();
}

fn inject(destination: &str) {
    use wayland_client::{
        globals::{registry_queue_init, GlobalListContents},
        protocol::{wl_pointer::ButtonState, wl_registry},
        Connection, Dispatch, QueueHandle,
    };
    use wayland_protocols_wlr::virtual_pointer::v1::client::{
        zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
        zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
    };
    struct State;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &wl_registry::WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZwlrVirtualPointerManagerV1,
            _: <ZwlrVirtualPointerManagerV1 as wayland_client::Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZwlrVirtualPointerV1,
            _: <ZwlrVirtualPointerV1 as wayland_client::Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    let connection = Connection::connect_to_env().unwrap();
    let (globals, mut queue) = registry_queue_init::<State>(&connection).unwrap();
    let qh = queue.handle();
    let manager = globals
        .bind::<ZwlrVirtualPointerManagerV1, _, _>(&qh, 1..=2, ())
        .unwrap();
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    pointer.motion_absolute(1, 180, 150, 900, 500);
    pointer.frame();
    queue.roundtrip(&mut State).unwrap();
    thread::sleep(Duration::from_millis(150));
    pointer.button(2, 0x110, ButtonState::Pressed);
    pointer.frame();
    queue.roundtrip(&mut State).unwrap();
    thread::sleep(Duration::from_millis(150));
    let (x, y) = match destination {
        "accept" => (570, 150),
        "self" => (210, 170),
        _ => (850, 450),
    };
    pointer.motion_absolute(3, x, y, 900, 500);
    pointer.frame();
    queue.roundtrip(&mut State).unwrap();
    thread::sleep(Duration::from_millis(150));
    pointer.button(4, 0x110, ButtonState::Released);
    pointer.frame();
    queue.roundtrip(&mut State).unwrap();
    thread::sleep(Duration::from_millis(150));
}
