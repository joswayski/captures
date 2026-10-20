//! Screenshot editor clipboard input. Editable snapshots stay in the session;
//! only an opaque private marker is published, never text or document JSON.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use captures_app::editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS};
use image::RgbaImage;

pub type Decoded = Result<Vec<(String, RgbaImage)>, String>;

/// Start consuming transports immediately, independently of the editor's busy
/// mutation worker. A queue holds the result, never a request to read a later OS selection.
pub fn decode_async(
    snapshot: Result<Snapshot, String>,
    ctx: &eframe::egui::Context,
    viewport: eframe::egui::ViewportId,
) -> std::sync::mpsc::Receiver<Decoded> {
    let (send, receive) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = send.send(snapshot.and_then(Snapshot::decode));
        ctx.request_repaint_of(viewport);
    });
    receive
}

/// Menu activation occurs on the native event loop. Enqueue the Wayland offer
/// request before returning to it, rather than waiting for a worker to do so.
pub fn capture_menu(ctx: &eframe::egui::Context) -> std::sync::mpsc::Receiver<Decoded> {
    let viewport = ctx.viewport_id();
    #[cfg(target_os = "linux")]
    if WAYLAND.load(std::sync::atomic::Ordering::Acquire) {
        let (reply, receive) = std::sync::mpsc::sync_channel(1);
        OFFER_READS.lock().unwrap().push(reply);
        let (send, result) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
        std::thread::spawn(move || {
            let decoded = receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| "Clipboard offer read did not reach the native event loop.".to_owned())
                .and_then(|offer| offer)
                .and_then(|offer| Snapshot::from_wayland(offer).decode());
            let _ = send.send(decoded);
            ctx.request_repaint_of(viewport);
        });
        return result;
    }
    decode_async(capture_native(), ctx, viewport)
}

/// An owned, single-use clipboard acquisition. No external file is opened until decode.
pub struct Snapshot {
    #[cfg(target_os = "windows")]
    files: Vec<std::path::PathBuf>,
    #[cfg(target_os = "windows")]
    bitmap: Result<Option<(u32, Vec<u8>)>, String>,
    #[cfg(target_os = "linux")]
    source: LinuxSource,
}

#[cfg(target_os = "linux")]
enum LinuxSource {
    X11(Box<x11_marker::Offer>),
    Wayland(winit::platform::wayland::ClipboardOffer),
}

impl Snapshot {
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    pub fn decode(self) -> Decoded {
        Err("Retained clipboard acquisition uses the AppKit host on macOS.".into())
    }

