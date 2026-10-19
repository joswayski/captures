//! Screenshot editor clipboard input. Editable snapshots stay in the session;
//! only an opaque private marker is published, never text or document JSON.
use captures_app::editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS};
use image::RgbaImage;

#[derive(Default)]
pub struct Clipboard {
    marker: Option<Marker>,
    images: Option<arboard::Clipboard>,
}

impl Clipboard {
    pub fn claim_layer(&mut self, ctx: &eframe::egui::Context) -> Result<(), String> {
        self.marker = None;
        self.marker = Some(Marker::claim(ctx)?);
        Ok(())
    }

    pub fn owns_layer(&self) -> bool {
        self.marker.as_ref().is_some_and(Marker::matches)
    }

    pub fn images(
        &mut self,
        ctx: &eframe::egui::Context,
    ) -> Result<Vec<(String, RgbaImage)>, String> {
        #[cfg(target_os = "linux")]
        let (files, png) = linux_files(ctx)?;
        #[cfg(target_os = "windows")]
        let files = {
            let _guard = clipboard_win::Clipboard::new_attempts(10)
                .map_err(|error| format!("Could not open clipboard: {error}"))?;
            let mut files = Vec::new();
            if clipboard_win::raw::is_format_avail(clipboard_win::formats::CF_HDROP) {
                // arboard maps both missing HDROP and failed reads to the
                // same error. Check availability while holding the clipboard.
                clipboard_win::raw::get_file_list_path(&mut files)
                    .map_err(|error| format!("Could not read clipboard files: {error}"))?;
            }
            files
        };
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let files = {
            if self.images.is_none() {
                self.images = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
            }
            self.images
                .as_mut()
                .unwrap()
                .get()
                .file_list()
                .unwrap_or_default()
        };
        #[cfg(not(target_os = "linux"))]
        let _ = ctx;
        let files: Vec<_> = files
            .into_iter()
            .filter(|path| {
                // Import local image files, never URLs or HTML references.
                path.is_absolute()
                    && path.is_file()
                    && captures_app::editor_session::is_supported_image_path(path)
            })
            .collect();
        if !files.is_empty() {
            return captures_app::editor_image_decode::decode_imports(files);
        }
        // Linux's image reader can report both absent formats and real
        // transfer failures as ContentNotAvailable. Inspect the actual offer
        // instead of swallowing errors or probing an unsupported PNG target.
        #[cfg(target_os = "linux")]
        match png {
            LinuxPng::Absent => return Ok(Vec::new()),
            LinuxPng::Wayland(fd) => {
                let bytes = read_transfer(
                    fd,
                    captures_history::editor_draft::MAX_IMAGE_BYTES,
                    "Clipboard image",
                )?;
                if !matches!(image::guess_format(&bytes), Ok(image::ImageFormat::Png)) {
                    return Err("Could not decode clipboard PNG. Copy it again and retry.".into());
                }
                let pixels = captures_app::editor_image_decode::decode_import_bytes(
                    &bytes,
                    "Clipboard image",
                )?;
                return Ok(vec![("Clipboard image".into(), pixels)]);
            }
            LinuxPng::X11 => {}
        }
        if self.images.is_none() {
            self.images = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
        }
        let image = match self.images.as_mut().unwrap().get_image() {
            Ok(image) => image,
            // Windows distinguishes absent formats from access/decoding
            // failures. The separate AppKit host already ignores absent images.
            #[cfg(target_os = "windows")]
            Err(arboard::Error::ContentNotAvailable) => return Ok(Vec::new()),
            Err(arboard::Error::ConversionFailure) => {
                return Err("Could not decode clipboard image. Copy it again and retry.".into());
            }
            Err(error) => return Err(format!("Could not read clipboard image: {error}")),
        };
        let width = u32::try_from(image.width).map_err(|_| "Clipboard image is too wide.")?;
        let height = u32::try_from(image.height).map_err(|_| "Clipboard image is too tall.")?;
        if width == 0
            || height == 0
            || width > MAX_RENDER_DIMENSION
            || height > MAX_RENDER_DIMENSION
            || u64::from(width) * u64::from(height) > MAX_RENDER_PIXELS
        {
            return Err(
                "Clipboard images are limited to 16,384 pixels per side and 100,000,000 pixels."
                    .into(),
            );
        }
        let pixels = RgbaImage::from_raw(width, height, image.bytes.into_owned())
            .ok_or("Clipboard image has invalid RGBA pixels.")?;
        Ok(vec![("Clipboard image".into(), pixels)])
    }
}

