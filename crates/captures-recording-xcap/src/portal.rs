//! Portal-consented, CPU-mapped Wayland video. Never connect to an unrestricted
//! PipeWire remote or fall back to another screen capture backend.
use std::{
    io::Cursor,
    os::fd::{FromRawFd, IntoRawFd, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use captures_recording::RecordingTarget;
use dbus::{
    Path,
    arg::{PropMap, RefArg, Variant},
    blocking::{Connection, stdintf::org_freedesktop_dbus::Properties},
    message::MatchRule,
};
use image::RgbaImage;
use parking_lot::Mutex;
use pipewire as pw;
use pw::{
    properties::properties,
    spa::{
        self,
        buffer::{ChunkFlags, DataType},
        param::{
            ParamType,
            format::{MediaSubtype, MediaType},
            video::{VideoFormat, VideoInfoRaw},
        },
        pod::{Pod, serialize::PodSerializer},
        utils::{Direction, SpaTypes},
    },
    stream::{StreamFlags, StreamState},
};
use xcap::Frame;

const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const CONSENT_TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);
const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;

type Response = (String, Result<(u32, PropMap), String>);

#[derive(Debug, thiserror::Error)]
pub enum PortalVideoError {
    #[error("Desktop recording portal: selection cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

impl From<String> for PortalVideoError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

struct PortalSession {
    connection: Connection,
    owner: String,
    path: Path<'static>,
    responses: Receiver<Response>,
    closed: Receiver<String>,
    deadline: Instant,
}

impl PortalSession {
    fn open(
        source_type: u32,
        show_cursor: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Self, OwnedFd, Stream), PortalVideoError> {
        check_cancel(cancelled)?;
        let deadline = Instant::now() + CONSENT_TIMEOUT;
        let connection = Connection::new_session().map_err(error)?;
        // Activate the desktop's public portal on first use, before pinning its
        // owner. This is not a direct backend or unrestricted PipeWire fallback.
        let _: (u32,) = connection
            .with_proxy(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                CALL_TIMEOUT,
            )
            .method_call(
                "org.freedesktop.DBus",
                "StartServiceByName",
                (DESKTOP, 0_u32),
            )
            .map_err(error)?;
        check_cancel(cancelled)?;
        let (owner,): (String,) = connection
            .with_proxy(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                CALL_TIMEOUT,
            )
            .method_call("org.freedesktop.DBus", "GetNameOwner", (DESKTOP,))
            .map_err(error)?;
        // Pin the unique owner for both calls and signals. A restarted portal
        // must not inherit an old session or impersonate its responses.
        let (tx, responses) = mpsc::sync_channel(8);
        let sender = owner.clone();
        connection
            .add_match(
                MatchRule::new_signal(REQUEST, "Response").with_sender(DESKTOP),
                move |_: (), _, message| {
                    if message
                        .sender()
                        .is_some_and(|actual| actual.as_ref() == sender)
                        && let Some(path) = message.path()
                    {
                        let response = message.read2::<u32, PropMap>().map_err(error);
                        let _ = tx.try_send((path.to_string(), response));
                    }
                    true
                },
            )
            .map_err(error)?;
        let (tx, closed) = mpsc::sync_channel(8);
        let sender = owner.clone();
        connection
            .add_match(
                MatchRule::new_signal(SESSION, "Closed").with_sender(DESKTOP),
                move |_: (), _, message| {
                    if message
                        .sender()
                        .is_some_and(|actual| actual.as_ref() == sender)
                        && let Some(path) = message.path()
                    {
                        let _ = tx.try_send(path.to_string());
                    }
                    true
                },
            )
            .map_err(error)?;
        let token = token();
        let sender = connection
            .unique_name()
            .to_string()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = Path::new(format!("{DESKTOP_PATH}/session/{sender}/{token}")).map_err(error)?;
        let mut session = Self {
            connection,
            owner,
            path,
            responses,
            closed,
            deadline,
        };
        let results = session.request("CreateSession", cancelled, |proxy, mut options| {
            options.insert("session_handle_token".into(), value(token));
            proxy.method_call(SCREENCAST, "CreateSession", (options,))
        })?;
        let path = results
            .get("session_handle")
            .and_then(|value| value.0.as_str())
            .ok_or_else(|| error("CreateSession has no session handle"))?;
        if !path.starts_with(&format!("{DESKTOP_PATH}/session/")) {
            return Err(error("CreateSession returned an invalid session handle").into());
        }
        session.path = Path::new(path.to_owned()).map_err(error)?;
        check_cancel(cancelled)?;
        let proxy = session
            .connection
            .with_proxy(&session.owner, DESKTOP_PATH, CALL_TIMEOUT);
        let version: u32 = proxy.get(SCREENCAST, "version").map_err(error)?;
        let sources: u32 = proxy
            .get(SCREENCAST, "AvailableSourceTypes")
            .map_err(error)?;
        if sources & source_type == 0 {
            let name = if source_type == 1 {
                "display"
            } else {
                "window"
            };
            return Err(error(format!("the portal cannot share a {name}")).into());
        }
        // Window streams must identify their source type, introduced in v3.
        // Do not publish a display stream under a promised window target.
        if source_type == 2 && version < 3 {
            return Err(
                error("window recording requires ScreenCast portal version 3 or later").into(),
            );
        }
        let cursor_mode = if show_cursor { 2_u32 } else { 1_u32 };
        if version >= 2 {
            let modes: u32 = proxy
                .get(SCREENCAST, "AvailableCursorModes")
                .map_err(error)?;
            if modes & cursor_mode == 0 {
                return Err(error("the portal does not support the requested cursor mode").into());
            }
        } else if show_cursor {
            return Err(error("this portal cannot embed the cursor").into());
        }
        let path = session.path.clone();
        session.request("SelectSources", cancelled, |proxy, mut options| {
            options.insert("types".into(), value(source_type));
            options.insert("multiple".into(), value(false));
            if version >= 2 {
                options.insert("cursor_mode".into(), value(cursor_mode));
            }
            // No restore token or persistent permission. Every new session asks
            // the portal; cancellation/denial ends this acquisition.
            proxy.method_call(SCREENCAST, "SelectSources", (path, options))
        })?;
        let path = session.path.clone();
        let results = session.request("Start", cancelled, |proxy, options| {
            proxy.method_call(SCREENCAST, "Start", (path, "", options))
        })?;
        let stream = selected_stream(&results, source_type)?;
        check_cancel(cancelled)?;
        let (fd,): (dbus::arg::OwnedFd,) = session
            .connection
            .with_proxy(&session.owner, DESKTOP_PATH, CALL_TIMEOUT)
            .method_call(
                SCREENCAST,
                "OpenPipeWireRemote",
                (session.path.clone(), PropMap::new()),
            )
            .map_err(error)?;
        // D-Bus 0.9 transfers this descriptor through IntoRawFd, suppressing its
        // destructor. Exactly one std OwnedFd now owns that same valid FD.
        let fd = unsafe { OwnedFd::from_raw_fd(fd.into_raw_fd()) };
        check_cancel(cancelled)?;
        Ok((session, fd, stream))
    }

    fn request(
        &self,
        method: &str,
        cancelled: &dyn Fn() -> bool,
        call: impl FnOnce(
            dbus::blocking::Proxy<'_, &Connection>,
            PropMap,
        ) -> Result<(Path<'static>,), dbus::Error>,
    ) -> Result<PropMap, PortalVideoError> {
        self.pending(cancelled)?;
        let token = token();
        let sender = self
            .connection
            .unique_name()
            .to_string()
            .trim_start_matches(':')
            .replace('.', "_");
        let expected = format!("{DESKTOP_PATH}/request/{sender}/{token}");
        let options = [("handle_token".into(), value(token))].into();
        let timeout = self
            .deadline
            .saturating_duration_since(Instant::now())
            .min(CALL_TIMEOUT);
        let handle = match call(
            self.connection
                .with_proxy(&self.owner, DESKTOP_PATH, timeout),
            options,
        ) {
            Ok((path,)) => path,
            Err(problem) => {
                self.close_request(&expected);
                return Err(error(format!("{method}: {problem}")).into());
            }
        };
        let result = (|| {
            loop {
                self.pending(cancelled)?;
                for (path, result) in self.responses.try_iter() {
                    if path == handle.as_ref() {
                        let (status, results) = result?;
                        return match status {
                            0 => Ok(results),
                            1 => Err(PortalVideoError::Cancelled),
                            _ => Err(error(format!("{method} was denied or failed")).into()),
                        };
                    }
                }
                self.connection
                    .process(POLL.min(self.deadline.saturating_duration_since(Instant::now())))
                    .map_err(error)?;
            }
        })();
        if result.is_err() {
            self.close_request(&handle);
        }
        result
    }

    fn pending(&self, cancelled: &dyn Fn() -> bool) -> Result<(), PortalVideoError> {
        check_cancel(cancelled)?;
        if Instant::now() >= self.deadline {
            return Err(error("timed out waiting for consent").into());
        }
        if self
            .closed
            .try_iter()
            .any(|path| path == self.path.as_ref())
        {
            return Err(error("the screen cast session was closed").into());
        }
        Ok(())
    }

    fn close_request(&self, path: &str) {
        let _: Result<(), _> = self
            .connection
            .with_proxy(&self.owner, path, Duration::from_secs(1))
            .method_call(REQUEST, "Close", ());
    }
}

impl Drop for PortalSession {
    fn drop(&mut self) {
        let _: Result<(), _> = self
            .connection
            .with_proxy(&self.owner, &self.path, Duration::from_secs(1))
            .method_call(SESSION, "Close", ());
    }
}

fn value(item: impl RefArg + 'static) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(item))
}
fn token() -> String {
    format!("captures_{}", uuid::Uuid::new_v4().simple())
}
fn error(item: impl std::fmt::Display) -> String {
    format!("Desktop recording portal: {item}")
}
fn check_cancel(cancelled: &dyn Fn() -> bool) -> Result<(), PortalVideoError> {
    if cancelled() {
        Err(PortalVideoError::Cancelled)
    } else {
        Ok(())
    }
}

struct Stream {
    node: u32,
    serial: Option<u64>,
}

fn selected_stream(results: &PropMap, source_type: u32) -> Result<Stream, String> {
    let mut streams = results
        .get("streams")
        .and_then(|value| value.0.as_iter())
        .ok_or_else(|| error("Start has no streams"))?;
    let first = streams
        .next()
        .ok_or_else(|| error("Start returned no stream"))?;
    if streams.next().is_some() {
        return Err(error("Start returned more than one stream"));
    }
    let mut fields = first
        .as_iter()
        .ok_or_else(|| error("invalid stream tuple"))?;
    let node = fields
        .next()
        .and_then(RefArg::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|node| *node != u32::MAX)
        .ok_or_else(|| error("invalid stream node"))?;
    let properties = fields
        .next()
        .and_then(RefArg::as_iter)
        .ok_or_else(|| error("invalid stream properties"))?;
    if fields.next().is_some() {
        return Err(error("invalid stream tuple"));
    }
    let mut properties = properties;
    let mut serial = None;
    let mut source_matched = false;
    while let Some(name) = properties.next() {
        let item = properties
            .next()
            .ok_or_else(|| error("invalid stream property"))?;
        let value = item
            .as_iter()
            .and_then(|mut values| values.next())
            .unwrap_or(item);
        match name.as_str() {
            Some("pipewire-serial") => {
                serial = Some(
                    value
                        .as_u64()
                        .filter(|serial| *serial != 0)
                        .ok_or_else(|| error("invalid stream serial"))?,
                );
            }
            Some("source_type") => {
                if value.as_u64() != Some(u64::from(source_type)) {
                    return Err(error(
                        "the selected source does not match the requested recording target",
                    ));
                }
                source_matched = true;
            }
            _ => {}
        }
    }
    if source_type == 2 && !source_matched {
        return Err(error(
            "the portal did not identify the selected window source",
        ));
    }
    Ok(Stream { node, serial })
}

/// A bounded, portal-granted video source. Dropping it stops its worker, releases
/// the PipeWire connection and closes the portal session. Call on a worker.
pub struct PortalVideoSource {
    control: pw::channel::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
    warning: Arc<Mutex<Option<String>>>,
    dropped: Arc<AtomicU64>,
}

impl PortalVideoSource {
    pub fn start(
        target: &RecordingTarget,
        show_cursor: bool,
        frame_rate: u16,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Self, Receiver<Frame>), PortalVideoError> {
        let source_type = match target {
            RecordingTarget::PortalDisplay => 1,
            RecordingTarget::PortalWindow => 2,
            _ => return Err(error("recording target is not a portal source").into()),
        };
        if !(1..=60).contains(&frame_rate) {
            return Err(error("invalid video frame rate").into());
        }
        let (session, fd, stream) = PortalSession::open(source_type, show_cursor, cancelled)?;
        let (control, commands) = pw::channel::channel();
        let (tx, frames) = mpsc::sync_channel(1);
        let warning = Arc::new(Mutex::new(None));
        let dropped = Arc::new(AtomicU64::new(0));
        let thread_warning = warning.clone();
        let thread_dropped = dropped.clone();
        let worker = thread::Builder::new()
            .name("captures-portal-video".into())
            .spawn(move || {
                if let Err(problem) = video_loop(
                    session,
                    fd,
                    stream,
                    frame_rate,
                    commands,
                    tx,
                    thread_warning.clone(),
                    thread_dropped,
                ) {
                    *thread_warning.lock() = Some(problem);
                }
            })
            .map_err(error)?;
        Ok((
            Self {
                control,
                worker: Some(worker),
                warning,
                dropped,
            },
            frames,
        ))
    }

    pub fn warning(&self) -> Option<String> {
        self.warning.lock().clone()
    }
    pub fn dropped_frames(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn stop(mut self) -> Result<(), String> {
        self.finish()
    }

    pub(crate) fn finish(&mut self) -> Result<(), String> {
        let _ = self.control.send(());
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| error("video worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for PortalVideoSource {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

struct VideoData {
    format: Option<(VideoFormat, u32, u32)>,
    frames: SyncSender<Frame>,
    dropped: Arc<AtomicU64>,
    warning: Arc<Mutex<Option<String>>>,
}

#[allow(clippy::too_many_arguments)]
fn video_loop(
    session: PortalSession,
    fd: OwnedFd,
    selected: Stream,
    frame_rate: u16,
    control: pw::channel::Receiver<()>,
    frames: SyncSender<Frame>,
    warning: Arc<Mutex<Option<String>>>,
    dropped: Arc<AtomicU64>,
) -> Result<(), String> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(error)?;
    let context = pw::context::ContextRc::new(&main_loop, None).map_err(error)?;
    let core = context.connect_fd_rc(fd, None).map_err(error)?;
    let mut properties = properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    if let Some(serial) = selected.serial {
        // Portal v6 serials cannot be reused after node destruction. Older
        // portals are confined to their granted remote and use the returned ID.
        properties.insert("target.object", serial.to_string());
    }
    let stream =
        pw::stream::StreamRc::new(core, "Captures portal video", properties).map_err(error)?;
    let quit = main_loop.clone();
    let _control = control.attach(main_loop.loop_(), move |_| quit.quit());
    let quit = main_loop.clone();
    let session_warning = warning.clone();
    let _session_poll = main_loop.loop_().add_timer(move |_| {
        let problem = session
            .connection
            .process(Duration::ZERO)
            .err()
            .map(error)
            .or_else(|| {
                session
                    .closed
                    .try_iter()
                    .any(|path| path == session.path.as_ref())
                    .then(|| error("the portal ended the recording session"))
            });
        if let Some(problem) = problem {
            *session_warning.lock() = Some(problem);
            quit.quit();
        }
    });
    _session_poll
        .update_timer(Some(POLL), Some(POLL))
        .into_result()
        .map_err(error)?;
    let quit = main_loop.clone();
    let state_warning = warning.clone();
    let format_quit = main_loop.clone();
    let process_quit = main_loop.clone();
    let _listener = stream
        .add_local_listener_with_user_data(VideoData {
            format: None,
            frames,
            dropped,
            warning,
        })
        .state_changed(move |_, data, _, state| {
            if let StreamState::Error(problem) = state {
                *state_warning.lock() = Some(error(problem));
                quit.quit();
            } else if matches!(state, StreamState::Unconnected) && data.format.is_some() {
                *state_warning.lock() = Some(error("the granted video stream disconnected"));
                quit.quit();
            }
        })
        .param_changed(move |_, data, id, param| {
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else {
                return;
            };
            let parsed = (|| {
                let (media, subtype) =
                    spa::param::format_utils::parse_format(param).map_err(error)?;
                if media != MediaType::Video || subtype != MediaSubtype::Raw {
                    return Err(error("video is not raw RGB"));
                }
                let mut info = VideoInfoRaw::new();
                info.parse(param).map_err(error)?;
                validate_format(info.format(), info.size().width, info.size().height)?;
                if data.format.is_some_and(|previous| {
                    previous != (info.format(), info.size().width, info.size().height)
                }) {
                    return Err(error("video format changed during recording"));
                }
                Ok((info.format(), info.size().width, info.size().height))
            })();
            match parsed {
                Ok(format) => data.format = Some(format),
                Err(problem) => {
                    *data.warning.lock() = Some(problem);
                    format_quit.quit();
                }
            }
        })
        .process(move |stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some((format, width, height)) = data.format else {
                return;
            };
            let decoded = (|| {
                let planes = buffer.datas_mut();
                if planes.len() != 1 {
                    return Err(error("video is not a single CPU-mapped plane"));
                }
                let plane = &mut planes[0];
                if !matches!(plane.type_(), DataType::MemPtr | DataType::MemFd) {
                    return Err(error("video is not CPU-mapped; DMA-BUF is unsupported"));
                }
                if plane.chunk().flags().contains(ChunkFlags::CORRUPTED) {
                    return Err(error("corrupted video frame"));
                }
                let offset = plane.chunk().offset() as usize;
                let size = plane.chunk().size() as usize;
                let stride = plane.chunk().stride();
                if size == 0 {
                    return Ok(None);
                } // Empty buffers do not contain a frame.
                let bytes = plane
                    .data()
                    .ok_or_else(|| error("video buffer has no CPU mapping"))?;
                decode_frame(bytes, format, width, height, offset, size, stride).map(Some)
            })();
            match decoded {
                Ok(Some(frame)) => match data.frames.try_send(Frame {
                    width: frame.width(),
                    height: frame.height(),
                    raw: frame.into_raw(),
                }) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        data.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Disconnected(_)) => process_quit.quit(),
                },
                Ok(None) => {}
                Err(problem) => {
                    *data.warning.lock() = Some(problem);
                    process_quit.quit();
                }
            }
        })
        .register()
        .map_err(error)?;
    let values: Vec<u8> = PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::EnumFormat.as_raw(),
            properties: vec![
                spa::pod::Property::new(
                    spa::param::format::FormatProperties::MediaType.as_raw(),
                    spa::pod::Value::Id(spa::utils::Id(MediaType::Video.as_raw())),
                ),
                spa::pod::Property::new(
                    spa::param::format::FormatProperties::MediaSubtype.as_raw(),
                    spa::pod::Value::Id(spa::utils::Id(MediaSubtype::Raw.as_raw())),
                ),
                spa::pod::Property::new(
                    spa::param::format::FormatProperties::VideoFormat.as_raw(),
                    spa::pod::Value::Choice(spa::pod::ChoiceValue::Id(spa::utils::Choice(
                        spa::utils::ChoiceFlags::empty(),
                        spa::utils::ChoiceEnum::Enum {
                            default: spa::utils::Id(VideoFormat::BGRx.as_raw()),
                            alternatives: [
                                VideoFormat::BGRx,
                                VideoFormat::RGBx,
                                VideoFormat::BGRA,
                                VideoFormat::RGBA,
                            ]
                            .map(|format| spa::utils::Id(format.as_raw()))
                            .to_vec(),
                        },
                    ))),
                ),
                spa::pod::Property::new(
                    spa::param::format::FormatProperties::VideoSize.as_raw(),
                    spa::pod::Value::Choice(spa::pod::ChoiceValue::Rectangle(spa::utils::Choice(
                        spa::utils::ChoiceFlags::empty(),
                        spa::utils::ChoiceEnum::Range {
                            default: spa::utils::Rectangle {
                                width: 1920,
                                height: 1080,
                            },
                            min: spa::utils::Rectangle {
                                width: 1,
                                height: 1,
                            },
                            max: spa::utils::Rectangle {
                                width: 16384,
                                height: 16384,
                            },
                        },
                    ))),
                ),
                spa::pod::Property::new(
                    spa::param::format::FormatProperties::VideoFramerate.as_raw(),
                    spa::pod::Value::Choice(spa::pod::ChoiceValue::Fraction(spa::utils::Choice(
                        spa::utils::ChoiceFlags::empty(),
                        spa::utils::ChoiceEnum::Range {
                            default: spa::utils::Fraction {
                                num: u32::from(frame_rate),
                                denom: 1,
                            },
                            min: spa::utils::Fraction { num: 0, denom: 1 },
                            max: spa::utils::Fraction {
                                num: u32::from(frame_rate),
                                denom: 1,
                            },
                        },
                    ))),
                ),
            ],
        }),
    )
    .map_err(error)?
    .0
    .into_inner();
    let mut params =
        [Pod::from_bytes(&values).ok_or_else(|| error("invalid video format parameters"))?];
    stream
        .connect(
            Direction::Input,
            selected.serial.is_none().then_some(selected.node),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::DONT_RECONNECT,
            &mut params,
        )
        .map_err(error)?;
    main_loop.run();
    // A local stop deliberately transitions to Unconnected. Remove callbacks
    // first so cleanup cannot replace a healthy stream's outcome with failure.
    drop(_listener);
    stream.disconnect().map_err(error)?;
    Ok(())
}

