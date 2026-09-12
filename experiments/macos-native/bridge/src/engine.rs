use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use captures_capture::{
    PointerCursor, XcapBackend, overlay_pointer_cursor, overlay_pointer_cursor_in_crop,
    overlay_pointer_cursor_on_window, screenshot_pointer_scale,
};
use captures_media::{
    AudioEdit, CancelToken, EditSpec, ExportFormat, ExportSpec, MediaToolchain,
    RecordingAudioLayout, RecordingSegmentInput,
};
use captures_recording::{
    RecordingDraftManifest, RecordingKind, RecordingOptions, RecordingSegmentInfo, RecordingState,
};
use captures_recording_macos::MacRecordingSegment;
use serde_json::{Value, json};

use crate::{
    protocol::{
        self, BridgeResult, DescribeRequest, Envelope, FreezeCreateRequest, FreezeDiscardRequest,
        ImageEncodeRequest, ImageFormat, MediaExportRequest, MediaPathRequest,
        MicrophonePermissionRequest, RecordMuteRequest, RecordStartRequest, RecoverDiscardRequest,
        RecoverRequest, ScreenshotRequest, ScreenshotTarget, failure_json, success_json,
    },
    storage::{
        self, Draft, PendingSegment, checked_draft_file, complete_segment, create_draft,
        publish_exact, publish_unique, register_pending_segment, save_draft,
        stage_for_exact_output, stage_output,
    },
};

struct Job {
    request: String,
    response: mpsc::SyncSender<String>,
}

static ENGINE: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
static RECOVERY_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn request(request: String) -> String {
    let direct = catch_unwind(AssertUnwindSafe(|| dispatch_direct(&request)))
        .unwrap_or_else(|_| Some(Err("bridge operation panicked".to_owned())));
    if let Some(result) = direct {
        return match result {
            Ok(value) => success_json(value),
            Err(error) => failure_json(&error),
        };
    }
    request_engine(request)
}