#[cfg(target_os = "windows")]
struct Marker {
    revision: std::num::NonZeroU32,
}

#[cfg(target_os = "windows")]
pub fn set_window(ctx: &eframe::egui::Context, window: &std::sync::Arc<winit::window::Window>) {
    ctx.data_mut(|data| {
        data.insert_temp(
            eframe::egui::Id::unique("editor-clipboard-window"),
            std::sync::Arc::downgrade(window),
        )
    });
}

#[cfg(target_os = "windows")]
impl Marker {
    fn claim(ctx: &eframe::egui::Context) -> Result<Self, String> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let window = ctx
            .data(|data| {
                data.get_temp::<std::sync::Weak<winit::window::Window>>(eframe::egui::Id::unique(
                    "editor-clipboard-window",
                ))
            })
            .and_then(|window| window.upgrade())
            .ok_or("Clipboard owner window is unavailable.")?;
        let handle = window.window_handle().map_err(|error| error.to_string())?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err("Clipboard owner is not a Win32 window.".into());
        };
        let format = clipboard_win::register_format("application/x-captures-editor-layer")
            .ok_or("Could not register editor clipboard format.")?
            .get();
        let token = uuid::Uuid::new_v4().as_bytes().to_vec();
        let guard = clipboard_win::Clipboard::new_attempts_for(handle.hwnd.get() as _, 10)
            .map_err(|error| error.to_string())?;
        clipboard_win::raw::set(format, &token).map_err(|error| error.to_string())?;
        drop(guard);
        // Reopen and verify after publication; never label an intervening
        // external writer's sequence as ours.
        let _guard =
            clipboard_win::Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
        let mut published = Vec::new();
        clipboard_win::raw::get_vec(format, &mut published).map_err(|error| error.to_string())?;
        if published != token {
            return Err("Clipboard changed during layer Copy.".into());
        }
        let revision = clipboard_win::seq_num().ok_or("Clipboard revision unavailable.")?;
        Ok(Self { revision })
    }

    fn matches(&self) -> bool {
        clipboard_win::seq_num() == Some(self.revision)
    }
}

#[cfg(target_os = "linux")]
enum Marker {
    X11(x11_marker::Marker),
    Wayland(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

#[cfg(target_os = "linux")]
static WAYLAND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "linux")]
struct WaylandClaim {
    mime: String,
    owned: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reply: std::sync::mpsc::SyncSender<Result<(), String>>,
}

#[cfg(target_os = "linux")]
static CLAIMS: std::sync::Mutex<Vec<WaylandClaim>> = std::sync::Mutex::new(Vec::new());

#[cfg(target_os = "linux")]
type OfferReply =
    std::sync::mpsc::SyncSender<Result<winit::platform::wayland::ClipboardOffer, String>>;
#[cfg(target_os = "linux")]
static OFFER_READS: std::sync::Mutex<Vec<OfferReply>> = std::sync::Mutex::new(Vec::new());

#[cfg(target_os = "linux")]
pub fn service(event_loop: &winit::event_loop::ActiveEventLoop) {
    use std::sync::atomic::Ordering;
    use winit::platform::wayland::ActiveEventLoopExtWayland;
    WAYLAND.store(event_loop.is_wayland(), Ordering::Release);
    for claim in std::mem::take(&mut *CLAIMS.lock().unwrap()) {
        // A timed-out/closed worker dropped its reply. Don't publish late.
        if std::sync::Arc::strong_count(&claim.owned) == 1 {
            continue;
        }
        let result = event_loop
            .set_clipboard_marker(claim.mime, claim.owned)
            .map_err(str::to_owned);
        let _ = claim.reply.send(result);
    }
    for reply in std::mem::take(&mut *OFFER_READS.lock().unwrap()) {
        let _ = reply.send(event_loop.clipboard_offer().map_err(str::to_owned));
    }
}