    #[cfg(target_os = "linux")]
    pub fn from_wayland(offer: winit::platform::wayland::ClipboardOffer) -> Self {
        Self {
            source: LinuxSource::Wayland(offer),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn decode(self) -> Result<Vec<(String, RgbaImage)>, String> {
        #[cfg(target_os = "windows")]
        let (files, bitmap) = (self.files, self.bitmap);
        #[cfg(target_os = "linux")]
        let (files, png) = match self.source {
            LinuxSource::X11(offer) => offer.read().map_err(|error| error.to_string())?,
            LinuxSource::Wayland(offer) => {
                let png = offer.png.map_or(LinuxPng::Absent, LinuxPng::Wayland);
                let bytes = match offer.uri_list {
                    Some(fd) => read_transfer(fd, 1024 * 1024, "Clipboard file list")?,
                    None => Vec::new(),
                };
                (parse_files(&bytes)?, png)
            }
        };
        let files: Vec<_> = files
            .into_iter()
            .filter(|path| {
                path.is_absolute()
                    && path.is_file()
                    && captures_app::editor_session::is_supported_image_path(path)
            })
            .collect();
        if !files.is_empty() {
            return captures_app::editor_image_decode::decode_imports(files);
        }
        #[cfg(target_os = "windows")]
        let bytes = {
            let Some((format, bytes)) = bitmap? else {
                return Ok(Vec::new());
            };
            if format == clipboard_win::formats::CF_DIBV5 {
                return captures_app::editor_image_decode::decode_clipboard_dib(bytes)
                    .map(|pixels| vec![("Clipboard image".into(), pixels)]);
            }
            bytes
        };
        #[cfg(target_os = "linux")]
        let bytes = match png {
            LinuxPng::Absent => return Ok(Vec::new()),
            LinuxPng::Wayland(fd) => read_transfer(
                fd,
                captures_history::editor_draft::MAX_IMAGE_BYTES,
                "Clipboard image",
            )?,
            LinuxPng::Encoded(bytes) => bytes?,
        };
        if !matches!(image::guess_format(&bytes), Ok(image::ImageFormat::Png)) {
            return Err("Could not decode clipboard PNG. Copy it again and retry.".into());
        }
        let pixels =
            captures_app::editor_image_decode::decode_import_bytes(&bytes, "Clipboard image")?;
        Ok(vec![("Clipboard image".into(), pixels)])
    }
}

/// Call at native input, before enqueueing work to the editor worker.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn capture_native() -> Result<Snapshot, String> {
    Err("Retained clipboard acquisition uses the AppKit host on macOS.".into())
}

#[cfg(target_os = "linux")]
pub fn capture_native() -> Result<Snapshot, String> {
    if WAYLAND.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Wayland acquisition requires the native event loop clipboard offer.".into());
    }
    x11_marker::capture()
        .map(|offer| Snapshot {
            source: LinuxSource::X11(Box::new(offer)),
        })
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
pub fn capture_native() -> Result<Snapshot, String> {
    let _guard = clipboard_win::Clipboard::new_attempts(10)
        .map_err(|error| format!("Could not open clipboard: {error}"))?;
    let mut files = Vec::new();
    if clipboard_win::raw::is_format_avail(clipboard_win::formats::CF_HDROP) {
        clipboard_win::raw::get_file_list_path(&mut files)
            .map_err(|error| format!("Could not read clipboard files: {error}"))?;
    }
    let bitmap = (|| -> Result<_, String> {
        let png = clipboard_win::register_format("PNG")
            .ok_or("Could not register clipboard PNG format.")?
            .get();
        let format = if clipboard_win::raw::is_format_avail(png) {
            png
        } else if clipboard_win::raw::is_format_avail(clipboard_win::formats::CF_DIBV5) {
            clipboard_win::formats::CF_DIBV5
        } else {
            return Ok(None);
        };
        let size = clipboard_win::raw::size(format)
            .ok_or("Could not read clipboard image size.")?
            .get();
        if size > captures_history::editor_draft::MAX_IMAGE_BYTES {
            return Err("Clipboard image exceeds the encoded-byte limit.".into());
        }
        let mut bytes = Vec::new();
        clipboard_win::raw::get_vec(format, &mut bytes)
            .map_err(|error| format!("Could not read clipboard image: {error}"))?;
        Ok(Some((format, bytes)))
    })();
    Ok(Snapshot { files, bitmap })
}

#[derive(Default)]
pub struct Clipboard {
    marker: Option<std::sync::Arc<Marker>>,
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    images: Option<arboard::Clipboard>,
}

impl Clipboard {
    pub fn claim_layer(&mut self, ctx: &eframe::egui::Context) -> Result<(), String> {
        self.marker = None;
        self.marker = Some(std::sync::Arc::new(Marker::claim(ctx)?));
        Ok(())
    }

    pub fn claim_copy(
        &mut self,
        ctx: &eframe::egui::Context,
        layer: std::sync::Arc<captures_app::editor_session::CopiedLayer>,
        copies: &std::sync::Arc<std::sync::Mutex<Copies>>,
    ) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if WAYLAND.load(std::sync::atomic::Ordering::Acquire) {
            self.marker = None;
            self.marker = Some(std::sync::Arc::new(Marker::claim_wayland(
                ctx,
                Some((copies.clone(), layer)),
            )?));
            return Ok(());
        }
        // Only platforms that do not need the UI event loop may hold this lock
        // while claiming. Input sees the marker and payload as one publication.
        let mut copies = copies.lock().unwrap();
        self.claim_layer(ctx)?;
        copies.publish(self.published(layer));
        Ok(())
    }

    pub fn owns_layer(&self) -> bool {
        self.marker.as_ref().is_some_and(|marker| marker.matches())
    }

    pub fn published(
        &self,
        layer: std::sync::Arc<captures_app::editor_session::CopiedLayer>,
    ) -> Published {
        Published {
            marker: self.marker.clone().expect("claimed marker"),
            layer,
        }
    }