fn request_engine(request: String) -> String {
    let sender = ENGINE.get_or_init(start_worker);
    let (response_sender, response_receiver) = mpsc::sync_channel(1);
    if sender
        .send(Job {
            request,
            response: response_sender,
        })
        .is_err()
    {
        return failure_json("capture engine is unavailable");
    }
    response_receiver
        .recv()
        .unwrap_or_else(|_| failure_json("capture engine stopped before replying"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationLane {
    RecordingEngine,
    Stateless,
    Recovery,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MicrophonePermissionOperation {
    Authorized,
    Request,
    NotDetermined,
    Denied,
}

#[cfg(any(target_os = "macos", test))]
const fn microphone_permission_operation(
    authorized: bool,
    can_request: bool,
    request: bool,
) -> MicrophonePermissionOperation {
    if authorized {
        MicrophonePermissionOperation::Authorized
    } else if can_request && request {
        MicrophonePermissionOperation::Request
    } else if can_request {
        MicrophonePermissionOperation::NotDetermined
    } else {
        MicrophonePermissionOperation::Denied
    }
}

fn operation_lane(operation: &str) -> OperationLane {
    match operation {
        "image_encode"
        | "media_probe"
        | "media_export"
        | "recover_list"
        | "microphone_permission" => OperationLane::Stateless,
        "recover" | "recover_discard" => OperationLane::Recovery,
        _ => OperationLane::RecordingEngine,
    }
}

fn dispatch_direct(request: &str) -> Option<BridgeResult<Value>> {
    let value: Value = match serde_json::from_str(request) {
        Ok(value) => value,
        Err(error) => {
            return Some(Err(format!("request is not valid JSON: {error}")));
        }
    };
    let envelope: Envelope = match serde_json::from_value(value.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            return Some(Err(if value.get("op").is_none() {
                "request field 'op' is required".to_owned()
            } else {
                format!("invalid request envelope: {error}")
            }));
        }
    };
    match operation_lane(&envelope.op) {
        OperationLane::RecordingEngine => None,
        OperationLane::Stateless => Some(match envelope.op.as_str() {
            "image_encode" => protocol::parse(&value).and_then(Engine::image_encode),
            "media_probe" => protocol::parse(&value).and_then(Engine::media_probe),
            "media_export" => protocol::parse(&value).and_then(Engine::media_export),
            "microphone_permission" => {
                protocol::parse(&value).and_then(Engine::microphone_permission)
            }
            "recover_list" => Engine::recover_list(),
            _ => unreachable!(),
        }),
        OperationLane::Recovery => Some(with_recovery_reservation(|| match envelope.op.as_str() {
            "recover" => Engine::recover(protocol::parse(&value)?),
            "recover_discard" => Engine::recover_discard(protocol::parse(&value)?),
            _ => unreachable!(),
        })),
    }
}

fn with_recovery_reservation<T>(work: impl FnOnce() -> BridgeResult<T>) -> BridgeResult<T> {
    let _reservation = Reservation::acquire(&RECOVERY_ACTIVE)?;
    let status: Value =
        serde_json::from_str(&request_engine(r#"{"op":"record_status"}"#.to_owned()))
            .map_err(|error| format!("could not query recording state: {error}"))?;
    ensure_recovery_idle(&status)?;
    work()
}

fn ensure_recovery_idle(status: &Value) -> BridgeResult<()> {
    if status["ok"] == true && status["value"]["state"] == "idle" {
        Ok(())
    } else {
        Err("recording recovery is unavailable while a recording session is active".to_owned())
    }
}

struct Reservation<'a>(&'a AtomicBool);

impl<'a> Reservation<'a> {
    fn acquire(flag: &'a AtomicBool) -> BridgeResult<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self(flag))
            .map_err(|_| "another recording recovery operation is already active".to_owned())
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn start_worker() -> mpsc::Sender<Job> {
    let (sender, receiver) = mpsc::channel::<Job>();
    thread::Builder::new()
        .name("captures-native-engine".to_owned())
        .spawn(move || worker(receiver))
        .expect("capture engine thread can start");
    sender
}

fn worker(receiver: mpsc::Receiver<Job>) {
    let mut engine = Engine::default();
    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(job) => {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    engine.safety_tick();
                    engine.dispatch(&job.request)
                }))
                .unwrap_or_else(|_| Err("capture engine operation panicked".to_owned()));
                let response = match result {
                    Ok(value) => success_json(value),
                    Err(error) => failure_json(&error),
                };
                let _ = job.response.send(response);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => engine.safety_tick(),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[derive(Default)]
struct Engine {
    recording: Option<RecordingSession>,
}

struct RecordingSession {
    options: RecordingOptions,
    exclude_app: bool,
    output_directory: PathBuf,
    draft: Draft,
    active: Option<MacRecordingSegment>,
    segments: Vec<RecordingSegmentInfo>,
    lifecycle: Lifecycle,
    warning: Option<String>,
}

#[derive(Debug)]
struct Lifecycle {
    state: RecordingState,
    elapsed_ms: u64,
    active_since: Option<Instant>,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            state: RecordingState::Selecting,
            elapsed_ms: 0,
            active_since: None,
        }
    }
}

impl Lifecycle {
    fn begin(&mut self) -> BridgeResult<()> {
        if !matches!(
            self.state,
            RecordingState::Selecting | RecordingState::Paused
        ) {
            return Err(format!(
                "cannot start a segment while recording is {:?}",
                self.state
            ));
        }
        self.state = RecordingState::Recording;
        self.active_since = Some(Instant::now());
        Ok(())
    }

    fn pause(&mut self, segment_duration_ms: u64) -> BridgeResult<()> {
        if self.state != RecordingState::Recording {
            return Err(format!("cannot pause while recording is {:?}", self.state));
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(segment_duration_ms);
        self.active_since = None;
        self.state = RecordingState::Paused;
        Ok(())
    }

    fn restart(&mut self) {
        self.state = RecordingState::Selecting;
        self.elapsed_ms = 0;
        self.active_since = None;
    }

    fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms.saturating_add(
            self.active_since
                .map_or(0, |started| duration_ms(started.elapsed())),
        )
    }
}

impl Engine {
    fn dispatch(&mut self, request: &str) -> BridgeResult<Value> {
        let value: Value = serde_json::from_str(request)
            .map_err(|error| format!("request is not valid JSON: {error}"))?;
        let envelope: Envelope = serde_json::from_value(value.clone()).map_err(|error| {
            if value.get("op").is_none() {
                "request field 'op' is required".to_owned()
            } else {
                format!("invalid request envelope: {error}")
            }
        })?;
        match envelope.op.as_str() {
            "describe" => self.describe(protocol::parse(&value)?),
            "freeze_create" => self.freeze_create(protocol::parse(&value)?),
            "freeze_discard" => Self::freeze_discard(protocol::parse(&value)?),
            "screenshot" => self.screenshot(protocol::parse(&value)?),
            "record_start" => self.record_start(protocol::parse(&value)?),
            "record_pause" => self.record_pause(),
            "record_resume" => self.record_resume(),
            "record_restart" => self.record_restart(),
            "record_mute" => self.record_mute(protocol::parse(&value)?),
            "record_status" => self.record_status(),
            "session_status" => Ok(Self::session_status()),
            "record_stop" => self.record_stop(),
            "record_discard" => self.record_discard(),
            unknown => Err(format!("unknown operation '{unknown}'")),
        }
    }

    fn describe(&self, request: DescribeRequest) -> BridgeResult<Value> {
        if !captures_session::capture_session_available() {
            return Err(
                "capture discovery is unavailable while the console is locked or inactive"
                    .to_owned(),
            );
        }
        let backend = XcapBackend;
        backend
            .ensure_permission(request.request_permission)
            .map_err(|error| error.to_string())?;
        let displays = backend.displays().map_err(|error| error.to_string())?;
        let windows = backend.windows().map_err(|error| error.to_string())?;
        let devices = captures_recording_macos::microphone_devices();
        Ok(json!({
            "displays": displays,
            "windows": windows,
            "devices": devices,
        }))
    }

    fn microphone_permission(request: MicrophonePermissionRequest) -> BridgeResult<Value> {
        let status = microphone_permission_status(request.request);
        Ok(json!({
            "status": status,
            "devices": captures_recording_macos::microphone_devices(),
        }))
    }

    fn screenshot(&self, request: ScreenshotRequest) -> BridgeResult<Value> {
        if !captures_session::capture_session_available() {
            return Err(
                "screen capture is unavailable while the console is locked or inactive".to_owned(),
            );
        }
        let backend = XcapBackend;
        backend
            .ensure_permission(false)
            .map_err(|error| error.to_string())?;
        let cursor = request.cursor.then(pointer_cursor).flatten();
        if request.cursor && matches!(request.target, ScreenshotTarget::FrozenRegion { .. }) {
            return Err(
                "cursor for a frozen region must be selected by freeze_create, not screenshot"
                    .to_owned(),
            );
        }
        let (image, consumed_freeze) = match &request.target {
            ScreenshotTarget::Display { display_id } => {
                let mut frame = backend
                    .capture_display(display_id)
                    .map_err(|error| error.to_string())?;
                if let Some(cursor) = &cursor {
                    let pointer_scale = screenshot_pointer_scale(frame.descriptor.scale_factor);
                    let descriptor = frame.descriptor.clone();
                    overlay_pointer_cursor(&mut frame.image, &descriptor, cursor, pointer_scale);
                }
                (frame.image, None)
            }
            ScreenshotTarget::Region { display_id, rect } => {
                let frame = backend
                    .capture_display(display_id)
                    .map_err(|error| error.to_string())?;
                let scale = frame
                    .descriptor
                    .overlay_to_buffer_scale(frame.image.width(), frame.image.height());
                let crop = rect.to_physical(scale, frame.image.width(), frame.image.height());
                let mut image = frame.crop(crop).ok_or("screenshot region is empty")?;
                if let Some(cursor) = &cursor {
                    overlay_pointer_cursor_in_crop(
                        &mut image,
                        &frame.descriptor,
                        crop.x,
                        crop.y,
                        frame.image.width(),
                        frame.image.height(),
                        cursor,
                        screenshot_pointer_scale(frame.descriptor.scale_factor),
                    );
                }
                (image, None)
            }
            ScreenshotTarget::Window { window_id } => {
                let window = if cursor.is_some() {
                    backend
                        .windows()
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .find(|window| window.id == *window_id)
                } else {
                    None
                };
                let mut image = backend
                    .capture_window(window_id)
                    .map_err(|error| error.to_string())?;
                if let (Some(cursor), Some(window)) = (&cursor, window) {
                    overlay_pointer_cursor_on_window(&mut image, &window, cursor, 1.0);
                }
                (image, None)
            }
            ScreenshotTarget::FrozenRegion { freeze_id, rect } => {
                let (_directory, image, display) = storage::load_freeze(freeze_id)?;
                let scale = display.overlay_to_buffer_scale(image.width(), image.height());
                let crop = rect.to_physical(scale, image.width(), image.height());
                if crop.width == 0 || crop.height == 0 {
                    return Err("screenshot region is empty".to_owned());
                }
                let image =
                    image::imageops::crop_imm(&image, crop.x, crop.y, crop.width, crop.height)
                        .to_image();
                (image, Some(freeze_id.clone()))
            }
        };
        let (width, height) = image.dimensions();
        let path = storage::save_screenshot(&request.output_dir, &image, &request.target)?;
        if let Some(freeze_id) = consumed_freeze {
            // Publication has succeeded and must not be reported as failed if
            // private cleanup encounters a transient filesystem error. Stale
            // owned freezes are pruned by the next freeze_create.
            let _ = storage::discard_freeze(&freeze_id);
        }
        Ok(artifact(path, width, height, "image"))
    }

    fn freeze_create(&self, request: FreezeCreateRequest) -> BridgeResult<Value> {
        if !captures_session::capture_session_available() {
            return Err(
                "screen capture is unavailable while the console is locked or inactive".to_owned(),
            );
        }
        let backend = XcapBackend;
        backend
            .ensure_permission(false)
            .map_err(|error| error.to_string())?;
        let mut frame = backend
            .capture_display(&request.display_id)
            .map_err(|error| error.to_string())?;
        if request.cursor
            && let Some(cursor) = pointer_cursor()
        {
            let pointer_scale = screenshot_pointer_scale(frame.descriptor.scale_factor);
            let descriptor = frame.descriptor.clone();
            overlay_pointer_cursor(&mut frame.image, &descriptor, &cursor, pointer_scale);
        }
        let (width, height) = frame.image.dimensions();
        let (id, path) = storage::save_freeze(&frame.image, frame.descriptor)?;
        Ok(json!({
            "id": id,
            "path": path.to_string_lossy(),
            "width": width,
            "height": height,
        }))
    }

    fn freeze_discard(request: FreezeDiscardRequest) -> BridgeResult<Value> {
        storage::discard_freeze(&request.id)?;
        Ok(json!({}))
    }

    fn record_start(&mut self, request: RecordStartRequest) -> BridgeResult<Value> {
        if RECOVERY_ACTIVE.load(Ordering::Acquire) {
            return Err("recording cannot start while recording recovery is active".to_owned());
        }
        if self.recording.is_some() {
            return Err("a recording is already active".to_owned());
        }
        request.options.validate().map_err(str::to_owned)?;
        // TCC prompts belong to the independent permission lane, never the
        // lifecycle worker. Revalidate saved selections without waiting for UI.
        if request.options.audio.microphone_device_id.is_some()
            && microphone_permission_status(false) != "authorized"
        {
            return Err("Microphone access is required for the selected input. Open Preferences > Recording and allow microphone access; if denied, open Microphone Settings there. Then start the recording again.".to_owned());
        }
        if !captures_session::capture_session_available() {
            return Err(
                "screen recording is unavailable while the console is locked or inactive"
                    .to_owned(),
            );
        }
        let draft = create_draft(&request.options)?;
        let mut session = RecordingSession {
            options: request.options,
            exclude_app: request.exclude_app,
            output_directory: request.output_dir,
            draft,
            active: None,
            segments: Vec::new(),
            lifecycle: Lifecycle::default(),
            warning: None,
        };
        if let Err(error) = session.begin_segment() {
            let _ = session
                .draft
                .store
                .remove(&session.draft.manifest.session_id);
            return Err(error);
        }
        let status = session.status();
        self.recording = Some(session);
        Ok(status)
    }

    fn record_pause(&mut self) -> BridgeResult<Value> {
        let session = self.recording_mut()?;
        session.pause_segment(None)?;
        Ok(session.status())
    }

    fn record_resume(&mut self) -> BridgeResult<Value> {
        let session = self.recording_mut()?;
        session.begin_segment()?;
        Ok(session.status())
    }

    fn record_restart(&mut self) -> BridgeResult<Value> {
        let session = self.recording_mut()?;
        if let Some(active) = session.active.take() {
            active.discard().map_err(|error| error.to_string())?;
        }
        let old_id = session.draft.manifest.session_id.clone();
        session
            .draft
            .store
            .remove(&old_id)
            .map_err(|error| error.to_string())?;
        session.draft = create_draft(&session.options)?;
        session.segments.clear();
        session.lifecycle.restart();
        session.warning = None;
        // Restart is a two-phase operation. The frontend owns the visible,
        // cancellable countdown and calls record_resume only after it reaches
        // zero. Keeping the engine in Selecting prevents frames from being
        // recorded behind that countdown.
        Ok(session.status())
    }

    fn record_mute(&mut self, request: RecordMuteRequest) -> BridgeResult<Value> {
        let session = self.recording_mut()?;
        if session.options.audio.microphone_device_id.is_none() {
            return Err("recording has no microphone to mute".to_owned());
        }
        if session.options.audio.microphone_muted == request.muted {
            return Ok(session.status());
        }
        let resume = session.active.is_some();
        if resume {
            session.pause_segment(None)?;
        }
        session.options.audio.microphone_muted = request.muted;
        session.draft.manifest.options = session.options.clone();
        save_draft(&mut session.draft)?;
        if resume {
            session.begin_segment()?;
        }
        Ok(session.status())
    }

    fn record_status(&self) -> BridgeResult<Value> {
        Ok(self.recording.as_ref().map_or_else(
            || {
                json!({
                    "state": "idle",
                    "elapsed_ms": 0,
                    "microphone_level": 0.0,
                    "microphone_muted": false,
                })
            },
            RecordingSession::status,
        ))
    }

    fn session_status() -> Value {
        json!({ "available": captures_session::capture_session_available() })
    }

    fn record_stop(&mut self) -> BridgeResult<Value> {
        let mut session = self.recording.take().ok_or("no recording is active")?;
        if session.active.is_some()
            && let Err(error) = session.pause_segment(None)
        {
            self.recording = Some(session);
            return Err(error);
        }
        session.lifecycle.state = RecordingState::Finalizing;
        session.draft.manifest.state = RecordingState::Finalizing;
        save_draft(&mut session.draft)?;
        let result = session.assemble_and_publish();
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                session.lifecycle.state = RecordingState::Failed;
                session.draft.manifest.state = RecordingState::Failed;
                session.draft.manifest.last_error = Some(error.clone());
                let _ = save_draft(&mut session.draft);
                self.recording = Some(session);
                Err(format!("{error}; recoverable draft was retained"))
            }
        }
    }

    fn record_discard(&mut self) -> BridgeResult<Value> {
        let mut session = self.recording.take().ok_or("no recording is active")?;
        if let Some(active) = session.active.take() {
            active.discard().map_err(|error| error.to_string())?;
        }
        session
            .draft
            .store
            .remove(&session.draft.manifest.session_id)
            .map_err(|error| error.to_string())?;
        Ok(json!({}))
    }

    fn media_probe(request: MediaPathRequest) -> BridgeResult<Value> {
        ensure_regular_source(&request.path)?;
        let probe = media_toolchain()
            .probe(&request.path)
            .map_err(|error| error.to_string())?;
        Ok(json!({
            "duration_ms": probe.metadata.duration_ms.unwrap_or(0),
            "width": probe.metadata.width,
            "height": probe.metadata.height,
        }))
    }

    fn image_encode(request: ImageEncodeRequest) -> BridgeResult<Value> {
        ensure_regular_source(&request.path)?;
        if request.output.exists() {
            return Err(format!(
                "image destination already exists: {}",
                request.output.display()
            ));
        }
        let image = image::open(&request.path)
            .map_err(|error| format!("image source is unreadable: {error}"))?
            .into_rgba8();
        let bytes = crate::image_encoder::encode(
            &image,
            request.format,
            request.quality,
            request.max_bytes,
        )?;
        let extension = match request.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        };
        let staged = stage_for_exact_output(&request.output, extension)?;
        fs::write(&staged.path, bytes).map_err(|error| error.to_string())?;
        let path = publish_exact(&staged, &request.output)?;
        Ok(artifact(path, image.width(), image.height(), "image"))
    }

    fn media_export(request: MediaExportRequest) -> BridgeResult<Value> {
        ensure_regular_source(&request.path)?;
        if request.format == ExportFormat::WebM {
            return Err(
                "WebM export is not supported by the Captures native media backend".to_owned(),
            );
        }
        if request.fps.is_some() && request.format != ExportFormat::Gif {
            return Err("fps is accepted only for GIF media export".to_owned());
        }
        if request.fps.is_some_and(|fps| !(1..=30).contains(&fps)) {
            return Err("GIF export fps must be between 1 and 30".to_owned());
        }
        if request.max_bytes == Some(0) {
            return Err("media export max_bytes must be greater than zero".to_owned());
        }
        if request.output.exists() {
            return Err(format!(
                "export destination already exists: {}",
                request.output.display()
            ));
        }
        if !(request.system_volume.is_finite()
            && request.microphone_volume.is_finite()
            && (0.0..=2.0).contains(&request.system_volume)
            && (0.0..=2.0).contains(&request.microphone_volume))
        {
            return Err("audio volume must be a finite multiplier between 0 and 2".to_owned());
        }
        let media = media_toolchain();
        let probe = media
            .probe(&request.path)
            .map_err(|error| error.to_string())?;
        let (output_width, output_height) = scaled_dimensions(
            request.width,
            request.crop,
            probe.metadata.width,
            probe.metadata.height,
        )?;
        let edit = EditSpec {
            trim_start_ms: request.start_ms,
            trim_end_ms: Some(request.end_ms),
            crop: request.crop,
            output_width,
            output_height,
            audio: AudioEdit {
                system_volume: request.system_volume,
                microphone_volume: request.microphone_volume,
                mute_system_audio: request.system_volume == 0.0,
                mute_microphone: request.microphone_volume == 0.0,
                mono_output: request.mono,
                source_has_system_audio: probe.audio_stream_count >= 1,
                source_has_microphone_audio: probe.audio_stream_count >= 2,
            },
        };
        let spec = ExportSpec {
            format: request.format,
            quality: request.quality,
            max_size_bytes: request.max_bytes,
            frames_per_second: request.fps,
            gif_max_colors: None,
        };
        let extension = extension_for_format(request.format)?;
        let staged = stage_for_exact_output(&request.output, extension)?;
        media
            .export(
                &request.path,
                &staged.path,
                &edit,
                &spec,
                &CancelToken::default(),
                |_| {},
            )
            .map_err(|error| error.to_string())?;
        let output_probe = media
            .probe(&staged.path)
            .map_err(|error| error.to_string())?;
        let path = publish_exact(&staged, &request.output)?;
        Ok(artifact(
            path,
            output_probe.metadata.width,
            output_probe.metadata.height,
            kind_for_format(request.format),
        ))
    }

    fn recover_list() -> BridgeResult<Value> {
        let store = storage::draft_store()?;
        let drafts = store
            .list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|manifest| {
                if manifest.state == RecordingState::Ready && manifest.final_path.is_some() {
                    let _ = store.remove(&manifest.session_id);
                    false
                } else {
                    true
                }
            })
            .map(|manifest| {
                json!({
                    "id": manifest.session_id,
                    "name": format!("Recording {}", manifest.created_at_ms),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "drafts": drafts }))
    }

    fn recover(request: RecoverRequest) -> BridgeResult<Value> {
        let store = storage::draft_store()?;
        let mut manifest = store.load(&request.id).map_err(|error| error.to_string())?;
        let result = recover_manifest(&store, &manifest, &request.output_dir);
        match result {
            Ok(value) => {
                manifest.state = RecordingState::Ready;
                manifest.final_path = value["path"].as_str().map(str::to_owned);
                manifest.updated_at_ms = storage::now_ms();
                let _ = store.save(&manifest);
                let _ = store.remove(&request.id);
                Ok(value)
            }
            Err(error) => {
                manifest.state = RecordingState::Failed;
                manifest.last_error = Some(error.clone());
                manifest.updated_at_ms = storage::now_ms();
                let _ = store.save(&manifest);
                Err(error)
            }
        }
    }

    fn recover_discard(request: RecoverDiscardRequest) -> BridgeResult<Value> {
        storage::draft_store()?
            .remove(&request.id)
            .map_err(|error| error.to_string())?;
        Ok(json!({}))
    }

    fn recording_mut(&mut self) -> BridgeResult<&mut RecordingSession> {
        self.recording
            .as_mut()
            .ok_or("no recording is active".to_owned())
    }

    fn safety_tick(&mut self) {
        let Some(session) = self.recording.as_mut() else {
            return;
        };
        if session.active.is_none() || captures_session::capture_session_available() {
            return;
        }
        let warning = "recording auto-paused because the console became locked or inactive";
        if let Err(error) = session.pause_segment(Some(warning.to_owned())) {
            session.warning = Some(format!("{warning}; segment finalization failed: {error}"));
            session.lifecycle.state = RecordingState::Failed;
            session.draft.manifest.state = RecordingState::Failed;
            session
                .draft
                .manifest
                .last_error
                .clone_from(&session.warning);
            let _ = save_draft(&mut session.draft);
        }
    }
}