#[cfg(target_os = "linux")]
enum LinuxPng {
    Absent,
    X11,
    Wayland(std::os::fd::OwnedFd),
}

#[cfg(target_os = "linux")]
fn linux_files(ctx: &eframe::egui::Context) -> Result<(Vec<std::path::PathBuf>, LinuxPng), String> {
    use std::{sync::atomic::Ordering, time::Duration};
    let (png, bytes) = if WAYLAND.load(Ordering::Acquire) {
        let (reply, receive) = std::sync::mpsc::sync_channel(1);
        OFFER_READS.lock().unwrap().push(reply);
        ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
        let offer = receive
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Clipboard offer read did not reach the native event loop.")??;
        let png = offer.png.map_or(LinuxPng::Absent, LinuxPng::Wayland);
        let bytes = match offer.uri_list {
            Some(fd) => read_transfer(fd, 1024 * 1024, "Clipboard file list")?,
            None => Vec::new(),
        };
        (png, bytes)
    } else {
        let (has_png, bytes) = x11_marker::offered_files().map_err(|error| error.to_string())?;
        (
            if has_png {
                LinuxPng::X11
            } else {
                LinuxPng::Absent
            },
            bytes,
        )
    };
    // Parse lines before percent decoding: encoded CR/LF belong to filenames,
    // but literal CRLF belongs to the URI-list transport. Never fetch URLs.
    let text = std::str::from_utf8(&bytes).map_err(|_| "Invalid clipboard file list.")?;
    let files = text
        .lines()
        .filter(|line| {
            (line.starts_with("file:///") || line.starts_with("file://localhost/"))
                && !line.contains(['?', '#'])
        })
        .map(|line| captures_app::preview_channel::local_media_path(line.into()))
        .collect::<Result<_, _>>()?;
    Ok((files, png))
}

