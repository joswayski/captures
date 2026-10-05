use std::{
    io::{self, BufRead},
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
    Visibility,
}

#[derive(Debug)]
enum UserEvent {
    Command(String),
}

struct App {
    mode: Mode,
    windows: Vec<Arc<Window>>,
    surfaces: Vec<Surface<Arc<Window>, Arc<Window>>>,
    dragging: Arc<AtomicBool>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let titles: &[&str] = match self.mode {
            Mode::Source(_) => &["drag-source", "drag-own-receiver"],
            Mode::Receiver => &["drag-receiver"],
            Mode::Visibility => &["visibility-primary", "visibility-initially-hidden"],
        };
        for (index, title) in titles.iter().enumerate() {
            let initially_visible = !matches!(self.mode, Mode::Visibility) || index == 0;
            let window = Arc::new(
                event_loop
                    .create_window(
                        WindowAttributes::default()
                            .with_title(*title)
                            .with_inner_size(if matches!(self.mode, Mode::Visibility) {
                                winit::dpi::LogicalSize::new(80, 60)
                            } else {
                                winit::dpi::LogicalSize::new(300, 220)
                            })
                            .with_resizable(!matches!(self.mode, Mode::Visibility))
                            .with_visible(initially_visible),
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
            if initially_visible {
                let mut buffer = surface.buffer_mut().unwrap();
                buffer.fill(if matches!(self.mode, Mode::Source(_)) {
                    0xff224488
                } else if matches!(self.mode, Mode::Visibility) {
                    0xffcc3311
                } else {
                    0xff228844
                });
                buffer.present().unwrap();
            }
            self.windows.push(window);
            self.surfaces.push(surface);
            println!("READY {title}");
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Command(command) = event;
        let mut words = command.split_whitespace();
        match (words.next(), words.next()) {
            (Some("hide"), Some(index)) => {
                self.windows[index.parse::<usize>().unwrap()].set_visible(false)
            }
            (Some("show"), Some(index)) => {
                self.windows[index.parse::<usize>().unwrap()].set_visible(true)
            }
            (Some("redraw"), Some(index)) => {
                self.windows[index.parse::<usize>().unwrap()].request_redraw()
            }
            (Some("status"), Some(index)) => {
                let index = index.parse::<usize>().unwrap();
                println!("STATUS {index} {:?}", self.windows[index].is_visible());
            }
            (Some("quit"), _) => event_loop.exit(),
            _ => panic!("bad visibility command: {command}"),
        }
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
                && self.windows.first().is_some_and(|window| window.id() == id)
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
                let target = if self.windows.first().is_some_and(|window| window.id() == id) {
                    "source"
                } else {
                    "own-receiver"
                };
                println!(
                    "DROPPED target={target} path={} bytes={}",
                    path.display(),
                    hex(&bytes)
                );
            }
            WindowEvent::RedrawRequested if matches!(self.mode, Mode::Visibility) => {
                let index = self
                    .windows
                    .iter()
                    .position(|window| window.id() == id)
                    .unwrap();
                let size = self.windows[index].inner_size();
                self.surfaces[index]
                    .resize(
                        NonZeroU32::new(size.width).unwrap(),
                        NonZeroU32::new(size.height).unwrap(),
                    )
                    .unwrap();
                let mut buffer = self.surfaces[index].buffer_mut().unwrap();
                buffer.fill(if index == 0 { 0xffcc3311 } else { 0xff22aa55 });
                buffer.present().unwrap();
                println!("REDRAW {index}");
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
        inject(args.next().as_deref().unwrap_or("reject"), None);
        return;
    }
    if command.as_deref() == Some("pointer") {
        inject("pointer", None);
        return;
    }
    if command.as_deref() == Some("click") {
        let coordinates = std::array::from_fn(|_| {
            args.next()
                .expect("x y width height")
                .parse::<u32>()
                .expect("coordinate")
        });
        inject("click", Some(coordinates));
        return;
    }
    let mode = match command.as_deref() {
        Some("source") => Mode::Source(PathBuf::from(args.next().expect("source path"))),
        Some("receiver") => Mode::Receiver,
        Some("visibility") => Mode::Visibility,
        _ => panic!("usage: probe source PATH | receiver"),
    };
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    builder.with_wayland();
    let event_loop = builder.build().unwrap();
    if matches!(mode, Mode::Visibility) {
        let proxy = event_loop.create_proxy();
        thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                if proxy.send_event(UserEvent::Command(line.unwrap())).is_err() {
                    break;
                }
            }
        });
    }
    let mut app = App {
        mode,
        windows: Vec::new(),
        surfaces: Vec::new(),
        dragging: Arc::new(AtomicBool::new(false)),
    };
    event_loop.run_app(&mut app).unwrap();
}

fn inject(destination: &str, click: Option<[u32; 4]>) {
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
    queue.roundtrip(&mut State).unwrap();
    let clock = std::time::Instant::now();
    let timestamp = || clock.elapsed().as_millis() as u32;
    let mut perform = |click: Option<[u32; 4]>| {
        let [x, y, width, height] = click.unwrap_or([180, 150, 900, 500]);
        pointer.motion_absolute(timestamp(), x, y, width, height);
        pointer.frame();
        queue.roundtrip(&mut State).unwrap();
        thread::sleep(Duration::from_millis(150));
        pointer.button(timestamp(), 0x110, ButtonState::Pressed);
        pointer.frame();
        queue.roundtrip(&mut State).unwrap();
        thread::sleep(Duration::from_millis(150));
        if click.is_none() {
            let (x, y) = match destination {
                "accept" => (570, 150),
                "self" => (210, 170),
                _ => (850, 450),
            };
            pointer.motion_absolute(timestamp(), x, y, 900, 500);
            pointer.frame();
            queue.roundtrip(&mut State).unwrap();
            thread::sleep(Duration::from_millis(150));
        }
        pointer.button(timestamp(), 0x110, ButtonState::Released);
        pointer.frame();
        queue.roundtrip(&mut State).unwrap();
        thread::sleep(Duration::from_millis(150));
    };
    if destination == "pointer" {
        // Keep a pointer device present through the entire private desktop test.
        // Reconnecting per click exposes old wlroots' inert-relative-pointer bug.
        println!("READY");
        for line in io::stdin().lock().lines() {
            let line = line.unwrap();
            let mut values = line.split_whitespace();
            let coordinates = std::array::from_fn(|_| {
                values
                    .next()
                    .expect("x y width height")
                    .parse::<u32>()
                    .expect("coordinate")
            });
            perform(Some(coordinates));
            println!("CLICKED");
        }
    } else {
        perform(click);
    }
}