#[cfg(target_os = "macos")]
fn microphone_permission_status(request: bool) -> &'static str {
    use captures_recording_macos::{
        microphone_authorized, microphone_can_request, request_microphone_access,
    };
    match microphone_permission_operation(
        microphone_authorized(),
        microphone_can_request(),
        request,
    ) {
        MicrophonePermissionOperation::Authorized => "authorized",
        MicrophonePermissionOperation::Request if request_microphone_access() => "authorized",
        MicrophonePermissionOperation::NotDetermined => "not_determined",
        MicrophonePermissionOperation::Request | MicrophonePermissionOperation::Denied => "denied",
    }
}

#[cfg(not(target_os = "macos"))]
const fn microphone_permission_status(_request: bool) -> &'static str {
    "unavailable"
}

impl RecordingSession {
    fn begin_segment(&mut self) -> BridgeResult<()> {
        if !captures_session::capture_session_available() {
            return Err(
                "screen recording is unavailable while the console is locked or inactive"
                    .to_owned(),
            );
        }
        let index = self.draft.manifest.segments.len();
        let path = self.draft.directory.join(format!("segment-{index:03}.mp4"));
        let segment = MacRecordingSegment::start(&self.options, &path, self.exclude_app)
            .map_err(|error| error.to_string())?;
        let (width, height) = segment.dimensions();
        let pending = PendingSegment {
            path,
            system_audio: segment.system_audio_draft_info(),
            microphone: segment.microphone_draft_info(),
            width,
            height,
        };
        if let Err(error) = register_pending_segment(&mut self.draft, &pending) {
            let _ = segment.discard();
            return Err(error);
        }
        self.lifecycle.begin()?;
        self.active = Some(segment);
        self.warning = None;
        Ok(())
    }

