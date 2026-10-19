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
    hide_after_paint: Option<usize>,
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
            (Some("present-hide"), Some(index)) => {
                let index = index.parse::<usize>().unwrap();
                self.hide_after_paint = Some(index);
                self.windows[index].request_redraw();
            }
            (Some("show-hide"), Some(index)) => {
                let index = index.parse::<usize>().unwrap();
                self.windows[index].set_visible(true);
                self.windows[index].set_visible(false);
                println!("SHOW-HIDE {index}");
            }
            (Some("hide-show"), Some(index)) => {
                let index = index.parse::<usize>().unwrap();
                self.windows[index].set_visible(false);
                self.windows[index].set_visible(true);
                println!("HIDE-SHOW {index}");
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
                if self.hide_after_paint == Some(index) {
                    self.hide_after_paint = None;
                    self.windows[index].set_visible(false);
                    use winit::platform::wayland::WindowExtWayland;
                    assert_eq!(self.windows[index].is_surface_ready(), Some(false));
                    println!("PRESENT-HIDE {index}");
                }
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
    if command.as_deref() == Some("clipboard-replace") {
        clipboard_replace(
            PathBuf::from(args.next().expect("first PNG")),
            PathBuf::from(args.next().expect("replacement PNG")),
        );
        return;
    }
    if command.as_deref() == Some("keyboard") {
        keyboard();
        return;
    }
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
        hide_after_paint: None,
    };
    event_loop.run_app(&mut app).unwrap();
}

/// Replace the selection after receiving its URI-list request. The old offer
/// still has a PNG: a receiver must not read that fallback from the new owner.
fn clipboard_replace(first: PathBuf, replacement: PathBuf) {
    use std::io::Write;
    use wayland_client::{
        delegate_noop,
        globals::{registry_queue_init, GlobalListContents},
        protocol::{wl_callback, wl_registry, wl_seat::WlSeat},
        Connection, Dispatch, QueueHandle,
    };
    use wayland_protocols_wlr::data_control::v1::client::{
        zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
        zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
        zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
        zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
    };
    struct State {
        device: ZwlrDataControlDeviceV1,
        replacement: ZwlrDataControlSourceV1,
        pixels: [Vec<u8>; 2],
    }
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
    delegate_noop!(State: ignore WlSeat);
    delegate_noop!(State: ignore ZwlrDataControlManagerV1);
    delegate_noop!(State: ignore ZwlrDataControlOfferV1);
    impl Dispatch<wl_callback::WlCallback, std::sync::Mutex<Option<std::fs::File>>> for State {
        fn event(
            _: &mut Self,
            _: &wl_callback::WlCallback,
            _: wl_callback::Event,
            pipe: &std::sync::Mutex<Option<std::fs::File>>,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            // The replacement is processed before we finish the old URI list.
            pipe.lock()
                .unwrap()
                .take()
                .unwrap()
                .write_all(b"# no local files\r\n")
                .unwrap();
            println!("REPLACED");
            std::io::stdout().flush().unwrap();
        }
    }
    impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZwlrDataControlDeviceV1,
            _: wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
        wayland_client::event_created_child!(State, ZwlrDataControlDeviceV1, [
            0 => (ZwlrDataControlOfferV1, ())
        ]);
    }
    impl Dispatch<ZwlrDataControlSourceV1, usize> for State {
        fn event(
            state: &mut Self,
            _: &ZwlrDataControlSourceV1,
            event: zwlr_data_control_source_v1::Event,
            index: &usize,
            connection: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let zwlr_data_control_source_v1::Event::Send { mime_type, fd } = event {
                let mut pipe = std::fs::File::from(fd);
                if mime_type == "text/uri-list" {
                    state.device.set_selection(Some(&state.replacement));
                    // Keep the old transfer open until the compositor has
                    // processed the replacement (the receiver is still busy).
                    connection
                        .display()
                        .sync(qh, std::sync::Mutex::new(Some(pipe)));
                } else if mime_type == "image/png" {
                    pipe.write_all(&state.pixels[*index]).unwrap();
                }
                println!("SENT {index} {mime_type}");
                std::io::stdout().flush().unwrap();
            }
        }
    }
    let connection = Connection::connect_to_env().unwrap();
    let (globals, mut queue) = registry_queue_init::<State>(&connection).unwrap();
    let qh = queue.handle();
    let seat = globals.bind::<WlSeat, _, _>(&qh, 1..=1, ()).unwrap();
    let manager = globals
        .bind::<ZwlrDataControlManagerV1, _, _>(&qh, 1..=2, ())
        .unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, 0);
    // egui-winit eagerly requests text before the editor receives Ctrl+V.
    // Serve empty plain text so that read cannot trigger our URI replacement.
    source.offer("text/plain;charset=utf-8".into());
    source.offer("text/uri-list".into());
    source.offer("image/png".into());
    let replacement_source = manager.create_data_source(&qh, 1);
    replacement_source.offer("image/png".into());
    device.set_selection(Some(&source));
    let mut state = State {
        device,
        replacement: replacement_source,
        pixels: [
            std::fs::read(first).unwrap(),
            std::fs::read(replacement).unwrap(),
        ],
    };
    queue.roundtrip(&mut state).unwrap();
    println!("READY");
    std::io::stdout().flush().unwrap();
    loop {
        queue.blocking_dispatch(&mut state).unwrap();
    }
}

