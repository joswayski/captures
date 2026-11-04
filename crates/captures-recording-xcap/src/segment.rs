use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use captures_capture::{DisplayDescriptor, WindowDescriptor, XcapBackend};
use captures_recording::{
    CaptureRect, PortalRegionSelection, RecordingOptions, RecordingSegmentInfo, RecordingTarget,
};
use captures_video::{H264Mp4Error, H264Mp4Writer};
use image::RgbaImage;
use parking_lot::Mutex;
use thiserror::Error;
use xcap::{Frame, Monitor, VideoRecorder};

use crate::{
    audio::AudioSegment,
    overlay::{PointerCaptureSpace, PointerLayout, PointerOverlay},
    pointer::{PointerSource, pointer_features_available},
    transform::{FrameRect, FrameTransform},
};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(3);
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// After a Region selection, how long to wait for a frame without the selector.
#[cfg(target_os = "linux")]
const POST_SELECTION_FRAME_WAIT: Duration = Duration::from_millis(200);

/// Shows the first granted frame of a Region recording and blocks until the
/// user confirms a rectangle in that frame's pixels (`None` cancels).
pub type PortalRegionSelector<'a> = dyn Fn(&RgbaImage) -> Option<CaptureRect> + 'a;

#[derive(Debug, Error)]
pub enum XcapRecordingError {
    #[error("recording target is no longer available")]
    TargetUnavailable,
    #[error("the selected recording target is outside its display")]
    InvalidTarget,
    #[error("audio capture failed: {0}")]
    Audio(String),
    #[error("cursor and click capture are unavailable in this desktop session")]
    PointerUnavailable,
    #[error("the screen recorder did not deliver a usable frame")]
    FirstFrameUnavailable,
    #[error("screen capture failed: {0}")]
    Capture(String),
    #[cfg(target_os = "linux")]
    #[error("Recording cancelled")]
    PortalCancelled,
    #[error("recording worker failed: {0}")]
    Worker(String),
    #[error(transparent)]
    Video(#[from] H264Mp4Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type XcapRecordingResult<T> = Result<T, XcapRecordingError>;

struct WorkerOutcome {
    duration_ms: u64,
    dropped_frames: u64,
}

struct EncoderContext {
    transform: FrameTransform,
    frame_rate: u16,
    writer: H264Mp4Writer,
    started_at: Instant,
    pointer_overlay: Option<PointerOverlay>,
}

enum CaptureRecorder {
    Direct(VideoRecorder),
    /// The source and its dropped-frame count before encoding began. Frames
    /// discarded while the user selected a Region are not dropped frames.
    #[cfg(target_os = "linux")]
    Portal(crate::PortalVideoSource, u64),
}

impl CaptureRecorder {
    fn warning(&self) -> Option<String> {
        match self {
            Self::Direct(_) => None,
            #[cfg(target_os = "linux")]
            Self::Portal(source, _) => source.warning(),
        }
    }

    fn is_portal(&self) -> bool {
        match self {
            Self::Direct(_) => false,
            #[cfg(target_os = "linux")]
            Self::Portal(..) => true,
        }
    }

    fn stop(self) -> XcapRecordingResult<u64> {
        match self {
            Self::Direct(recorder) => recorder
                .stop()
                .map(|()| 0)
                .map_err(|error| XcapRecordingError::Capture(error.to_string())),
            #[cfg(target_os = "linux")]
            Self::Portal(mut source, dropped_before) => {
                source.finish().map_err(XcapRecordingError::Capture)?;
                if let Some(problem) = source.warning() {
                    return Err(XcapRecordingError::Capture(problem));
                }
                Ok(source.dropped_frames().saturating_sub(dropped_before))
            }
        }
    }
}

struct RecordingInput {
    recorder: CaptureRecorder,
    receiver: Receiver<Frame>,
    first_image: RgbaImage,
    transform: FrameTransform,
}

pub struct XcapRecordingSegment {
    recorder: Option<CaptureRecorder>,
    stop_requested: Arc<AtomicBool>,
    capture_worker: Option<thread::JoinHandle<()>>,
    encoder_worker: Option<thread::JoinHandle<XcapRecordingResult<WorkerOutcome>>>,
    output_path: PathBuf,
    width: u32,
    height: u32,
    warning: Arc<Mutex<Option<String>>>,
    system_audio: Option<AudioSegment>,
    microphone: Option<AudioSegment>,
    portal_region: Option<PortalRegionSelection>,
}

impl XcapRecordingSegment {
    pub fn start(
        options: &RecordingOptions,
        output_path: &Path,
        display: &DisplayDescriptor,
    ) -> XcapRecordingResult<Self> {
        options
            .validate()
            .map_err(|error| XcapRecordingError::Worker(error.to_owned()))?;
        if options.target.is_portal() {
            return Err(XcapRecordingError::InvalidTarget);
        }
        if (options.show_cursor || options.highlight_clicks) && !pointer_features_available() {
            return Err(XcapRecordingError::PointerUnavailable);
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let monitor = find_monitor(&display.id)?;
        let source = source_rect(options, display)?;
        let (recorder, receiver) = monitor
            .video_recorder()
            .map_err(|error| XcapRecordingError::Capture(error.to_string()))?;
        recorder
            .start()
            .map_err(|error| XcapRecordingError::Capture(error.to_string()))?;
        let first = match receiver.recv_timeout(FIRST_FRAME_TIMEOUT) {
            Ok(frame) => frame,
            Err(_) => {
                drop(receiver);
                let _ = recorder.stop();
                return Err(XcapRecordingError::FirstFrameUnavailable);
            }
        };
        let first_image = match frame_image(first) {
            Ok(image) => image,
            Err(error) => {
                drop(receiver);
                let _ = recorder.stop();
                return Err(error);
            }
        };
        // Region targets are selected in overlay/CSS DIPs. Display/window
        // geometry stays in the backend's native units (physical on Windows).
        let (map_width, map_height) = match &options.target {
            RecordingTarget::Region { .. } => {
                let (width, height) = display.overlay_size();
                (
                    width.round().max(1.0) as u32,
                    height.round().max(1.0) as u32,
                )
            }
            _ => (display.width.max(1), display.height.max(1)),
        };
        let frame_rect = FrameRect::from_logical(
            source,
            map_width,
            map_height,
            first_image.width(),
            first_image.height(),
        );
        let Some(frame_rect) = frame_rect else {
            drop(receiver);
            let _ = recorder.stop();
            return Err(XcapRecordingError::InvalidTarget);
        };
        let transform = FrameTransform::new(
            frame_rect,
            options.max_resolution,
            first_image.width(),
            first_image.height(),
        );
        let Some(transform) = transform else {
            drop(receiver);
            let _ = recorder.stop();
            return Err(XcapRecordingError::InvalidTarget);
        };
        let (width, height) = transform.dimensions();
        let source_is_overlay_space = matches!(options.target, RecordingTarget::Region { .. });
        let pointer_overlay = (options.show_cursor || options.highlight_clicks).then(|| {
            PointerOverlay::new(
                PointerLayout::for_capture(
                    PointerCaptureSpace {
                        display_x: display.x,
                        display_y: display.y,
                        scale_factor: display.scale_factor,
                        physical_display_geometry: DisplayDescriptor::reports_physical_geometry(),
                        source,
                        source_is_overlay_space,
                    },
                    width,
                    height,
                ),
                options.show_cursor,
                options.highlight_clicks,
            )
        });
        Self::encode(
            options,
            output_path,
            RecordingInput {
                recorder: CaptureRecorder::Direct(recorder),
                receiver,
                first_image,
                transform,
            },
            pointer_overlay,
        )
    }

    /// Records only the portal-selected display/window. Cursor pixels come from the
    /// granted stream; no pointer polling, monitor enumeration or fallback.
    /// A Region crops a display stream: without a stored selection, the first
    /// granted frame goes to `select_region` (one consent, exact stream pixels).
    #[cfg(target_os = "linux")]
    pub fn start_portal(
        options: &RecordingOptions,
        output_path: &Path,
        cancelled: &dyn Fn() -> bool,
        select_region: Option<&PortalRegionSelector<'_>>,
    ) -> XcapRecordingResult<Self> {
        options
            .validate()
            .map_err(|error| XcapRecordingError::Worker(error.to_owned()))?;
        if !options.target.is_portal() {
            return Err(XcapRecordingError::InvalidTarget);
        }
        let (source, receiver) = crate::PortalVideoSource::start(
            &options.target,
            options.show_cursor,
            options.frames_per_second,
            cancelled,
        )
        .map_err(|error| match error {
            crate::PortalVideoError::Cancelled => XcapRecordingError::PortalCancelled,
            crate::PortalVideoError::Failed(error) => XcapRecordingError::Capture(error),
        })?;
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        let mut first_image = loop {
            if cancelled() {
                return Err(XcapRecordingError::PortalCancelled);
            }
            if Instant::now() >= deadline {
                return Err(XcapRecordingError::FirstFrameUnavailable);
            }
            match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
                Ok(frame) => break frame_image(frame)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(XcapRecordingError::Capture(
                        source.warning().unwrap_or_else(|| {
                            "The portal video stream ended before its first frame.".into()
                        }),
                    ));
                }
            }
        };
        let (receiver, region) = match &options.target {
            RecordingTarget::PortalRegion {
                selection: Some(selection),
            } => (receiver, Some(*selection)),
            RecordingTarget::PortalRegion { selection: None } => {
                let select = select_region.ok_or_else(|| {
                    XcapRecordingError::Worker("Region recording requires a selection".into())
                })?;
                let (chosen, receiver, latest) =
                    select_while_draining(receiver, &first_image, select)?;
                let Some(rect) = chosen else {
                    return Err(XcapRecordingError::PortalCancelled);
                };
                if cancelled() {
                    return Err(XcapRecordingError::PortalCancelled);
                }
                let selection = PortalRegionSelection {
                    rect,
                    frame_width: first_image.width(),
                    frame_height: first_image.height(),
                };
                if !selection.is_valid() {
                    return Err(XcapRecordingError::InvalidTarget);
                }
                // Encode from a frame taken after the selector closed: prefer a
                // fresh one, else the newest frame drained during selection.
                let after = match receiver.recv_timeout(POST_SELECTION_FRAME_WAIT) {
                    Ok(frame) => Some(frame),
                    Err(RecvTimeoutError::Timeout) => latest,
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(XcapRecordingError::Capture(
                            source.warning().unwrap_or_else(|| {
                                "The portal video stream ended during region selection.".into()
                            }),
                        ));
                    }
                };
                if let Some(frame) = after {
                    first_image = frame_image(frame)?;
                }
                (receiver, Some(selection))
            }
            _ => (receiver, None),
        };
        let source_rect = portal_source_rect(region, first_image.width(), first_image.height())
            .ok_or(XcapRecordingError::InvalidTarget)?;
        let transform = FrameTransform::new(
            source_rect,
            options.max_resolution,
            first_image.width(),
            first_image.height(),
        )
        .ok_or(XcapRecordingError::InvalidTarget)?;
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let dropped_before = source.dropped_frames();
        let mut segment = Self::encode(
            options,
            output_path,
            RecordingInput {
                recorder: CaptureRecorder::Portal(source, dropped_before),
                receiver,
                first_image,
                transform,
            },
            None,
        )?;
        segment.portal_region = region;
        Ok(segment)
    }