    fn pause_segment(&mut self, warning: Option<String>) -> BridgeResult<()> {
        let segment = self.active.take().ok_or("recording is already paused")?;
        let info = segment.stop().map_err(|error| error.to_string())?;
        self.lifecycle.pause(info.duration_ms)?;
        self.segments.push(info.clone());
        self.warning = warning;
        complete_segment(&mut self.draft, &info)
    }

    fn status(&self) -> Value {
        let warning = self
            .active
            .as_ref()
            .and_then(MacRecordingSegment::warning)
            .or_else(|| self.warning.clone());
        let mut status = json!({
            "state": state_name(self.lifecycle.state),
            "elapsed_ms": self.lifecycle.elapsed_ms(),
            "microphone_muted": self.options.audio.microphone_muted,
            "microphone_level": self
                .active
                .as_ref()
                .map_or(0.0, MacRecordingSegment::microphone_level),
        });
        if let Some(warning) = warning {
            status["warning"] = Value::String(warning);
        }
        status
    }

    fn assemble_and_publish(&mut self) -> BridgeResult<Value> {
        if self.segments.is_empty() {
            return Err("recording contains no finalized media segments".to_owned());
        }
        let format = if self.options.kind == RecordingKind::Gif {
            ExportFormat::Gif
        } else {
            ExportFormat::Mp4
        };
        let extension = extension_for_format(format)?;
        let staged = stage_output(&self.output_directory, extension)?;
        assemble_segments(&self.options, &self.segments, &staged.path)?;
        let media = media_toolchain();
        let probe = media
            .probe(&staged.path)
            .map_err(|error| error.to_string())?;
        let path = publish_unique(&staged, &self.output_directory, extension)?;
        self.draft.manifest.state = RecordingState::Ready;
        self.draft.manifest.final_path = Some(path.to_string_lossy().into_owned());
        let _ = save_draft(&mut self.draft);
        let _ = self.draft.store.remove(&self.draft.manifest.session_id);
        Ok(artifact(
            path,
            probe.metadata.width,
            probe.metadata.height,
            kind_for_format(format),
        ))
    }
}

