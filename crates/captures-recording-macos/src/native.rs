use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use captures_recording::{AudioDevice, AudioDeviceKind, RecordingOptions, RecordingTarget};
use core_graphics::display::CGDisplay as CoreGraphicsDisplay;
use screencapturekit::{
    audio_devices::AudioInputDevice,
    cg::CGRect,
    cm::{CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus},
    dispatch_queue::{DispatchQoS, DispatchQueue},
    shareable_content::{SCDisplay, SCRunningApplication, SCShareableContent, SCWindow},
    stream::{
        SCStream,
        configuration::{PixelFormat, SCCaptureResolutionType, SCStreamConfiguration},
        content_filter::SCContentFilter,
        delegate_trait::StreamCallbacks,
        output_type::SCStreamOutputType,
    },
};

use crate::{
    MacRecordingError, MacRecordingResult, SegmentInfo,
    writer::{ClickHighlightSource, MediaWriter},
};

pub struct NativeRecordingSegment {
    stream: SCStream,
    _output_queue: DispatchQueue,
    writer: MediaWriter,
    video_output_id: usize,
    audio_output_id: Option<usize>,
    path: PathBuf,
    width: u32,
    height: u32,
    failure: Arc<Mutex<Option<String>>>,
    started_at: Instant,
}

struct CaptureSource {
    filter: SCContentFilter,
    pixel_width: f64,
    pixel_height: f64,
    frame: CGRect,
}