fn validate_format(format: VideoFormat, width: u32, height: u32) -> Result<usize, String> {
    if !matches!(
        format,
        VideoFormat::RGBA | VideoFormat::BGRA | VideoFormat::RGBx | VideoFormat::BGRx
    ) {
        return Err(error("unsupported raw video format"));
    }
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(error("invalid video dimensions"));
    }
    let size = (width as usize)
        .checked_mul(height as usize)
        .and_then(|size| size.checked_mul(4))
        .filter(|size| *size <= MAX_FRAME_BYTES)
        .ok_or_else(|| error("video frame exceeds its size limit"))?;
    Ok(size)
}

#[allow(clippy::too_many_arguments)]
fn decode_frame(
    bytes: &[u8],
    format: VideoFormat,
    width: u32,
    height: u32,
    offset: usize,
    size: usize,
    stride: i32,
) -> Result<RgbaImage, String> {
    let length = validate_format(format, width, height)?;
    let stride =
        usize::try_from(stride).map_err(|_| error("negative video stride is unsupported"))?;
    let row_bytes = width as usize * 4;
    let required = (height as usize - 1)
        .checked_mul(stride)
        .and_then(|size| size.checked_add(row_bytes))
        .ok_or_else(|| error("video row geometry overflow"))?;
    let end = offset
        .checked_add(size)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| error("video chunk escapes its mapping"))?;
    if stride < row_bytes || required > size {
        return Err(error("truncated video rows"));
    }
    let mut output = Vec::with_capacity(length);
    let source = &bytes[offset..end];
    for row in 0..height as usize {
        for pixel in source[row * stride..row * stride + row_bytes].chunks_exact(4) {
            let alpha = if matches!(format, VideoFormat::RGBx | VideoFormat::BGRx) {
                255
            } else {
                pixel[3]
            };
            if matches!(format, VideoFormat::BGRA | VideoFormat::BGRx) {
                output.extend_from_slice(&[pixel[2], pixel[1], pixel[0], alpha]);
            } else {
                output.extend_from_slice(&[pixel[0], pixel[1], pixel[2], alpha]);
            }
        }
    }
    RgbaImage::from_raw(width, height, output).ok_or_else(|| error("invalid decoded video frame"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_offset_rows_keep_channel_order_alpha_and_dimensions() {
        let bytes = [
            90, 91, 92, 3, 17, 201, 19, 8, 51, 111, 23, 99, 98, 97, 96, 41, 61, 81, 27, 53, 73, 93,
            31, 89,
        ];
        for format in [
            VideoFormat::BGRA,
            VideoFormat::BGRx,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
        ] {
            let image = decode_frame(&bytes, format, 2, 2, 3, 20, 12).unwrap();
            assert_eq!(image.dimensions(), (2, 2));
            let expected = match format {
                VideoFormat::BGRA => vec![
                    201, 17, 3, 19, 111, 51, 8, 23, 81, 61, 41, 27, 93, 73, 53, 31,
                ],
                VideoFormat::BGRx => vec![
                    201, 17, 3, 255, 111, 51, 8, 255, 81, 61, 41, 255, 93, 73, 53, 255,
                ],
                VideoFormat::RGBA => vec![
                    3, 17, 201, 19, 8, 51, 111, 23, 41, 61, 81, 27, 53, 73, 93, 31,
                ],
                _ => vec![
                    3, 17, 201, 255, 8, 51, 111, 255, 41, 61, 81, 255, 53, 73, 93, 255,
                ],
            };
            assert_eq!(image.into_raw(), expected);
        }
    }

    #[test]
    fn invalid_geometry_never_interprets_padding_or_unmapped_bytes_as_pixels() {
        for (offset, size, stride) in [
            (3, 19, 12),
            (3, 22, 12),
            (usize::MAX, 20, 12),
            (3, 20, 7),
            (3, 20, -12),
        ] {
            assert!(decode_frame(&[0; 24], VideoFormat::BGRx, 2, 2, offset, size, stride).is_err());
        }
        assert!(validate_format(VideoFormat::RGB, 2, 2).is_err());
        assert!(validate_format(VideoFormat::RGBA, 0, 2).is_err());
        assert!(validate_format(VideoFormat::RGBA, 16384, 16384).is_err());
    }

    #[test]
    fn selected_stream_preserves_target_type_and_rejects_ambiguous_windows() {
        let results = |streams: Vec<(u32, PropMap)>| [("streams".into(), value(streams))].into();
        assert!(selected_stream(&results(vec![]), 1).is_err());
        assert!(
            selected_stream(
                &results(vec![(13, PropMap::new()), (37, PropMap::new())]),
                1
            )
            .is_err()
        );
        assert!(selected_stream(&results(vec![(u32::MAX, PropMap::new())]), 1).is_err());
        // Older display-only portals omit source_type; window grants cannot.
        assert!(selected_stream(&results(vec![(13, PropMap::new())]), 1).is_ok());
        assert!(selected_stream(&results(vec![(13, PropMap::new())]), 2).is_err());
        for returned in [1_u32, 2, 4] {
            for requested in [1, 2] {
                let properties = [
                    ("source_type".into(), value(returned)),
                    ("pipewire-serial".into(), value(987654321_u64)),
                ]
                .into();
                let stream = selected_stream(&results(vec![(37, properties)]), requested);
                if returned == requested {
                    let stream = stream.unwrap();
                    assert_eq!(stream.node, 37);
                    assert_eq!(stream.serial, Some(987654321));
                } else {
                    assert!(
                        stream.is_err(),
                        "requested {requested}, returned {returned}"
                    );
                }
            }
        }
    }
}