fn inject(destination: &str, click: Option<[u32; 4]>) {
    use wayland_client::{
        globals::{registry_queue_init, GlobalListContents},
        protocol::{
            wl_pointer::{Axis, ButtonState},
            wl_registry,
        },
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
    let mut perform = |click: Option<[u32; 4]>, scroll: Option<f64>| {
        let start_x = if destination == "reverse" { 570 } else { 180 };
        let [x, y, width, height] = click.unwrap_or([start_x, 150, 900, 500]);
        pointer.motion_absolute(timestamp(), x, y, width, height);
        pointer.frame();
        queue.roundtrip(&mut State).unwrap();
        thread::sleep(Duration::from_millis(150));
        if let Some(distance) = scroll {
            pointer.axis(timestamp(), Axis::VerticalScroll, distance);
            pointer.frame();
            queue.roundtrip(&mut State).unwrap();
            thread::sleep(Duration::from_millis(150));
            return;
        }
        pointer.button(timestamp(), 0x110, ButtonState::Pressed);
        pointer.frame();
        queue.roundtrip(&mut State).unwrap();
        thread::sleep(Duration::from_millis(150));
        if click.is_none() {
            // Cross the drag threshold while still inside the source. GTK
            // needs to start its Wayland grab before the pointer leaves it.
            let threshold_x = if destination == "reverse" { 540 } else { 210 };
            pointer.motion_absolute(timestamp(), threshold_x, 150, 900, 500);
            pointer.frame();
            queue.roundtrip(&mut State).unwrap();
            thread::sleep(Duration::from_millis(150));
            let (x, y) = match destination {
                "accept" | "cancel-hover" => (570, 150),
                "reverse" => (180, 150),
                "center" => (350, 250),
                "self" => (210, 170),
                _ => (850, 450),
            };
            pointer.motion_absolute(timestamp(), x, y, 900, 500);
            pointer.frame();
            queue.roundtrip(&mut State).unwrap();
            thread::sleep(Duration::from_millis(150));
            if destination == "cancel-hover" {
                pointer.motion_absolute(timestamp(), 850, 450, 900, 500);
                pointer.frame();
                queue.roundtrip(&mut State).unwrap();
                thread::sleep(Duration::from_millis(150));
            }
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
            let scroll = values
                .next()
                .map(|value| value.parse().expect("scroll distance"));
            perform(Some(coordinates), scroll);
            println!(
                "{}",
                if scroll.is_some() {
                    "SCROLLED"
                } else {
                    "CLICKED"
                }
            );
        }
    } else {
        perform(click, None);
    }
}

/// Persistent keyboard on the private compositor. Each stdin line is an evdev
/// keycode and `none` or a `+`-separated set of shift/ctrl/alt modifiers.
fn keyboard() {
    use std::{io::Write, os::fd::AsFd};
    use wayland_client::{
        delegate_noop,
        globals::{registry_queue_init, GlobalListContents},
        protocol::{wl_registry, wl_seat::WlSeat},
        Connection, Dispatch, QueueHandle,
    };
    use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
        zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
        zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
    };
    use xkbcommon::xkb;

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
    delegate_noop!(State: ignore WlSeat);
    delegate_noop!(State: ignore ZwpVirtualKeyboardManagerV1);
    delegate_noop!(State: ignore ZwpVirtualKeyboardV1);

    let connection = Connection::connect_to_env().unwrap();
    let (globals, mut queue) = registry_queue_init::<State>(&connection).unwrap();
    let qh = queue.handle();
    let seat = globals.bind::<WlSeat, _, _>(&qh, 1..=1, ()).unwrap();
    let manager = globals
        .bind::<ZwpVirtualKeyboardManagerV1, _, _>(&qh, 1..=1, ())
        .unwrap();
    let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_names(
        &context,
        "evdev",
        "pc105",
        "us",
        "",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .unwrap();
    let bytes = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1) + "\0";
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes.as_bytes()).unwrap();
    keyboard.keymap(1, file.as_fd(), bytes.len().try_into().unwrap());
    queue.roundtrip(&mut State).unwrap();
    println!("READY");
    let clock = std::time::Instant::now();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut words = line.split_whitespace();
        let code: u32 = words.next().expect("evdev keycode").parse().unwrap();
        let modifiers = words.next().expect("modifiers");
        assert!(words.next().is_none(), "unexpected keyboard argument");
        let mut mask = 0;
        for modifier in modifiers.split('+') {
            let name = match modifier {
                "none" => continue,
                "shift" => xkb::MOD_NAME_SHIFT,
                "ctrl" => xkb::MOD_NAME_CTRL,
                "alt" => xkb::MOD_NAME_ALT,
                _ => panic!("unknown modifier: {modifier}"),
            };
            let index = keymap.mod_get_index(name);
            assert_ne!(index, xkb::MOD_INVALID);
            mask |= 1 << index;
        }
        // Virtual-keyboard key requests do not update XKB state. Send masks
        // relative to the exact keymap, not wlroots convenience modifier bits.
        keyboard.modifiers(mask, 0, 0, 0);
        keyboard.key(clock.elapsed().as_millis() as u32, code, 1);
        queue.roundtrip(&mut State).unwrap();
        thread::sleep(Duration::from_millis(40));
        keyboard.key(clock.elapsed().as_millis() as u32, code, 0);
        keyboard.modifiers(0, 0, 0, 0);
        queue.roundtrip(&mut State).unwrap();
        thread::sleep(Duration::from_millis(40));
        println!("KEYED {code} {modifiers}");
    }
    keyboard.destroy();
    queue.roundtrip(&mut State).unwrap();
}