fn recover_manifest(
    store: &captures_recording::DraftStore,
    manifest: &RecordingDraftManifest,
    output_directory: &Path,
) -> BridgeResult<Value> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "recording draft schema {} is unsupported",
            manifest.schema_version
        ));
    }
    let directory = store
        .session_directory(&manifest.session_id)
        .map_err(|error| error.to_string())?;
    let mut completed = Vec::new();
    let mut saw_incomplete = false;
    for segment in &manifest.segments {
        if segment.complete && saw_incomplete {
            return Err(
                "recording draft has a complete segment after an incomplete segment".to_owned(),
            );
        }
        if !segment.complete {
            saw_incomplete = true;
            continue;
        }
        if segment.index != u32::try_from(completed.len()).unwrap_or(u32::MAX) {
            return Err("recording draft segment indexes are not contiguous".to_owned());
        }
        completed.push(RecordingSegmentInfo {
            path: checked_draft_file(&directory, &segment.relative_path)?,
            system_audio_path: segment
                .system_audio_relative_path
                .as_deref()
                .map(|path| checked_draft_file(&directory, path))
                .transpose()?,
            system_audio_offset_ms: segment.system_audio_offset_ms,
            system_audio_warning: segment.system_audio_warning.clone(),
            microphone_path: segment
                .microphone_relative_path
                .as_deref()
                .map(|path| checked_draft_file(&directory, path))
                .transpose()?,
            microphone_offset_ms: segment.microphone_offset_ms,
            microphone_warning: segment.microphone_warning.clone(),
            width: segment.width,
            height: segment.height,
            duration_ms: segment.duration_ms,
            size_bytes: segment.size_bytes,
            dropped_frames: segment.dropped_frames,
        });
    }
    if completed.is_empty() {
        return Err("recording draft has no finalized media segments".to_owned());
    }
    let format = if manifest.options.kind == RecordingKind::Gif {
        ExportFormat::Gif
    } else {
        ExportFormat::Mp4
    };
    let extension = extension_for_format(format)?;
    let staged = stage_output(output_directory, extension)?;
    assemble_segments(&manifest.options, &completed, &staged.path)?;
    let probe = media_toolchain()
        .probe(&staged.path)
        .map_err(|error| error.to_string())?;
    let path = publish_unique(&staged, output_directory, extension)?;
    Ok(artifact(
        path,
        probe.metadata.width,
        probe.metadata.height,
        kind_for_format(format),
    ))
}

