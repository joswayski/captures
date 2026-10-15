//! Screenshot editor clipboard input. Editable snapshots stay in the session;
//! only an opaque private marker is published, never text or document JSON.
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

    pub fn images(&mut self) -> Result<Vec<(String, RgbaImage)>, String> {
        if self.images.is_none() {
            self.images = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
        }
        let clipboard = self.images.as_mut().unwrap();
        let files = clipboard.get().file_list().unwrap_or_default();
        let files: Vec<_> = files
            .into_iter()
            .filter(|path| {
                // arboard's Linux URI decoder also accepts file://host/... as a
                // relative path. Only actual local absolute files are imported.
                path.is_absolute()
                    && path.is_file()
                    && captures_app::editor_session::is_supported_image_path(path)
            })
            .collect();
        if !files.is_empty() {
            return files
                .into_iter()
                .map(|path| {
                    let pixels = captures_app::editor_image_decode::decode_import(&path)?;
                    Ok((
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        pixels,
                    ))
                })
                .collect();
        }
        let image = clipboard
            .get_image()
            .map_err(|error| format!("No clipboard image available: {error}"))?;
        let width = u32::try_from(image.width).map_err(|_| "Clipboard image is too wide.")?;
        let height = u32::try_from(image.height).map_err(|_| "Clipboard image is too tall.")?;
        if width == 0
            || height == 0
            || width > 16_384
            || height > 16_384
            || u64::from(width) * u64::from(height) > 100_000_000
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
impl Marker {
    fn claim(_: &eframe::egui::Context) -> Result<Self, String> {
        let format = clipboard_win::register_format("application/x-captures-editor-layer")
            .ok_or("Could not register editor clipboard format.")?
            .get();
        let token = uuid::Uuid::new_v4().as_bytes().to_vec();
        let _guard =
            clipboard_win::Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
        clipboard_win::raw::set(format, &token).map_err(|error| error.to_string())?;
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