    /// The Region selection this portal segment crops, to persist for later
    /// segments. `None` for every other target.
    pub const fn portal_region_selection(&self) -> Option<PortalRegionSelection> {
        self.portal_region
    }

    fn encode(
        options: &RecordingOptions,
        output_path: &Path,
        input: RecordingInput,
        pointer_overlay: Option<PointerOverlay>,
    ) -> XcapRecordingResult<Self> {
        let RecordingInput {
            recorder,
            receiver,
            first_image,
            transform,
        } = input;
        let (width, height) = transform.dimensions();
        let bitrate = recording_bitrate(width, height, options.frames_per_second);
        let writer = match H264Mp4Writer::create(
            output_path,
            width,
            height,
            options.frames_per_second,
            bitrate,
        ) {
            Ok(writer) => writer,
            Err(error) => {
                drop(receiver);
                let _ = recorder.stop();
                return Err(error.into());
            }
        };
        let output_path = output_path.to_path_buf();
        let stop_requested = Arc::new(AtomicBool::new(false));
        let warning = Arc::new(Mutex::new(None));
        let latest_frame = Arc::new(Mutex::new(Some(first_image)));
        let dropped_frames = Arc::new(AtomicU64::new(0));
        let stop_on_disconnect = recorder.is_portal();
        let capture_worker = {
            let stop_requested = stop_requested.clone();
            let warning = warning.clone();
            let latest_frame = latest_frame.clone();
            let dropped_frames = dropped_frames.clone();
            match thread::Builder::new()
                .name("captures-xcap-frame-receiver".to_owned())
                .spawn(move || {
                    receive_frames(
                        receiver,
                        &latest_frame,
                        &dropped_frames,
                        &stop_requested,
                        &warning,
                        stop_on_disconnect,
                    );
                }) {
                Ok(worker) => worker,
                Err(error) => {
                    let _ = recorder.stop();
                    let _ = fs::remove_file(&output_path);
                    return Err(XcapRecordingError::Worker(error.to_string()));
                }
            }
        };
        let frame_rate = options.frames_per_second;
        let video_started_at = Instant::now();
        let encoder_worker = {
            let encoder_stop = stop_requested.clone();
            let latest_frame = latest_frame.clone();
            let dropped_frames = dropped_frames.clone();
            match thread::Builder::new()
                .name("captures-xcap-encoder".to_owned())
                .spawn(move || {
                    record_frames(
                        &latest_frame,
                        EncoderContext {
                            transform,
                            frame_rate,
                            writer,
                            started_at: video_started_at,
                            pointer_overlay,
                        },
                        &dropped_frames,
                        &encoder_stop,
                    )
                }) {
                Ok(worker) => worker,
                Err(error) => {
                    let _ = recorder.stop();
                    stop_requested.store(true, Ordering::Release);
                    let _ = capture_worker.join();
                    let _ = fs::remove_file(&output_path);
                    return Err(XcapRecordingError::Worker(error.to_string()));
                }
            }
        };
        let mut system_audio = None;
        if options.audio.capture_system_audio {
            let path = system_audio_path(&output_path);
            match AudioSegment::start_system(&path, video_started_at) {
                Ok(segment) => system_audio = Some(segment),
                Err(error) => {
                    let _ = recorder.stop();
                    stop_requested.store(true, Ordering::Release);
                    let _ = capture_worker.join();
                    let _ = encoder_worker.join();
                    let _ = fs::remove_file(&output_path);
                    return Err(error);
                }
            }
        }
        let mut microphone = None;
        if !options.audio.microphone_muted
            && let Some(device_id) = options.audio.microphone_device_id.as_deref()
        {
            let path = microphone_path(&output_path);
            match AudioSegment::start_microphone(device_id, &path, video_started_at) {
                Ok(segment) => microphone = Some(segment),
                Err(error) => {
                    let _ = recorder.stop();
                    stop_requested.store(true, Ordering::Release);
                    let _ = capture_worker.join();
                    let _ = encoder_worker.join();
                    if let Some(system_audio) = system_audio {
                        let _ = system_audio.discard();
                    }
                    let _ = fs::remove_file(&output_path);
                    return Err(error);
                }
            }
        }

        Ok(Self {
            recorder: Some(recorder),
            stop_requested,
            capture_worker: Some(capture_worker),
            encoder_worker: Some(encoder_worker),
            output_path,
            width,
            height,
            warning,
            system_audio,
            microphone,
            portal_region: None,
        })
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn stop(mut self) -> XcapRecordingResult<RecordingSegmentInfo> {
        self.stop_requested.store(true, Ordering::Release);
        let recorder_result = self.recorder.take().map(CaptureRecorder::stop).transpose();
        let system_audio_result = self.system_audio.take().map(AudioSegment::stop).transpose();
        let microphone_result = self.microphone.take().map(AudioSegment::stop).transpose();
        let capture_result = self.join_capture_worker();
        let encoder_result = self.join_encoder_worker();
        capture_result?;
        let outcome = encoder_result?;
        let source_dropped = recorder_result?.unwrap_or_default();
        let system_audio = system_audio_result?;
        let microphone = microphone_result?;
        let size_bytes = fs::metadata(&self.output_path)?.len();
        Ok(RecordingSegmentInfo {
            path: self.output_path.clone(),
            system_audio_path: system_audio.as_ref().and_then(|audio| audio.path.clone()),
            system_audio_offset_ms: system_audio.as_ref().map_or(0, |audio| audio.offset_ms),
            system_audio_warning: system_audio.and_then(|audio| audio.warning),
            microphone_path: microphone.as_ref().and_then(|audio| audio.path.clone()),
            microphone_offset_ms: microphone.as_ref().map_or(0, |audio| audio.offset_ms),
            microphone_warning: microphone.and_then(|audio| audio.warning),
            width: self.width,
            height: self.height,
            duration_ms: outcome.duration_ms,
            size_bytes,
            dropped_frames: outcome.dropped_frames.saturating_add(source_dropped),
        })
    }

    pub fn discard(mut self) -> XcapRecordingResult<()> {
        self.abort();
        match fs::remove_file(&self.output_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn abort(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        if let Some(recorder) = self.recorder.take() {
            let _ = recorder.stop();
        }
        if let Some(system_audio) = self.system_audio.take() {
            let _ = system_audio.discard();
        }
        if let Some(microphone) = self.microphone.take() {
            let _ = microphone.discard();
        }
        let _ = self.capture_worker.take().map(thread::JoinHandle::join);
        let _ = self.encoder_worker.take().map(thread::JoinHandle::join);
    }

    pub fn warning(&self) -> Option<String> {
        self.recorder
            .as_ref()
            .and_then(CaptureRecorder::warning)
            .or_else(|| self.warning.lock().clone())
            .or_else(|| self.system_audio.as_ref().and_then(AudioSegment::warning))
            .or_else(|| self.microphone.as_ref().and_then(AudioSegment::warning))
    }

    /// Fatal loss of a granted video source, distinct from device warnings.
    pub fn source_failure(&self) -> Option<String> {
        let recorder = self
            .recorder
            .as_ref()
            .filter(|recorder| recorder.is_portal())?;
        recorder.warning().or_else(|| {
            if self.stop_requested.load(Ordering::Acquire) {
                self.warning.lock().clone()
            } else {
                None
            }
        })
    }

    pub fn microphone_level(&self) -> f32 {
        self.microphone.as_ref().map_or(0.0, AudioSegment::level)
    }

    pub fn microphone_draft_info(&self) -> Option<(PathBuf, i64)> {
        self.microphone.as_ref().map(AudioSegment::draft_info)
    }

    pub fn system_audio_draft_info(&self) -> Option<(PathBuf, i64)> {
        self.system_audio.as_ref().map(AudioSegment::draft_info)
    }

    fn join_capture_worker(&mut self) -> XcapRecordingResult<()> {
        self.capture_worker
            .take()
            .ok_or_else(|| {
                XcapRecordingError::Worker("frame receiver worker was missing".to_owned())
            })?
            .join()
            .map_err(|_| XcapRecordingError::Worker("frame receiver worker panicked".to_owned()))
    }

    fn join_encoder_worker(&mut self) -> XcapRecordingResult<WorkerOutcome> {
        self.encoder_worker
            .take()
            .ok_or_else(|| XcapRecordingError::Worker("encoder worker was missing".to_owned()))?
            .join()
            .map_err(|_| XcapRecordingError::Worker("encoder worker panicked".to_owned()))?
    }
}

impl Drop for XcapRecordingSegment {
    fn drop(&mut self) {
        self.abort();
    }
}

fn receive_frames(
    receiver: Receiver<Frame>,
    latest_frame: &Mutex<Option<RgbaImage>>,
    dropped_frames: &AtomicU64,
    stop_requested: &AtomicBool,
    warning: &Mutex<Option<String>>,
    stop_on_disconnect: bool,
) {
    while !stop_requested.load(Ordering::Acquire) {
        match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
            Ok(frame) => match frame_image(frame) {
                Ok(image) => {
                    if latest_frame.lock().replace(image).is_some() {
                        dropped_frames.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(error) => set_warning_once(warning, error.to_string()),
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                set_warning_once(
                    warning,
                    "The screen capture stream stopped unexpectedly.".to_owned(),
                );
                if stop_on_disconnect {
                    stop_requested.store(true, Ordering::Release);
                }
                break;
            }
        }
    }
}

fn record_frames(
    latest_frame: &Mutex<Option<RgbaImage>>,
    context: EncoderContext,
    dropped_frames: &AtomicU64,
    stop_requested: &AtomicBool,
) -> XcapRecordingResult<WorkerOutcome> {
    let EncoderContext {
        transform,
        frame_rate,
        mut writer,
        started_at,
        mut pointer_overlay,
    } = context;
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(frame_rate));
    let first_image = latest_frame
        .lock()
        .take()
        .ok_or(XcapRecordingError::FirstFrameUnavailable)?;
    let mut last_rgb = Vec::new();
    transform.rgb_into(&first_image, &mut last_rgb);
    let mut skipped_frames = 0_u64;
    let pointer_source = pointer_overlay.as_ref().map(|_| PointerSource::new());
    encode_frame(
        &mut writer,
        &mut last_rgb,
        0,
        &mut pointer_overlay,
        pointer_source.as_ref(),
        Instant::now(),
    )?;
    let mut next_frame_at = started_at + frame_interval;

    while !stop_requested.load(Ordering::Acquire) {
        let now = Instant::now();
        if now < next_frame_at {
            thread::sleep((next_frame_at - now).min(CONTROL_POLL_INTERVAL));
            continue;
        }

        if let Some(image) = latest_frame.lock().take() {
            transform.rgb_into(&image, &mut last_rgb);
        }
        let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        encode_frame(
            &mut writer,
            &mut last_rgb,
            elapsed_ms,
            &mut pointer_overlay,
            pointer_source.as_ref(),
            Instant::now(),
        )?;

        next_frame_at += frame_interval;
        let now = Instant::now();
        if now > next_frame_at {
            let missed = ((now - next_frame_at).as_secs_f64() / frame_interval.as_secs_f64())
                .floor() as u64
                + 1;
            skipped_frames = skipped_frames.saturating_add(missed);
            next_frame_at += frame_interval.mul_f64(missed as f64);
        }
    }

    let duration_ms = u64::try_from(started_at.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    let info = writer.finish(duration_ms)?;
    Ok(WorkerOutcome {
        duration_ms: info.duration_ms,
        dropped_frames: dropped_frames
            .load(Ordering::Relaxed)
            .saturating_add(skipped_frames)
            .saturating_add(info.skipped_frames),
    })
}

fn encode_frame(
    writer: &mut H264Mp4Writer,
    rgb: &mut [u8],
    elapsed_ms: u64,
    overlay: &mut Option<PointerOverlay>,
    pointer: Option<&PointerSource>,
    now: Instant,
) -> XcapRecordingResult<()> {
    let patch = overlay.as_mut().map(|overlay| {
        let cursor = overlay
            .shows_cursor()
            .then(|| pointer.and_then(PointerSource::cursor_image))
            .flatten();
        overlay.draw(
            rgb,
            pointer.and_then(PointerSource::sample),
            cursor.as_ref(),
            now,
        )
    });
    let encoded = writer.encode_rgb(rgb, elapsed_ms);
    if let Some(patch) = patch {
        patch.restore(rgb);
    }
    let _ = encoded?;
    Ok(())
}

/// Runs the blocking Region selection while a worker keeps receiving frames, so
/// the bounded stream never backs up. Returns the choice, the receiver and the
/// newest frame that arrived meanwhile.
#[cfg(any(target_os = "linux", test))]
fn select_while_draining(
    receiver: Receiver<Frame>,
    first_image: &RgbaImage,
    select: &PortalRegionSelector<'_>,
) -> XcapRecordingResult<(Option<CaptureRect>, Receiver<Frame>, Option<Frame>)> {
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        let drain = thread::Builder::new()
            .name("captures-portal-region-drain".to_owned())
            .spawn_scoped(scope, || {
                let mut latest = None;
                while !done.load(Ordering::Acquire) {
                    match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
                        Ok(frame) => latest = Some(frame),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                (receiver, latest)
            })
            .map_err(|error| XcapRecordingError::Worker(error.to_string()))?;
        let chosen = select(first_image);
        done.store(true, Ordering::Release);
        let (receiver, latest) = drain.join().map_err(|_| {
            XcapRecordingError::Worker("region selection frame worker panicked".to_owned())
        })?;
        Ok((chosen, receiver, latest))
    })
}

/// The portal frame area to encode: the whole frame, or a Region selection
/// mapped from the frame size it was chosen on onto this stream's size.
#[cfg(any(target_os = "linux", test))]
fn portal_source_rect(
    region: Option<PortalRegionSelection>,
    width: u32,
    height: u32,
) -> Option<FrameRect> {
    match region {
        None => Some(FrameRect {
            x: 0,
            y: 0,
            width,
            height,
        }),
        Some(selection) if selection.is_valid() => FrameRect::from_logical(
            selection.rect,
            selection.frame_width,
            selection.frame_height,
            width,
            height,
        ),
        Some(_) => None,
    }
}

fn find_monitor(display_id: &str) -> XcapRecordingResult<Monitor> {
    Monitor::all()
        .map_err(|error| XcapRecordingError::Capture(error.to_string()))?
        .into_iter()
        .find(|monitor| {
            monitor
                .id()
                .map(|id| id.to_string() == display_id)
                .unwrap_or(false)
        })
        .ok_or(XcapRecordingError::TargetUnavailable)
}

fn source_rect(
    options: &RecordingOptions,
    display: &DisplayDescriptor,
) -> XcapRecordingResult<CaptureRect> {
    match &options.target {
        RecordingTarget::Display { display_id } if display_id == &display.id => Ok(CaptureRect {
            x: 0,
            y: 0,
            width: display.width,
            height: display.height,
        }),
        RecordingTarget::Region { display_id, rect } if display_id == &display.id => Ok(*rect),
        RecordingTarget::Window { window_id } => {
            let window = XcapBackend
                .windows()
                .map_err(|error| XcapRecordingError::Capture(error.to_string()))?
                .into_iter()
                .find(|window| &window.id == window_id)
                .ok_or(XcapRecordingError::TargetUnavailable)?;
            window_rect(&window, display)
        }
        _ => Err(XcapRecordingError::InvalidTarget),
    }
}

fn window_rect(
    window: &WindowDescriptor,
    display: &DisplayDescriptor,
) -> XcapRecordingResult<CaptureRect> {
    let x = window.x.saturating_sub(display.x);
    let y = window.y.saturating_sub(display.y);
    if x < 0 || y < 0 {
        return Err(XcapRecordingError::InvalidTarget);
    }
    Ok(CaptureRect {
        x,
        y,
        width: window.width,
        height: window.height,
    })
}

fn frame_image(frame: Frame) -> XcapRecordingResult<RgbaImage> {
    RgbaImage::from_raw(frame.width, frame.height, frame.raw)
        .ok_or(XcapRecordingError::FirstFrameUnavailable)
}

fn recording_bitrate(width: u32, height: u32, frame_rate: u16) -> u32 {
    // 12% bits-per-pixel. Compress exports on Windows/Linux must stay at or
    // below this (see `openh264_bitrate`) or Highest can enlarge the file.
    let estimated = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(u64::from(frame_rate))
        .saturating_mul(12)
        / 100;
    u32::try_from(estimated.clamp(1_000_000, 50_000_000)).unwrap_or(50_000_000)
}

fn set_warning_once(warning: &Mutex<Option<String>>, message: String) {
    let mut warning = warning.lock();
    if warning.is_none() {
        *warning = Some(message);
    }
}

fn system_audio_path(output_path: &Path) -> PathBuf {
    companion_audio_path(output_path, "system")
}

fn microphone_path(output_path: &Path) -> PathBuf {
    companion_audio_path(output_path, "mic")
}

fn companion_audio_path(output_path: &Path, suffix: &str) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("segment");
    output_path.with_file_name(format!("{stem}.{suffix}.wav"))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use captures_recording::{CaptureRect, MaxResolution, PortalRegionSelection};
    use image::{Rgba, RgbaImage};
    use xcap::Frame;

    use super::{FrameRect, FrameTransform, portal_source_rect, select_while_draining};

    /// Each pixel encodes its own coordinates, so a shifted, transposed or
    /// unscaled crop cannot match.
    fn encoded(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                (x % 251) as u8,
                (y % 241) as u8,
                (x / 251 + y / 241 * 8) as u8,
                255,
            ])
        })
    }

    fn pixel(rgb: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let offset = ((y * width + x) * 3) as usize;
        [rgb[offset], rgb[offset + 1], rgb[offset + 2]]
    }

    #[test]
    fn region_crops_exact_asymmetric_stream_pixels() {
        let frame = encoded(1_280, 900);
        let selection = PortalRegionSelection {
            rect: CaptureRect {
                x: 101,
                y: 37,
                width: 300,
                height: 200,
            },
            frame_width: 1_280,
            frame_height: 900,
        };
        let source = portal_source_rect(Some(selection), 1_280, 900).unwrap();
        assert_eq!(
            source,
            FrameRect {
                x: 101,
                y: 37,
                width: 300,
                height: 200,
            }
        );
        let transform = FrameTransform::new(source, MaxResolution::Original, 1_280, 900).unwrap();
        assert_eq!(transform.dimensions(), (300, 200));
        let rgb = transform.rgb(&frame);
        let expected = |x: u32, y: u32| {
            let [r, g, b, _] = frame.get_pixel(x, y).0;
            [r, g, b]
        };
        assert_eq!(pixel(&rgb, 300, 0, 0), expected(101, 37));
        assert_eq!(pixel(&rgb, 300, 299, 0), expected(400, 37));
        assert_eq!(pixel(&rgb, 300, 0, 199), expected(101, 236));
        assert_eq!(pixel(&rgb, 300, 299, 199), expected(400, 236));

        // Odd sizes are even-rounded by the encoder, keeping the top-left.
        let odd = PortalRegionSelection {
            rect: CaptureRect {
                x: 7,
                y: 5,
                width: 301,
                height: 199,
            },
            ..selection
        };
        let source = portal_source_rect(Some(odd), 1_280, 900).unwrap();
        let transform = FrameTransform::new(source, MaxResolution::Original, 1_280, 900).unwrap();
        assert_eq!(transform.dimensions(), (300, 198));
        assert_eq!(portal_source_rect(None, 1_280, 900).unwrap().width, 1_280);
    }

    #[test]
    fn resumed_region_maps_onto_a_differently_sized_stream() {
        let selection = PortalRegionSelection {
            rect: CaptureRect {
                x: 100,
                y: 120,
                width: 300,
                height: 200,
            },
            frame_width: 1_280,
            frame_height: 900,
        };
        // The same output now streams at 2x, then at an asymmetric size.
        assert_eq!(
            portal_source_rect(Some(selection), 2_560, 1_800).unwrap(),
            FrameRect {
                x: 200,
                y: 240,
                width: 600,
                height: 400,
            }
        );
        assert_eq!(
            portal_source_rect(Some(selection), 640, 1_800).unwrap(),
            FrameRect {
                x: 50,
                y: 240,
                width: 150,
                height: 400,
            }
        );
        let invalid = PortalRegionSelection {
            rect: CaptureRect {
                x: 1_200,
                ..selection.rect
            },
            ..selection
        };
        assert_eq!(portal_source_rect(Some(invalid), 1_280, 900), None);
    }

    #[test]
    fn selection_keeps_draining_the_bounded_stream_and_returns_its_newest_frame() {
        let (tx, rx) = mpsc::sync_channel(1);
        let first = encoded(64, 48);
        let (chosen, receiver, latest) = select_while_draining(rx, &first, &|image| {
            assert_eq!(image.dimensions(), (64, 48));
            // A blocked selector must not back up the one-slot stream.
            for index in 0..20_u8 {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut frame = Some(Frame {
                    width: 1,
                    height: 1,
                    raw: vec![index, 0, 0, 255],
                });
                while let Some(pending) = frame.take() {
                    match tx.try_send(pending) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full(pending)) => {
                            assert!(std::time::Instant::now() < deadline, "stream backed up");
                            frame = Some(pending);
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => panic!("receiver dropped"),
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            Some(CaptureRect {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            })
        })
        .unwrap();
        assert_eq!(chosen.unwrap().width, 3);
        assert_eq!(latest.unwrap().raw[0], 19);
        // The receiver is handed back for encoding.
        tx.send(Frame {
            width: 1,
            height: 1,
            raw: vec![99, 0, 0, 255],
        })
        .unwrap();
        assert_eq!(receiver.recv().unwrap().raw[0], 99);

        let (_tx, rx) = mpsc::sync_channel(1);
        let (chosen, _, latest) = select_while_draining(rx, &first, &|_| None).unwrap();
        assert!(chosen.is_none() && latest.is_none());
    }
}