fn assemble_segments(
    options: &RecordingOptions,
    segments: &[RecordingSegmentInfo],
    destination: &Path,
) -> BridgeResult<()> {
    let media = media_toolchain();
    media.verify().map_err(|error| error.to_string())?;
    let inputs = segments
        .iter()
        .map(|segment| RecordingSegmentInput {
            video_path: segment.path.clone(),
            system_audio_path: segment.system_audio_path.clone(),
            system_audio_offset_ms: segment.system_audio_offset_ms,
            microphone_path: segment.microphone_path.clone(),
            microphone_offset_ms: segment.microphone_offset_ms,
            duration_ms: segment.duration_ms,
        })
        .collect::<Vec<_>>();
    let cancel = CancelToken::default();
    if options.kind == RecordingKind::Gif {
        let master = destination.with_extension("master.mp4");
        let result = media
            .assemble_recording_segments(&inputs, &master, RecordingAudioLayout::default(), &cancel)
            .and_then(|()| {
                media.create_gif(
                    &master,
                    destination,
                    options.frames_per_second,
                    options.gif.max_width,
                    options.gif.max_colors,
                    &cancel,
                )
            });
        let _ = fs::remove_file(master);
        result.map_err(|error| error.to_string())
    } else {
        media
            .assemble_recording_segments(
                &inputs,
                destination,
                RecordingAudioLayout {
                    system_audio: options.audio.capture_system_audio,
                    microphone_audio: segments
                        .iter()
                        .any(|segment| segment.microphone_path.is_some()),
                },
                &cancel,
            )
            .map_err(|error| error.to_string())
    }
}