    pub fn images(
        &mut self,
        ctx: &eframe::egui::Context,
    ) -> Result<Vec<(String, RgbaImage)>, String> {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            #[cfg(target_os = "linux")]
            if WAYLAND.load(std::sync::atomic::Ordering::Acquire) {
                let (reply, receive) = std::sync::mpsc::sync_channel(1);
                OFFER_READS.lock().unwrap().push(reply);
                ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                let offer = receive
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|_| "Clipboard offer read did not reach the native event loop.")??;
                return Snapshot::from_wayland(offer).decode();
            }
            let _ = ctx;
            capture_native()?.decode()
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        self.other_images(ctx)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    fn other_images(
        &mut self,
        ctx: &eframe::egui::Context,
    ) -> Result<Vec<(String, RgbaImage)>, String> {
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
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            if self.images.is_none() {
                self.images = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
            }
            let image = match self.images.as_mut().unwrap().get_image() {
                Ok(image) => image,
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
}

#[derive(Clone)]
pub struct Published {
    marker: std::sync::Arc<Marker>,
    layer: std::sync::Arc<captures_app::editor_session::CopiedLayer>,
}

impl Published {
    pub fn retained(&self) -> Option<std::sync::Arc<captures_app::editor_session::CopiedLayer>> {
        self.marker.matches().then(|| self.layer.clone())
    }
}

/// Keep old Wayland generations only while a protocol-time occurrence (or its
/// active source) holds the token. A later Copy cannot substitute its payload.
#[derive(Default)]
pub struct Copies(Vec<Published>);

impl Copies {
    pub fn publish(&mut self, copy: Published) {
        self.0.retain(|copy| {
            #[cfg(target_os = "linux")]
            if let Marker::Wayland(token) = copy.marker.as_ref() {
                return std::sync::Arc::strong_count(token) > 1;
            }
            let _ = copy;
            false
        });
        self.0.push(copy);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn retained(&self) -> Option<std::sync::Arc<captures_app::editor_session::CopiedLayer>> {
        self.0.last().and_then(Published::retained)
    }

    #[cfg(target_os = "linux")]
    pub fn find_marker(
        &self,
        token: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Option<std::sync::Arc<captures_app::editor_session::CopiedLayer>> {
        self.0.iter().find_map(|copy| match copy.marker.as_ref() {
            Marker::Wayland(owned) if std::sync::Arc::ptr_eq(owned, token) => {
                Some(copy.layer.clone())
            }
            _ => None,
        })
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
    publication: Option<(
        std::sync::Arc<std::sync::Mutex<Copies>>,
        std::sync::Arc<captures_app::editor_session::CopiedLayer>,
    )>,
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
    // Menu Paste accepted the current offer during the redraw that just ended.
    // Acquire it before publishing any worker Copy that completed meanwhile.
    for reply in std::mem::take(&mut *OFFER_READS.lock().unwrap()) {
        let _ = reply.send(event_loop.clipboard_offer().map_err(str::to_owned));
    }
    for claim in std::mem::take(&mut *CLAIMS.lock().unwrap()) {
        // A timed-out/closed worker dropped its reply. Don't publish late.
        if std::sync::Arc::strong_count(&claim.owned) == 1 {
            continue;
        }
        let owned = claim.owned.clone();
        let result = event_loop
            .set_clipboard_marker(claim.mime, claim.owned)
            .map_err(str::to_owned);
        if result.is_ok()
            && let Some((copies, layer)) = claim.publication
        {
            copies.lock().unwrap().publish(Published {
                marker: std::sync::Arc::new(Marker::Wayland(owned)),
                layer,
            });
        }
        let _ = claim.reply.send(result);
    }
}

#[cfg(target_os = "linux")]
enum LinuxPng {
    Absent,
    Encoded(Result<Vec<u8>, String>),
    Wayland(std::os::fd::OwnedFd),
}

#[cfg(target_os = "linux")]
fn parse_files(bytes: &[u8]) -> Result<Vec<std::path::PathBuf>, String> {
    // Parse lines before percent decoding: encoded CR/LF belong to filenames,
    // but literal CRLF belongs to the URI-list transport. Never fetch URLs.
    let text = std::str::from_utf8(bytes).map_err(|_| "Invalid clipboard file list.")?;
    let files = text
        .lines()
        .filter(|line| {
            (line.starts_with("file:///") || line.starts_with("file://localhost/"))
                && !line.contains(['?', '#'])
        })
        .map(|line| captures_app::preview_channel::local_media_path(line.into()))
        .collect::<Result<_, _>>()?;
    Ok(files)
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
        if !WAYLAND.load(std::sync::atomic::Ordering::Acquire) {
            return x11_marker::Marker::claim().map(Self::X11);
        }
        Self::claim_wayland(ctx, None)
    }

    fn claim_wayland(
        ctx: &eframe::egui::Context,
        publication: Option<(
            std::sync::Arc<std::sync::Mutex<Copies>>,
            std::sync::Arc<captures_app::editor_session::CopiedLayer>,
        )>,
    ) -> Result<Self, String> {
        use std::sync::{Arc, atomic::AtomicBool, mpsc};
        let owned = Arc::new(AtomicBool::new(false));
        let (reply, receive) = mpsc::sync_channel(1);
        CLAIMS.lock().unwrap().push(WaylandClaim {
            mime: format!(
                "application/x-captures-editor-layer-{}",
                uuid::Uuid::new_v4()
            ),
            owned: owned.clone(),
            reply,
            publication,
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
        protocol::{
            Event,
            xfixes::{ConnectionExt as _, SelectionEventMask},
            xproto::*,
        },
        rust_connection::RustConnection,
        wrapper::ConnectionExt as _,
    };

    pub(super) struct Transfer {
        connection: RustConnection,
        window: Window,
        selection: Atom,
        property: Atom,
        target: Atom,
        deadline: std::time::Instant,
    }

    impl Transfer {
        pub(super) fn read(self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let bytes = read_selection(
                &self,
                (self.target, 8),
                captures_history::editor_draft::MAX_IMAGE_BYTES,
            )?
            .ok_or("Clipboard offer became unavailable.")?;
            ensure_unchanged(&self.connection)?;
            Ok(bytes)
        }
    }

    pub(super) struct Offer {
        targets: Transfer,
        uri: Transfer,
        png: Transfer,
    }

    pub(super) fn capture() -> Result<Offer, Box<dyn std::error::Error>> {
        // Separate connections prevent a reader from consuming another target's
        // SelectionNotify/PropertyNotify events. The first watch remains alive
        // until every relevant acquisition has completed.
        let targets = start(b"TARGETS")?;
        let uri = start(b"text/uri-list")?;
        let png = start(b"image/png")?;
        Ok(Offer { targets, uri, png })
    }

    impl Offer {
        pub(super) fn read(
            self,
        ) -> Result<(Vec<std::path::PathBuf>, super::LinuxPng), Box<dyn std::error::Error>>
        {
            let t = &self.targets;
            let formats = read_selection(t, (AtomEnum::ATOM.into(), 32), 1024 * 1024)?;
            let Some(formats) = formats else {
                if t.connection
                    .get_selection_owner(t.selection)?
                    .reply()?
                    .owner
                    == 0
                {
                    ensure_unchanged(&t.connection)?;
                    return Ok((Vec::new(), super::LinuxPng::Absent));
                }
                return Err("Clipboard offer became unavailable.".into());
            };
            let atoms: Vec<_> = formats
                .chunks_exact(4)
                .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("four-byte atom")))
                .collect();
            let u = &self.uri;
            let bytes = if atoms.contains(&u.target) {
                read_selection(u, (u.target, 8), 1024 * 1024)?
                    .ok_or("Clipboard offer became unavailable.")?
            } else {
                Vec::new()
            };
            let files = super::parse_files(&bytes)?;
            // Finish PNG before dropping the first watch, but defer its failures
            // so usable local files still win. Unsupported targets are ignored.
            let png = if atoms.contains(&self.png.target) {
                super::LinuxPng::Encoded(self.png.read().map_err(|error| error.to_string()))
            } else {
                super::LinuxPng::Absent
            };
            ensure_unchanged(&t.connection)?;
            Ok((files, png))
        }
    }

    fn start(name: &[u8]) -> Result<Transfer, Box<dyn std::error::Error>> {
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
        // Owner-window equality misses a client replacing its own selection.
        // Watch every ownership transition before reading any offered format.
        connection.xfixes_query_version(2, 0)?.reply()?;
        connection
            .xfixes_select_selection_input(
                window,
                selection,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?
            .check()?;
        let target = connection.intern_atom(false, name)?.reply()?.atom;
        let property = connection
            .intern_atom(false, b"CAPTURES_EDITOR_URI_LIST")?
            .reply()?
            .atom;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        connection
            .convert_selection(window, selection, target, property, x11rb::CURRENT_TIME)?
            .check()?;
        connection.flush()?;
        let offer = Transfer {
            connection,
            window,
            selection,
            property,
            target,
            deadline,
        };
        Ok(offer)
    }

    fn ensure_unchanged(connection: &RustConnection) -> Result<(), Box<dyn std::error::Error>> {
        // A server round trip collects notifications preceding completion of the read.
        connection.get_input_focus()?.reply()?;
        while let Some(event) = connection.poll_for_event()? {
            if matches!(event, Event::XfixesSelectionNotify(_)) {
                return Err(
                    "Clipboard changed during Paste. Paste again to use the new selection.".into(),
                );
            }
        }
        Ok(())
    }

    /// Read one bounded selection, directly or through ICCCM's INCR handshake.
    /// The size hint is only a lower bound; only a typed empty chunk ends INCR.
    fn read_selection(
        transfer: &Transfer,
        expected: (Atom, u8),
        maximum: usize,
    ) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        let Transfer {
            ref connection,
            window,
            selection,
            target,
            property,
            deadline,
        } = *transfer;
        let (expected_type, expected_format) = expected;
        let incr = connection.intern_atom(false, b"INCR")?.reply()?.atom;
        let mut incremental = false;
        let mut bytes = Vec::new();
        loop {
            if std::time::Instant::now() >= deadline {
                return Err("Clipboard transfer timed out.".into());
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
                Event::XfixesSelectionNotify(_) => {
                    return Err(
                        "Clipboard changed during Paste. Paste again to use the new selection."
                            .into(),
                    );
                }
                _ => continue,
            }
            // ANY lets delete=true acknowledge an INCR header as well as data.
            // Never delete twice: that could discard the owner's next chunk.
            let result = connection
                .get_property(true, window, property, AtomEnum::ANY, 0, maximum as u32 / 4)?
                .reply()?;
            if !incremental && result.type_ == incr {
                if result.format != 32 || result.value.len() != 4 || result.bytes_after != 0 {
                    return Err("Clipboard incremental transfer has an invalid header.".into());
                }
                let hint = result
                    .value32()
                    .and_then(|mut sizes| sizes.next())
                    .ok_or("Clipboard incremental transfer has an invalid header.")?;
                if hint > maximum as u32 {
                    return Err(
                        format!("Clipboard data exceeds {} MiB.", maximum / (1024 * 1024)).into(),
                    );
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
            if result.value.len() > maximum - bytes.len() {
                return Err(
                    format!("Clipboard data exceeds {} MiB.", maximum / (1024 * 1024)).into(),
                );
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

#[cfg(all(test, target_os = "linux"))]
mod snapshot_tests {
    use super::Snapshot;
    use std::{io::Write, os::fd::OwnedFd, os::unix::net::UnixStream};
    use winit::platform::wayland::ClipboardOffer;

    fn transfer(bytes: &[u8]) -> OwnedFd {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(bytes).unwrap();
        drop(writer);
        reader.into()
    }

    #[test]
    fn retained_offer_is_send_and_ignores_nonlocal_files() {
        fn assert_send<T: Send>() {}
        assert_send::<Snapshot>();
        let snapshot = Snapshot::from_wayland(ClipboardOffer {
            uri_list: Some(transfer(
                b"https://example.com/image.png\r\nfile://remote/image.png\r\n",
            )),
            ..Default::default()
        });
        assert!(
            std::thread::spawn(move || snapshot.decode())
                .join()
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn retained_png_uses_the_png_validation_path() {
        let snapshot = Snapshot::from_wayland(ClipboardOffer {
            png: Some(transfer(b"not a PNG")),
            ..Default::default()
        });
        assert!(
            snapshot
                .decode()
                .unwrap_err()
                .contains("Could not decode clipboard PNG")
        );
    }
}
