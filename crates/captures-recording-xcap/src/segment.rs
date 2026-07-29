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
use captures_recording::{CaptureRect, RecordingOptions, RecordingSegmentInfo, RecordingTarget};
use captures_video::{H264Mp4Error, H264Mp4Writer};
use image::RgbaImage;
use parking_lot::Mutex;
use thiserror::Error;
use xcap::{Frame, Monitor, VideoRecorder};

use crate::{
    audio::AudioSegment,
    overlay::{PointerLayout, PointerOverlay},
    pointer::{PointerSource, pointer_features_available},
    transform::{FrameRect, FrameTransform},
};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(3);
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);

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

pub struct XcapRecordingSegment {
    recorder: VideoRecorder,
    stop_requested: Arc<AtomicBool>,
    capture_worker: Option<thread::JoinHandle<()>>,
    encoder_worker: Option<thread::JoinHandle<XcapRecordingResult<WorkerOutcome>>>,
    output_path: PathBuf,
    width: u32,
    height: u32,
    warning: Arc<Mutex<Option<String>>>,
    system_audio: Option<AudioSegment>,
    microphone: Option<AudioSegment>,
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
        let frame_rect = FrameRect::from_logical(
            source,
            display.width,
            display.height,
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
        let pointer_scale = if cfg!(target_os = "linux") {
            display.scale_factor
        } else {
            1.0
        };
        let pointer_overlay = (options.show_cursor || options.highlight_clicks).then(|| {
            PointerOverlay::new(
                PointerLayout::new(display.x, display.y, source, width, height, pointer_scale),
                options.show_cursor,
                options.highlight_clicks,
            )
        });
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
            recorder,
            stop_requested,
            capture_worker: Some(capture_worker),
            encoder_worker: Some(encoder_worker),
            output_path,
            width,
            height,
            warning,
            system_audio,
            microphone,
        })
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn stop(mut self) -> XcapRecordingResult<RecordingSegmentInfo> {
        let recorder_result = self
            .recorder
            .stop()
            .map_err(|error| XcapRecordingError::Capture(error.to_string()));
        self.stop_requested.store(true, Ordering::Release);
        let system_audio_result = self.system_audio.take().map(AudioSegment::stop).transpose();
        let microphone_result = self.microphone.take().map(AudioSegment::stop).transpose();
        let capture_result = self.join_capture_worker();
        let encoder_result = self.join_encoder_worker();
        capture_result?;
        let outcome = encoder_result?;
        recorder_result?;
        let system_audio = system_audio_result?;
        let microphone = microphone_result?;
        let size_bytes = fs::metadata(&self.output_path)?.len();
        Ok(RecordingSegmentInfo {
            path: self.output_path,
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
            dropped_frames: outcome.dropped_frames,
        })
    }

    pub fn discard(mut self) -> XcapRecordingResult<()> {
        let _ = self.recorder.stop();
        self.stop_requested.store(true, Ordering::Release);
        if let Some(system_audio) = self.system_audio.take() {
            let _ = system_audio.discard();
        }
        if let Some(microphone) = self.microphone.take() {
            let _ = microphone.discard();
        }
        let _ = self.join_capture_worker();
        let _ = self.join_encoder_worker();
        match fs::remove_file(self.output_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn warning(&self) -> Option<String> {
        self.warning
            .lock()
            .clone()
            .or_else(|| self.system_audio.as_ref().and_then(AudioSegment::warning))
            .or_else(|| self.microphone.as_ref().and_then(AudioSegment::warning))
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

fn receive_frames(
    receiver: Receiver<Frame>,
    latest_frame: &Mutex<Option<RgbaImage>>,
    dropped_frames: &AtomicU64,
    stop_requested: &AtomicBool,
    warning: &Mutex<Option<String>>,
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
    let mut last_rgb = transform.rgb(&first_image);
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
            last_rgb = transform.rgb(&image);
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
    let patch = overlay
        .as_mut()
        .map(|overlay| overlay.draw(rgb, pointer.and_then(PointerSource::sample), now));
    let encoded = writer.encode_rgb(rgb, elapsed_ms);
    if let Some(patch) = patch {
        patch.restore(rgb);
    }
    let _ = encoded?;
    Ok(())
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