fn media_toolchain() -> MediaToolchain {
    let ffmpeg = std::env::var_os("CAPTURES_NATIVE_FFMPEG")
        .map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from);
    let ffprobe = std::env::var_os("CAPTURES_NATIVE_FFPROBE")
        .map_or_else(|| PathBuf::from("ffprobe"), PathBuf::from);
    MediaToolchain::new(ffmpeg, ffprobe)
}

fn ensure_regular_source(path: &Path) -> BridgeResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("media source must be a regular file, not a symbolic link".to_owned());
    }
    Ok(())
}

fn scaled_dimensions(
    requested_width: Option<u32>,
    crop: Option<captures_media::CropRect>,
    source_width: u32,
    source_height: u32,
) -> BridgeResult<(Option<u32>, Option<u32>)> {
    let Some(width) = requested_width else {
        return Ok((None, None));
    };
    if width < 2 {
        return Err("export width must be at least two pixels".to_owned());
    }
    let (base_width, base_height) = crop.map_or((source_width, source_height), |crop| {
        (crop.width, crop.height)
    });
    if base_width < 2 || base_height < 2 {
        return Err("export source dimensions must be at least two pixels".to_owned());
    }
    let width = width & !1;
    let height = ((u64::from(width) * u64::from(base_height) + u64::from(base_width) / 2)
        / u64::from(base_width))
    .clamp(2, u64::from(u32::MAX)) as u32
        & !1;
    Ok((Some(width), Some(height.max(2))))
}

fn artifact(path: PathBuf, width: u32, height: u32, kind: &str) -> Value {
    json!({
        "path": path.to_string_lossy(),
        "width": width,
        "height": height,
        "kind": kind,
    })
}

fn extension_for_format(format: ExportFormat) -> BridgeResult<&'static str> {
    match format {
        ExportFormat::Mp4 => Ok("mp4"),
        ExportFormat::Gif => Ok("gif"),
        ExportFormat::WebM => {
            Err("WebM export is not supported by the Captures native media backend".to_owned())
        }
    }
}

const fn kind_for_format(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Mp4 | ExportFormat::WebM => "video",
        ExportFormat::Gif => "gif",
    }
}

const fn state_name(state: RecordingState) -> &'static str {
    match state {
        RecordingState::Selecting => "selecting",
        RecordingState::Countdown => "countdown",
        RecordingState::Recording => "recording",
        RecordingState::Paused => "paused",
        RecordingState::Finalizing => "finalizing",
        RecordingState::Ready => "ready",
        RecordingState::Editor => "editor",
        RecordingState::Failed => "failed",
        RecordingState::Discarded => "discarded",
    }
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(target_os = "macos")]
fn pointer_cursor() -> Option<PointerCursor> {
    use core_graphics::{
        event::CGEvent,
        event_source::{CGEventSource, CGEventSourceStateID},
    };
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let point = CGEvent::new(source).ok()?.location();
    Some(PointerCursor {
        position: (point.x.round() as i32, point.y.round() as i32),
        image: None,
    })
}