impl NativeRecordingSegment {
    pub fn start(options: &RecordingOptions, output_path: &Path) -> MacRecordingResult<Self> {
        if output_path.to_str().is_none() {
            return Err(MacRecordingError::InvalidOutputPath);
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if output_path.exists() {
            fs::remove_file(output_path)?;
        }

        let content = SCShareableContent::get()
            .map_err(|error| MacRecordingError::ScreenCaptureKit(error.to_string()))?;
        let source = filter_for_target(&content, &options.target)?;
        let source_width_pixels = source.pixel_width.round().max(2.0) as u32;
        let source_height_pixels = source.pixel_height.round().max(2.0) as u32;
        let (width, height) = options
            .max_resolution
            .constrain(source_width_pixels, source_height_pixels);
        let (shows_cursor, shows_native_mouse_clicks) =
            capture_pointer_options(options.show_cursor, options.highlight_clicks);

        let mut configuration = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_scales_to_fit(true)
            .with_capture_resolution_type(SCCaptureResolutionType::Best)
            .with_pixel_format(PixelFormat::BGRA)
            .with_fps(u32::from(options.frames_per_second))
            .with_queue_depth(8)
            .with_shows_cursor(shows_cursor)
            .with_shows_mouse_clicks(shows_native_mouse_clicks)
            .with_captures_audio(options.audio.capture_system_audio)
            .with_sample_rate(48_000)
            .with_channel_count(if options.audio.mono_output { 1 } else { 2 })
            .with_excludes_current_process_audio(true);

        if let RecordingTarget::Region { rect, .. } = options.target {
            configuration.set_source_rect(CGRect::new(
                f64::from(rect.x),
                f64::from(rect.y),
                f64::from(rect.width),
                f64::from(rect.height),
            ));
        }

        let failure = Arc::new(Mutex::new(None));
        let stream_failure = failure.clone();
        let stream_delegate = StreamCallbacks::new().on_error(move |error| {
            if let Ok(mut current) = stream_failure.lock() {
                *current = Some(error.to_string());
            }
        });
        let mut stream =
            SCStream::new_with_delegate(&source.filter, &configuration, stream_delegate);
        let click_highlight_source = options.highlight_clicks.then_some(ClickHighlightSource {
            x: source.frame.origin.x,
            y: source.frame.origin.y,
            width: source.frame.size.width,
            height: source.frame.size.height,
        });
        let writer = MediaWriter::new(
            output_path,
            width,
            height,
            u32::from(options.frames_per_second),
            options.audio.capture_system_audio,
            options.audio.mono_output,
            click_highlight_source,
        )?;
        let output_queue = DispatchQueue::new(
            "io.github.joswayski.captures.recording",
            DispatchQoS::UserInteractive,
        );
        let first_video_frame = Arc::new((AtomicBool::new(false), Mutex::new(()), Condvar::new()));
        let video_writer = writer.clone();
        let video_failure = failure.clone();
        let video_ready = first_video_frame.clone();
        let video_output_id = stream
            .add_output_handler_with_queue(
                move |sample: CMSampleBuffer, _output_type: SCStreamOutputType| {
                    let Some(frame_kind) = sample_video_frame_kind(&sample) else {
                        return;
                    };
                    let appended = match frame_kind {
                        VideoFrameKind::Content => video_writer.append_video(&sample),
                        VideoFrameKind::Idle => video_writer.append_idle_video(&sample),
                    };
                    if !appended {
                        if let Ok(mut current) = video_failure.lock() {
                            *current = Some(video_writer.error_message());
                        }
                    } else if !video_ready.0.load(Ordering::Acquire)
                        && video_writer.has_video_frame()
                    {
                        let (ready, gate, wake) = &*video_ready;
                        let _gate = gate
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        ready.store(true, Ordering::Release);
                        wake.notify_all();
                    }
                },
                SCStreamOutputType::Screen,
                Some(&output_queue),
            )
            .ok_or_else(|| {
                MacRecordingError::ScreenCaptureKit(
                    "could not register the video sample output".to_owned(),
                )
            })?;
        let audio_output_id = if options.audio.capture_system_audio {
            let audio_writer = writer.clone();
            let audio_failure = failure.clone();
            Some(
                stream
                    .add_output_handler_with_queue(
                        move |sample: CMSampleBuffer, _output_type: SCStreamOutputType| {
                            if !audio_writer.append_audio(&sample)
                                && let Ok(mut current) = audio_failure.lock()
                            {
                                *current = Some(audio_writer.error_message());
                            }
                        },
                        SCStreamOutputType::Audio,
                        Some(&output_queue),
                    )
                    .ok_or_else(|| {
                        MacRecordingError::ScreenCaptureKit(
                            "could not register the system-audio sample output".to_owned(),
                        )
                    })?,
            )
        } else {
            None
        };

        let started_at = Instant::now();
        if let Err(error) = stream.start_capture() {
            let _ = stream.remove_output_handler(video_output_id, SCStreamOutputType::Screen);
            if let Some(audio_output_id) = audio_output_id {
                let _ = stream.remove_output_handler(audio_output_id, SCStreamOutputType::Audio);
            }
            return Err(MacRecordingError::ScreenCaptureKit(error.to_string()));
        }

        let (ready, gate, wake) = &*first_video_frame;
        let gate = gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (gate, _) = wake
            .wait_timeout_while(gate, Duration::from_secs(2), |_| {
                !ready.load(Ordering::Acquire)
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let received_first_frame = ready.load(Ordering::Acquire);
        drop(gate);
        if !received_first_frame {
            let stream_error = failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let _ = stream.stop_capture();
            let _ = stream.remove_output_handler(video_output_id, SCStreamOutputType::Screen);
            if let Some(audio_output_id) = audio_output_id {
                let _ = stream.remove_output_handler(audio_output_id, SCStreamOutputType::Audio);
            }
            let _ = writer.finish();
            let _ = fs::remove_file(output_path);
            return Err(MacRecordingError::RecordingFailed(
                stream_error.unwrap_or_else(|| {
                    "ScreenCaptureKit did not deliver a usable video frame".to_owned()
                }),
            ));
        }

        Ok(Self {
            stream,
            _output_queue: output_queue,
            writer,
            video_output_id,
            audio_output_id,
            path: output_path.to_path_buf(),
            width,
            height,
            failure,
            started_at,
        })
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn elapsed_since_start_ms(&self) -> i64 {
        i64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    pub fn warning(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn stop(mut self) -> MacRecordingResult<SegmentInfo> {
        // ScreenCaptureKit can report a generic stop error after it has already
        // stopped delivering samples. The media collected up to that point is
        // still valid, so always detach the outputs and finalize the writer.
        // A writer or stream-delegate failure remains fatal below.
        let _ = self.stream.stop_capture();
        let _ = self
            .stream
            .remove_output_handler(self.video_output_id, SCStreamOutputType::Screen);
        if let Some(audio_output_id) = self.audio_output_id {
            let _ = self
                .stream
                .remove_output_handler(audio_output_id, SCStreamOutputType::Audio);
        }
        self.writer.finish()?;
        if let Some(error) = self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            return Err(MacRecordingError::RecordingFailed(error));
        }
        let size_bytes = fs::metadata(&self.path)?.len();
        let duration_ms = self.writer.duration_ms();
        Ok(SegmentInfo {
            path: self.path,
            microphone_path: None,
            microphone_offset_ms: 0,
            microphone_warning: None,
            width: self.width,
            height: self.height,
            duration_ms,
            size_bytes,
            dropped_frames: self.writer.dropped_frames(),
        })
    }

    pub fn discard(mut self) -> MacRecordingResult<()> {
        let path = self.path.clone();
        let _ = self.stream.stop_capture();
        let _ = self
            .stream
            .remove_output_handler(self.video_output_id, SCStreamOutputType::Screen);
        if let Some(audio_output_id) = self.audio_output_id {
            let _ = self
                .stream
                .remove_output_handler(audio_output_id, SCStreamOutputType::Audio);
        }
        let _ = self.writer.finish();
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoFrameKind {
    Content,
    Idle,
}

fn sample_video_frame_kind(sample: &CMSampleBuffer) -> Option<VideoFrameKind> {
    video_frame_kind(
        sample.frame_status(),
        sample.is_valid(),
        sample.data_is_ready(),
        // The raw pointer accessor returns a retained surface in the current
        // bridge. Adopt it so this temporary validation retain is released.
        sample.image_buffer().is_some(),
    )
}

const fn video_frame_kind(
    status: Option<SCFrameStatus>,
    is_valid: bool,
    data_is_ready: bool,
    has_image_buffer: bool,
) -> Option<VideoFrameKind> {
    if !is_valid || !data_is_ready || !has_image_buffer {
        return None;
    }
    match status {
        Some(SCFrameStatus::Complete | SCFrameStatus::Started) => Some(VideoFrameKind::Content),
        // Cursor movement and Captures' click ripple may arrive on an idle
        // surface. The writer accepts these only when no real content frame is
        // waiting, so pointer updates cannot displace screen motion.
        Some(SCFrameStatus::Idle) => Some(VideoFrameKind::Idle),
        Some(_) => None,
        // Some ScreenCaptureKit runtimes expose the status attachment as an
        // integer that the binding cannot currently decode. A valid, ready
        // video sample with an image buffer is still safe to append.
        None => Some(VideoFrameKind::Content),
    }
}

pub fn microphone_devices() -> Vec<AudioDevice> {
    microphone_choices(
        AudioInputDevice::list()
            .into_iter()
            .map(|device| (device.id, device.name, device.is_default))
            .collect(),
    )
}

fn microphone_choices(physical_devices: Vec<(String, String, bool)>) -> Vec<AudioDevice> {
    let default_name = physical_devices
        .iter()
        .find(|(_, _, is_default)| *is_default)
        .map(|(_, name, _)| name.as_str())
        .unwrap_or("System default");
    let mut devices = vec![AudioDevice {
        id: "default".to_owned(),
        name: format!("Default — {default_name}"),
        kind: AudioDeviceKind::Default,
        is_default: true,
    }];
    devices.extend(
        physical_devices
            .into_iter()
            .filter(|(_, _, is_default)| !is_default)
            .map(|(id, name, is_default)| AudioDevice {
                id,
                name,
                kind: AudioDeviceKind::Microphone,
                is_default,
            }),
    );
    devices
}

pub fn microphone_name_for_id(device_id: &str) -> Option<String> {
    AudioInputDevice::list()
        .into_iter()
        .find(|device| device.id == device_id)
        .map(|device| device.name)
}

fn filter_for_target(
    content: &SCShareableContent,
    target: &RecordingTarget,
) -> MacRecordingResult<CaptureSource> {
    match target {
        RecordingTarget::Display { display_id } => {
            let display = find_display(content, display_id)?;
            let scale = display_pixel_scale(&display);
            let frame = display.frame();
            let applications = current_process_applications(content);
            let application_refs = applications.iter().collect::<Vec<_>>();
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_excluding_applications(&application_refs, &[])
                .build();
            Ok(CaptureSource {
                filter,
                pixel_width: f64::from(display.width()) * scale,
                pixel_height: f64::from(display.height()) * scale,
                frame,
            })
        }
        RecordingTarget::Region {
            display_id, rect, ..
        } => {
            let display = find_display(content, display_id)?;
            let scale = display_pixel_scale(&display);
            let display_frame = display.frame();
            let applications = current_process_applications(content);
            let application_refs = applications.iter().collect::<Vec<_>>();
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_excluding_applications(&application_refs, &[])
                .build();
            Ok(CaptureSource {
                filter,
                pixel_width: f64::from(rect.width) * scale,
                pixel_height: f64::from(rect.height) * scale,
                frame: CGRect::new(
                    display_frame.origin.x + f64::from(rect.x),
                    display_frame.origin.y + f64::from(rect.y),
                    f64::from(rect.width),
                    f64::from(rect.height),
                ),
            })
        }
        RecordingTarget::Window { window_id } => {
            let window = find_window(content, window_id)?;
            let frame = window.frame();
            let scale = window_pixel_scale(content, frame);
            let filter = SCContentFilter::create().with_window(&window).build();
            Ok(CaptureSource {
                filter,
                pixel_width: frame.size.width * scale,
                pixel_height: frame.size.height * scale,
                frame,
            })
        }
    }
}

fn display_pixel_scale(display: &SCDisplay) -> f64 {
    let core_graphics_display = CoreGraphicsDisplay::new(display.display_id());
    core_graphics_display.display_mode().map_or_else(
        || {
            let logical_width = display.frame().size.width.max(1.0);
            (f64::from(display.width()) / logical_width).max(1.0)
        },
        |mode| pixel_scale_from_dimensions(mode.width(), mode.pixel_width()),
    )
}

fn pixel_scale_from_dimensions(logical_width: u64, pixel_width: u64) -> f64 {
    if logical_width == 0 || pixel_width == 0 {
        return 1.0;
    }
    (pixel_width as f64 / logical_width as f64).max(1.0)
}

// Captures draws its own lavender ripple into the writer's copied frame. Keep
// ScreenCaptureKit's fixed system circle disabled while still forcing the
// cursor on whenever a ripple is requested.
const fn capture_pointer_options(show_cursor: bool, highlight_clicks: bool) -> (bool, bool) {
    (show_cursor || highlight_clicks, false)
}

fn window_pixel_scale(content: &SCShareableContent, window_frame: CGRect) -> f64 {
    content
        .displays()
        .into_iter()
        .filter_map(|display| {
            let area = intersection_area(display.frame(), window_frame);
            (area > 0.0).then_some((area, display))
        })
        .max_by(|(left_area, _), (right_area, _)| left_area.total_cmp(right_area))
        .map_or(1.0, |(_, display)| display_pixel_scale(&display))
}

fn intersection_area(left: CGRect, right: CGRect) -> f64 {
    let left_edge = left.origin.x.max(right.origin.x);
    let top_edge = left.origin.y.max(right.origin.y);
    let right_edge = (left.origin.x + left.size.width).min(right.origin.x + right.size.width);
    let bottom_edge = (left.origin.y + left.size.height).min(right.origin.y + right.size.height);
    (right_edge - left_edge).max(0.0) * (bottom_edge - top_edge).max(0.0)
}

fn find_display(content: &SCShareableContent, id: &str) -> MacRecordingResult<SCDisplay> {
    let id = id
        .parse::<u32>()
        .map_err(|_| MacRecordingError::TargetUnavailable)?;
    content
        .displays()
        .into_iter()
        .find(|display| display.display_id() == id)
        .ok_or(MacRecordingError::TargetUnavailable)
}

fn find_window(content: &SCShareableContent, id: &str) -> MacRecordingResult<SCWindow> {
    let id = id
        .parse::<u32>()
        .map_err(|_| MacRecordingError::TargetUnavailable)?;
    content
        .windows()
        .into_iter()
        .find(|window| window.window_id() == id)
        .ok_or(MacRecordingError::TargetUnavailable)
}

fn current_process_applications(content: &SCShareableContent) -> Vec<SCRunningApplication> {
    let process_id = i32::try_from(std::process::id()).unwrap_or_default();
    content
        .applications()
        .into_iter()
        .filter(|application| application.process_id() == process_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        VideoFrameKind, capture_pointer_options, intersection_area, microphone_choices,
        pixel_scale_from_dimensions, video_frame_kind,
    };
    use screencapturekit::cm::SCFrameStatus;
    use screencapturekit::prelude::CGRect;
    use screencapturekit::stream::configuration::{SCCaptureResolutionType, SCStreamConfiguration};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(x, y, width, height)
    }

    #[test]
    fn intersection_area_handles_overlap_and_disjoint_rectangles() {
        assert_eq!(
            intersection_area(rect(0.0, 0.0, 100.0, 80.0), rect(50.0, 40.0, 80.0, 80.0)),
            2_000.0
        );
        assert_eq!(
            intersection_area(rect(0.0, 0.0, 10.0, 10.0), rect(20.0, 20.0, 5.0, 5.0)),
            0.0
        );
    }

    #[test]
    fn accepts_ready_video_samples_when_frame_status_is_unavailable() {
        assert_eq!(
            video_frame_kind(None, true, true, true),
            Some(VideoFrameKind::Content)
        );
        assert_eq!(
            video_frame_kind(Some(SCFrameStatus::Complete), true, true, true,),
            Some(VideoFrameKind::Content)
        );
        assert_eq!(
            video_frame_kind(Some(SCFrameStatus::Started), true, true, true,),
            Some(VideoFrameKind::Content)
        );
        assert_eq!(
            video_frame_kind(Some(SCFrameStatus::Idle), true, true, true,),
            Some(VideoFrameKind::Idle)
        );
        assert_eq!(video_frame_kind(None, false, true, true), None);
        assert_eq!(video_frame_kind(None, true, false, true), None);
        assert_eq!(video_frame_kind(None, true, true, false), None);
    }

    #[test]
    fn records_retina_sources_at_their_physical_pixel_scale() {
        assert_eq!(pixel_scale_from_dimensions(1_728, 3_456), 2.0);
        assert_eq!(pixel_scale_from_dimensions(1_920, 1_920), 1.0);
        assert_eq!(pixel_scale_from_dimensions(0, 3_456), 1.0);

        let configuration = SCStreamConfiguration::new()
            .with_capture_resolution_type(SCCaptureResolutionType::Best);
        assert_eq!(
            configuration.capture_resolution_type(),
            SCCaptureResolutionType::Best
        );
    }

    #[test]
    fn click_highlights_always_include_the_cursor_they_surround() {
        assert_eq!(capture_pointer_options(false, false), (false, false));
        assert_eq!(capture_pointer_options(true, false), (true, false));
        assert_eq!(capture_pointer_options(false, true), (true, false));

        let configuration = SCStreamConfiguration::new()
            .with_shows_cursor(true)
            .with_shows_mouse_clicks(false);
        assert!(configuration.shows_cursor());
        assert!(!configuration.shows_mouse_clicks());
    }

    #[test]
    fn labels_the_default_microphone_and_omits_its_duplicate() {
        let devices = microphone_choices(vec![
            ("built-in".to_owned(), "MacBook Microphone".to_owned(), true),
            ("usb".to_owned(), "USB Microphone".to_owned(), false),
        ]);

        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, "default");
        assert_eq!(devices[0].name, "Default — MacBook Microphone");
        assert_eq!(devices[1].id, "usb");
        assert!(!devices.iter().any(|device| device.id == "built-in"));
    }
}