#[cfg(target_os = "linux")]
fn read_transfer(fd: std::os::fd::OwnedFd, maximum: usize, label: &str) -> Result<Vec<u8>, String> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    use std::{
        io::Read,
        time::{Duration, Instant},
    };
    let flags = fcntl_getfl(&fd).map_err(|error| error.to_string())?;
    fcntl_setfl(&fd, flags | OFlags::NONBLOCK).map_err(|error| error.to_string())?;
    let mut pipe = std::fs::File::from(fd);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(format!("{label} transfer timed out."));
        }
        let mut chunk = [0; 8192];
        match pipe.read(&mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(count) => {
                if count > maximum - bytes.len() {
                    return Err(format!("{label} exceeds {} MiB.", maximum / (1024 * 1024)));
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
impl Marker {
    fn claim(ctx: &eframe::egui::Context) -> Result<Self, String> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        if !WAYLAND.load(Ordering::Acquire) {
            return x11_marker::Marker::claim().map(Self::X11);
        }
        let owned = Arc::new(AtomicBool::new(false));
        let (reply, receive) = mpsc::sync_channel(1);
        CLAIMS.lock().unwrap().push(WaylandClaim {
            mime: format!(
                "application/x-captures-editor-layer-{}",
                uuid::Uuid::new_v4()
            ),
            owned: owned.clone(),
            reply,
        });
        ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
        receive
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "Clipboard Copy did not reach the native event loop.")??;
        Ok(Self::Wayland(owned))
    }

    fn matches(&self) -> bool {
        match self {
            Self::X11(marker) => marker.matches(),
            Self::Wayland(owned) => owned.load(std::sync::atomic::Ordering::Acquire),
        }
    }
}

#[cfg(target_os = "linux")]
mod x11_marker {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use x11rb::{
        connection::Connection,
        protocol::{Event, xproto::*},
        rust_connection::RustConnection,
        wrapper::ConnectionExt as _,
    };

    pub(super) fn offered_files() -> Result<(bool, Vec<u8>), Box<dyn std::error::Error>> {
        let (connection, screen) = x11rb::connect(None)?;
        let window = connection.generate_id()?;
        connection
            .create_window(
                0,
                window,
                connection.setup().roots[screen].root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        let selection = connection.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
        let target = connection
            .intern_atom(false, b"text/uri-list")?
            .reply()?
            .atom;
        let property = connection
            .intern_atom(false, b"CAPTURES_EDITOR_URI_LIST")?
            .reply()?
            .atom;
        let targets = connection.intern_atom(false, b"TARGETS")?.reply()?.atom;
        let png = connection.intern_atom(false, b"image/png")?.reply()?.atom;
        let Some(formats) = read_selection(
            &connection,
            window,
            selection,
            targets,
            property,
            AtomEnum::ATOM.into(),
            32,
        )?
        else {
            if connection.get_selection_owner(selection)?.reply()?.owner == 0 {
                return Ok((false, Vec::new()));
            }
            return Err("Clipboard offer became unavailable.".into());
        };
        let atoms: Vec<_> = formats
            .chunks_exact(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("four-byte atom")))
            .collect();
        let has_png = atoms.contains(&png);
        if !atoms.contains(&target) {
            return Ok((has_png, Vec::new()));
        }
        let bytes = read_selection(&connection, window, selection, target, property, target, 8)?
            .ok_or("Clipboard offer became unavailable.")?;
        // This dedicated connection owns the requestor window. Every exit,
        // including an abandoned INCR transfer, destroys it and its properties.
        Ok((has_png, bytes))
    }

    /// Read one bounded selection, directly or through ICCCM's INCR handshake.
    /// The size hint is only a lower bound; only a typed empty chunk ends INCR.
    fn read_selection(
        connection: &RustConnection,
        window: Window,
        selection: Atom,
        target: Atom,
        property: Atom,
        expected_type: Atom,
        expected_format: u8,
    ) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        const MAX_BYTES: usize = 1024 * 1024;
        let incr = connection.intern_atom(false, b"INCR")?.reply()?.atom;
        connection
            .convert_selection(window, selection, target, property, x11rb::CURRENT_TIME)?
            .check()?;
        connection.flush()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut incremental = false;
        let mut bytes = Vec::new();
        loop {
            if std::time::Instant::now() >= deadline {
                return Err("Clipboard file transfer timed out.".into());
            }
            let Some(event) = connection.poll_for_event()? else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            match event {
                Event::SelectionNotify(event)
                    if !incremental
                        && event.requestor == window
                        && event.selection == selection
                        && event.target == target =>
                {
                    if event.property == 0 {
                        return Ok(None);
                    }
                    if event.property != property {
                        return Err("Clipboard reply used an unexpected property.".into());
                    }
                }
                Event::PropertyNotify(event)
                    if incremental
                        && event.window == window
                        && event.atom == property
                        && event.state == Property::NEW_VALUE => {}
                _ => continue,
            }
            // ANY lets delete=true acknowledge an INCR header as well as data.
            // Never delete twice: that could discard the owner's next chunk.
            let result = connection
                .get_property(
                    true,
                    window,
                    property,
                    AtomEnum::ANY,
                    0,
                    MAX_BYTES as u32 / 4,
                )?
                .reply()?;
            if !incremental && result.type_ == incr {
                if result.format != 32 || result.value.len() != 4 || result.bytes_after != 0 {
                    return Err("Clipboard incremental transfer has an invalid header.".into());
                }
                let hint = result
                    .value32()
                    .and_then(|mut sizes| sizes.next())
                    .ok_or("Clipboard incremental transfer has an invalid header.")?;
                if hint > MAX_BYTES as u32 {
                    return Err("Clipboard file list exceeds 1 MiB.".into());
                }
                incremental = true;
                continue;
            }
            if result.type_ != expected_type
                || result.format != expected_format
                || result.bytes_after != 0
                || result.value.len() % usize::from(expected_format / 8) != 0
            {
                return Err("Clipboard data has an invalid type or size.".into());
            }
            if !incremental {
                return Ok(Some(result.value));
            }
            if result.value.is_empty() {
                return Ok(Some(bytes));
            }
            if result.value.len() > MAX_BYTES - bytes.len() {
                return Err("Clipboard file list exceeds 1 MiB.".into());
            }
            bytes.extend_from_slice(&result.value);
        }
    }

    pub(super) struct Marker {
        connection: Arc<RustConnection>,
        window: Window,
        selection: Atom,
        stopped: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl Marker {
        pub(super) fn claim() -> Result<Self, String> {
            Self::open().map_err(|error| format!("Could not claim editor clipboard: {error}"))
        }

        fn open() -> Result<Self, Box<dyn std::error::Error>> {
            let (connection, screen) = x11rb::connect(None)?;
            let window = connection.generate_id()?;
            connection
                .create_window(
                    0,
                    window,
                    connection.setup().roots[screen].root,
                    0,
                    0,
                    1,
                    1,
                    0,
                    WindowClass::INPUT_ONLY,
                    0,
                    &CreateWindowAux::new(),
                )?
                .check()?;
            let selection = connection.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
            let private = connection
                .intern_atom(false, b"application/x-captures-editor-layer")?
                .reply()?
                .atom;
            let targets = connection.intern_atom(false, b"TARGETS")?.reply()?.atom;
            connection
                .set_selection_owner(window, selection, x11rb::CURRENT_TIME)?
                .check()?;
            connection.flush()?;
            let connection = Arc::new(connection);
            let stopped = Arc::new(AtomicBool::new(false));
            let conn = connection.clone();
            let stop = stopped.clone();
            let token = *uuid::Uuid::new_v4().as_bytes();
            let worker = std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match conn.poll_for_event() {
                        Ok(Some(Event::SelectionClear(_))) | Err(_) => break,
                        Ok(Some(Event::SelectionRequest(request))) => {
                            let property = if request.property == 0 {
                                request.target
                            } else {
                                request.property
                            };
                            let accepted = if request.target == targets {
                                conn.change_property32(
                                    PropMode::REPLACE,
                                    request.requestor,
                                    property,
                                    AtomEnum::ATOM,
                                    &[targets, private],
                                )
                                .is_ok()
                            } else if request.target == private {
                                conn.change_property8(
                                    PropMode::REPLACE,
                                    request.requestor,
                                    property,
                                    private,
                                    &token,
                                )
                                .is_ok()
                            } else {
                                false
                            };
                            let _ = conn.send_event(
                                false,
                                request.requestor,
                                EventMask::NO_EVENT,
                                SelectionNotifyEvent {
                                    response_type: SELECTION_NOTIFY_EVENT,
                                    sequence: 0,
                                    time: request.time,
                                    requestor: request.requestor,
                                    selection,
                                    target: request.target,
                                    property: if accepted { property } else { 0 },
                                },
                            );
                            let _ = conn.flush();
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    }
                }
                stop.store(true, Ordering::Release);
            });
            Ok(Self {
                connection,
                window,
                selection,
                stopped,
                worker: Some(worker),
            })
        }

        pub(super) fn matches(&self) -> bool {
            !self.stopped.load(Ordering::Acquire)
                && self
                    .connection
                    .get_selection_owner(self.selection)
                    .ok()
                    .and_then(|reply| reply.reply().ok())
                    .is_some_and(|reply| reply.owner == self.window)
        }
    }

    impl Drop for Marker {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            let _ = self.connection.destroy_window(self.window);
            let _ = self.connection.flush();
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
struct Marker;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl Marker {
    fn claim(_: &eframe::egui::Context) -> Result<Self, String> {
        Err("Use the AppKit host for editable layer Copy on macOS.".into())
    }
    fn matches(&self) -> bool {
        false
    }
}