#[cfg(not(target_os = "macos"))]
const fn pointer_cursor() -> Option<PointerCursor> {
    None
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };

    use captures_media::CropRect;
    use captures_recording::RecordingState;

    use super::{
        Engine, Lifecycle, MicrophonePermissionOperation, OperationLane, Reservation,
        ensure_recovery_idle, microphone_permission_operation, operation_lane, scaled_dimensions,
    };

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn microphone_discovery_uses_the_direct_lane_without_screen_permission() {
        let response = super::dispatch_direct(r#"{"op":"microphone_permission"}"#)
            .expect("permission operation must not wait for the recording worker")
            .unwrap();
        assert_eq!(response["status"], "unavailable");
        assert_eq!(response["devices"], serde_json::json!([]));
        assert!(
            super::dispatch_direct(r#"{"op":"microphone_permission","request":"true"}"#)
                .unwrap()
                .is_err()
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn saved_microphone_without_permission_cannot_create_a_recording() {
        let output = tempfile::tempdir().unwrap();
        let mut engine = Engine::default();
        let request = serde_json::json!({
            "op": "record_start", "output_dir": output.path(),
            "options": {
                "kind": "video", "target": {"type": "display", "display_id": "test"},
                "frames_per_second": 30, "max_resolution": "original",
                "countdown_seconds": 0, "show_cursor": true,
                "audio": {"microphone_device_id": "saved-device"}
            }
        });
        let error = engine.dispatch(&request.to_string()).unwrap_err();
        assert!(error.contains("Microphone access is required"), "{error}");
        assert!(engine.recording.is_none());
        assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
    }

    #[test]
    fn microphone_permission_contract_requests_only_when_needed_and_allowed() {
        assert_eq!(
            microphone_permission_operation(true, true, true),
            MicrophonePermissionOperation::Authorized
        );
        assert_eq!(
            microphone_permission_operation(false, true, true),
            MicrophonePermissionOperation::Request
        );
        assert_eq!(
            microphone_permission_operation(false, true, false),
            MicrophonePermissionOperation::NotDetermined
        );
        assert_eq!(
            microphone_permission_operation(false, false, true),
            MicrophonePermissionOperation::Denied
        );
    }

    #[test]
    fn long_stateless_operations_are_not_routed_to_recording_engine() {
        for operation in [
            "image_encode",
            "media_probe",
            "media_export",
            "recover_list",
            "microphone_permission",
        ] {
            assert_eq!(operation_lane(operation), OperationLane::Stateless);
        }
        for operation in ["recover", "recover_discard"] {
            assert_eq!(operation_lane(operation), OperationLane::Recovery);
        }
        assert_eq!(
            operation_lane("record_status"),
            OperationLane::RecordingEngine
        );
    }

    #[test]
    fn recovery_reservation_excludes_concurrent_recovery() {
        let flag = AtomicBool::new(false);
        let first = Reservation::acquire(&flag).unwrap();
        assert!(Reservation::acquire(&flag).is_err());
        drop(first);
        assert!(Reservation::acquire(&flag).is_ok());
    }

    #[test]
    fn recovery_requires_the_recording_engine_to_be_idle() {
        assert!(
            ensure_recovery_idle(&serde_json::json!({
                "ok": true,
                "value": { "state": "idle" }
            }))
            .is_ok()
        );
        assert!(
            ensure_recovery_idle(&serde_json::json!({
                "ok": true,
                "value": { "state": "paused" }
            }))
            .is_err()
        );
    }

    #[test]
    fn record_status_is_idle_without_a_session_and_reports_mute_state() {
        let engine = Engine::default();
        let status = engine.record_status().unwrap();
        assert_eq!(status["state"], "idle");
        assert_eq!(status["elapsed_ms"], 0);
        assert_eq!(status["microphone_muted"], false);
    }

    #[test]
    fn lifecycle_counts_only_finalized_and_active_recording_time() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.begin().unwrap();
        lifecycle.active_since = Some(Instant::now() - Duration::from_millis(40));
        assert!(lifecycle.elapsed_ms() >= 40);
        lifecycle.pause(125).unwrap();
        assert_eq!(lifecycle.elapsed_ms(), 125);
        assert_eq!(lifecycle.state, RecordingState::Paused);
        assert!(lifecycle.pause(1).is_err());
        lifecycle.begin().unwrap();
        lifecycle.restart();
        assert_eq!(lifecycle.elapsed_ms(), 0);
        assert_eq!(lifecycle.state, RecordingState::Selecting);
        lifecycle.begin().unwrap();
        assert_eq!(lifecycle.state, RecordingState::Recording);
    }

    #[test]
    fn scale_uses_cropped_aspect_ratio_and_even_dimensions() {
        assert_eq!(
            scaled_dimensions(
                Some(1001),
                Some(CropRect {
                    x: 30,
                    y: 40,
                    width: 800,
                    height: 300,
                }),
                1920,
                1080,
            )
            .unwrap(),
            (Some(1000), Some(374))
        );
        assert_eq!(
            scaled_dimensions(Some(640), None, 1920, 1080).unwrap(),
            (Some(640), Some(360))
        );
        assert!(scaled_dimensions(Some(1), None, 1920, 1080).is_err());
    }
}
