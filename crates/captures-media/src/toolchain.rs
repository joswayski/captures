use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use captures_video::H264Mp4Writer;
use serde::Deserialize;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::io;
use thiserror::Error;

use crate::{
    EditSpec, ExportFormat, ExportProgress, ExportSpec, ExportStage, MediaKind, MediaMetadata,
    QualityPreset, SizeBudgetError, calculate_size_budget,
    export::{MIN_AUDIO_BITRATE, MIN_VIDEO_BITRATE},
};

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeResult {
    pub metadata: MediaMetadata,
    pub has_audio: bool,
    pub audio_stream_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportOutcome {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub attempts: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordingSegmentInput {
    pub video_path: PathBuf,
    pub microphone_path: Option<PathBuf>,
    pub microphone_offset_ms: i64,
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimelineSpriteSpec {
    pub duration_ms: u64,
    pub frame_count: u16,
    pub frame_width: u32,
    pub frame_height: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecordingAudioLayout {
    pub system_audio: bool,
    pub microphone_audio: bool,
}

#[derive(Debug, Error)]
pub enum MediaToolError {
    #[error("the bundled {0} media tool is unavailable")]
    ToolUnavailable(&'static str),
    #[error("media processing was cancelled")]
    Cancelled,
    #[error("media processing failed: {0}")]
    Process(String),
    #[error("the requested maximum file size cannot be reached without trimming the recording")]
    UnattainableTarget,
    #[error("media metadata is incomplete")]
    IncompleteMetadata,
    #[error("the source does not contain independently editable system and microphone tracks")]
    SeparateAudioUnavailable,
    #[error("invalid edit request: {0}")]
    InvalidEdit(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<SizeBudgetError> for MediaToolError {
    fn from(_: SizeBudgetError) -> Self {
        Self::UnattainableTarget
    }
}

#[derive(Clone, Debug)]
pub struct MediaToolchain {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl MediaToolchain {
    pub fn new(ffmpeg: PathBuf, ffprobe: PathBuf) -> Self {
        Self { ffmpeg, ffprobe }
    }

    pub fn from_command_names() -> Self {
        Self::new(PathBuf::from("ffmpeg"), PathBuf::from("ffprobe"))
    }

    pub fn verify(&self) -> Result<(), MediaToolError> {
        verify_tool(&self.ffmpeg, "FFmpeg")?;
        verify_tool(&self.ffprobe, "ffprobe")
    }

    pub fn probe(&self, input: &Path) -> Result<ProbeResult, MediaToolError> {
        let output = Command::new(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
            ])
            .arg(input)
            .output()
            .map_err(|error| map_spawn_error(error, "ffprobe"))?;
        if !output.status.success() {
            return Err(MediaToolError::Process(process_message(&output.stderr)));
        }
        let probe: FfprobeOutput = serde_json::from_slice(&output.stdout)?;
        let video = probe
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("video"))
            .ok_or(MediaToolError::IncompleteMetadata)?;
        let duration_seconds = video
            .duration
            .as_deref()
            .or(probe.format.duration.as_deref())
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        let size_bytes = probe
            .format
            .size
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| fs::metadata(input).ok().map(|metadata| metadata.len()))
            .unwrap_or(0);
        let audio_stream_count = probe
            .streams
            .iter()
            .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
            .count();
        let extension = input
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        let kind = if extension.eq_ignore_ascii_case("gif") {
            MediaKind::Gif
        } else {
            MediaKind::Video
        };
        let mime_type = match kind {
            MediaKind::Gif => "image/gif",
            MediaKind::Video => "video/mp4",
            MediaKind::Screenshot => "image/png",
        };
        Ok(ProbeResult {
            metadata: MediaMetadata {
                kind,
                mime_type: mime_type.to_owned(),
                width: video.width.ok_or(MediaToolError::IncompleteMetadata)?,
                height: video.height.ok_or(MediaToolError::IncompleteMetadata)?,
                duration_ms: Some((duration_seconds.max(0.0) * 1_000.0).round() as u64),
                size_bytes,
            },
            has_audio: audio_stream_count > 0,
            audio_stream_count,
        })
    }

    pub fn concatenate_segments(
        &self,
        segments: &[PathBuf],
        destination: &Path,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        if segments.is_empty() {
            return Err(MediaToolError::IncompleteMetadata);
        }
        if segments.len() == 1 {
            return atomic_copy(&segments[0], destination);
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let concat_path = temporary_output_path(destination, "concat-list").with_extension("txt");
        let temporary = temporary_output_path(destination, "concat");
        let result = (|| {
            let mut concat = fs::File::create(&concat_path)?;
            for segment in segments {
                writeln!(concat, "file '{}'", escape_concat_path(segment))?;
            }
            concat.sync_all()?;
            let mut command = Command::new(&self.ffmpeg);
            command
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                ])
                .arg(&concat_path)
                .args(["-c", "copy", "-movflags", "+faststart"])
                .arg(&temporary);
            run_command(&mut command, cancel, "FFmpeg")?;
            commit_temporary(&temporary, destination)
        })();
        let _ = fs::remove_file(concat_path);
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn assemble_recording_segments(
        &self,
        segments: &[RecordingSegmentInput],
        destination: &Path,
        audio: RecordingAudioLayout,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        if segments.is_empty() {
            return Err(MediaToolError::IncompleteMetadata);
        }
        if !audio.microphone_audio {
            let paths = segments
                .iter()
                .map(|segment| segment.video_path.clone())
                .collect::<Vec<_>>();
            return self.concatenate_segments(&paths, destination, cancel);
        }

        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut normalized = Vec::with_capacity(segments.len());
        let result = (|| {
            for (index, segment) in segments.iter().enumerate() {
                let path = temporary_output_path(destination, &format!("normalized-{index}"));
                self.normalize_recording_segment(segment, &path, audio, cancel)?;
                normalized.push(path);
            }
            self.concatenate_segments(&normalized, destination, cancel)
        })();
        for path in normalized {
            let _ = fs::remove_file(path);
        }
        result
    }

    fn normalize_recording_segment(
        &self,
        segment: &RecordingSegmentInput,
        destination: &Path,
        audio: RecordingAudioLayout,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let system_stream_available =
            audio.system_audio && self.probe(&segment.video_path)?.has_audio;
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]);
        command.arg(&segment.video_path);
        if let Some(microphone_path) = &segment.microphone_path {
            command.arg("-i").arg(microphone_path);
        }

        let duration = seconds(segment.duration_ms.max(1));
        let mut filters = Vec::new();
        let mut audio_labels = Vec::new();
        if audio.system_audio {
            if system_stream_available {
                filters.push(format!(
                    "[0:a:0]aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration}[system]"
                ));
            } else {
                filters.push(format!(
                    "anullsrc=r=48000:cl=stereo,atrim=duration={duration}[system]"
                ));
            }
            audio_labels.push(("[system]", "System Audio"));
        }
        if segment.microphone_path.is_some() {
            let delay = segment.microphone_offset_ms.max(0);
            filters.push(format!(
                "[1:a:0]adelay={delay}:all=1,aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration}[microphone]"
            ));
        } else {
            filters.push(format!(
                "anullsrc=r=48000:cl=stereo,atrim=duration={duration}[microphone]"
            ));
        }
        audio_labels.push(("[microphone]", "Microphone"));

        if audio.system_audio {
            filters.push(
                "[system][microphone]amix=inputs=2:normalize=0:dropout_transition=0[playback]"
                    .to_owned(),
            );
            audio_labels.insert(0, ("[playback]", "System Audio + Microphone"));
        }

        command.args(["-filter_complex", &filters.join(";")]);
        command.args(["-map", "0:v:0", "-c:v", "copy"]);
        for (index, (label, title)) in audio_labels.iter().enumerate() {
            command.arg("-map").arg(label);
            command
                .arg(format!("-metadata:s:a:{index}"))
                .arg(format!("title={title}"));
        }
        command.args([
            "-c:a",
            "aac",
            "-ar",
            "48000",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
        ]);
        command.arg(destination);
        run_command(&mut command, cancel, "FFmpeg")
    }

    pub fn create_poster(
        &self,
        input: &Path,
        destination: &Path,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let temporary = temporary_output_path(destination, "poster.png");
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", "0", "-i"])
            .arg(input)
            .args(["-frames:v", "1", "-vf", "scale='min(960,iw)':-2"])
            .arg(&temporary);
        let result = run_command(&mut command, cancel, "FFmpeg")
            .and_then(|()| commit_temporary(&temporary, destination));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn create_timeline_sprite(
        &self,
        input: &Path,
        destination: &Path,
        spec: TimelineSpriteSpec,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        if spec.duration_ms == 0
            || spec.frame_count == 0
            || spec.frame_width < 2
            || spec.frame_height < 2
        {
            return Err(MediaToolError::InvalidEdit(
                "timeline preview dimensions and duration must be greater than zero".to_owned(),
            ));
        }
        let temporary = temporary_output_path(destination, "timeline");
        let frames_per_second = f64::from(spec.frame_count) * 1_000.0 / spec.duration_ms as f64;
        let filter = format!(
            "fps={frames_per_second:.6},scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},tile={}x1",
            spec.frame_width,
            spec.frame_height,
            spec.frame_width,
            spec.frame_height,
            spec.frame_count,
        );
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(input)
            .args(["-frames:v", "1", "-vf", &filter])
            .arg(&temporary);
        let result = run_command(&mut command, cancel, "FFmpeg")
            .and_then(|()| commit_temporary(&temporary, destination));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn create_gif(
        &self,
        input: &Path,
        destination: &Path,
        frames_per_second: u16,
        max_width: u32,
        max_colors: u16,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let temporary = temporary_output_path(destination, "gif");
        let filter = gif_filter(frames_per_second, max_width, max_colors);
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(input)
            .args(["-filter_complex", &filter, "-loop", "0"])
            .arg(&temporary);
        let result = run_command(&mut command, cancel, "FFmpeg")
            .and_then(|()| commit_temporary(&temporary, destination));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn export<F>(
        &self,
        input: &Path,
        destination: &Path,
        edit: &EditSpec,
        spec: &ExportSpec,
        cancel: &CancelToken,
        mut on_progress: F,
    ) -> Result<ExportOutcome, MediaToolError>
    where
        F: FnMut(ExportProgress),
    {
        on_progress(progress(ExportStage::Preparing, 0, 0, None));
        let probe = self.probe(input)?;
        validate_edit_spec(&probe, edit)?;
        let attempts = export_attempts(&probe, edit, spec)?;
        if spec.format == ExportFormat::Mp4
            && spec.quality == QualityPreset::Preserve
            && visual_edit_is_identity(&probe, edit)
            && spec
                .max_size_bytes
                .is_none_or(|maximum| probe.metadata.size_bytes <= maximum)
        {
            if cancel.is_cancelled() {
                on_progress(progress(ExportStage::Cancelled, 0, 0, None));
                return Err(MediaToolError::Cancelled);
            }
            on_progress(progress(
                ExportStage::Encoding,
                100,
                1,
                Some(if audio_edit_is_identity(edit) {
                    "Copying original recording".to_owned()
                } else {
                    "Saving audio changes".to_owned()
                }),
            ));
            if audio_edit_is_identity(edit) {
                atomic_copy(input, destination)?;
            } else {
                self.run_audio_only_export(
                    input,
                    destination,
                    edit,
                    attempts.first().ok_or(MediaToolError::IncompleteMetadata)?,
                    cancel,
                )?;
            }
            let size_bytes = fs::metadata(destination)?.len();
            on_progress(progress(ExportStage::Complete, 1_000, 1, None));
            return Ok(ExportOutcome {
                path: destination.to_path_buf(),
                size_bytes,
                attempts: 1,
            });
        }
        let max_attempts = if spec.max_size_bytes.is_some() { 4 } else { 1 };

        for (index, attempt) in attempts.into_iter().take(max_attempts).enumerate() {
            if cancel.is_cancelled() {
                on_progress(progress(ExportStage::Cancelled, 0, index as u8, None));
                return Err(MediaToolError::Cancelled);
            }
            let attempt_number = u8::try_from(index + 1).unwrap_or(u8::MAX);
            on_progress(progress(
                ExportStage::Encoding,
                100,
                attempt_number,
                Some(format!("Encoding attempt {attempt_number}")),
            ));
            let temporary =
                temporary_output_path(destination, &format!("attempt-{attempt_number}"));
            let result = self.run_export_attempt(input, &temporary, edit, spec, &attempt, cancel);
            if let Err(error) = result {
                let _ = fs::remove_file(&temporary);
                if matches!(error, MediaToolError::Cancelled) {
                    on_progress(progress(ExportStage::Cancelled, 0, attempt_number, None));
                }
                return Err(error);
            }
            on_progress(progress(ExportStage::Verifying, 900, attempt_number, None));
            let size_bytes = fs::metadata(&temporary)?.len();
            let fits = spec
                .max_size_bytes
                .is_none_or(|maximum| size_bytes <= maximum);
            if fits {
                commit_temporary(&temporary, destination)?;
                on_progress(progress(ExportStage::Complete, 1_000, attempt_number, None));
                return Ok(ExportOutcome {
                    path: destination.to_path_buf(),
                    size_bytes,
                    attempts: attempt_number,
                });
            }
            let _ = fs::remove_file(temporary);
        }

        on_progress(progress(
            ExportStage::Failed,
            1_000,
            u8::try_from(max_attempts).unwrap_or(u8::MAX),
            Some("The target is too small; trim the recording and try again.".to_owned()),
        ));
        Err(MediaToolError::UnattainableTarget)
    }

    fn run_export_attempt(
        &self,
        input: &Path,
        output: &Path,
        edit: &EditSpec,
        spec: &ExportSpec,
        attempt: &VideoAttempt,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        if spec.format == ExportFormat::Gif {
            return self.run_gif_export(input, output, edit, attempt, cancel);
        }
        if spec.format == ExportFormat::WebM {
            return Err(MediaToolError::Process(
                "WebM export is not available in the bundled media tools".to_owned(),
            ));
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            self.run_openh264_export(input, output, edit, spec, attempt, cancel)
        }
        #[cfg(target_os = "macos")]
        {
            self.run_videotoolbox_export(input, output, edit, spec, attempt, cancel)
        }
    }

    #[cfg(target_os = "macos")]
    fn run_videotoolbox_export(
        &self,
        input: &Path,
        output: &Path,
        edit: &EditSpec,
        spec: &ExportSpec,
        attempt: &VideoAttempt,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y"]);
        if edit.trim_start_ms > 0 {
            command.args(["-ss", &seconds(edit.trim_start_ms)]);
        }
        command.arg("-i").arg(input);
        if let Some(trim_end_ms) = edit.trim_end_ms {
            command.args([
                "-to",
                &seconds(trim_end_ms.saturating_sub(edit.trim_start_ms)),
            ]);
        }
        let video_filter = video_filter(
            edit,
            attempt.width,
            attempt.height,
            attempt.frames_per_second,
        );
        if !video_filter.is_empty() {
            command.args(["-vf", &video_filter]);
        }
        command.args(["-map", "0:v:0"]);
        // VideoToolbox selects hardware when it is available. Its explicit
        // software fallback keeps exports working in VMs and when the hardware
        // encoder is temporarily busy without changing the H.264 container.
        command.args(["-c:v", "h264_videotoolbox", "-allow_sw", "1"]);
        if let Some(video_bitrate) = attempt.video_bitrate {
            command.args([
                "-b:v",
                &video_bitrate.to_string(),
                "-maxrate",
                &video_bitrate.to_string(),
            ]);
        } else {
            let quality = match spec.quality {
                QualityPreset::Preserve | QualityPreset::High => "90",
                QualityPreset::Standard => "70",
                QualityPreset::Small => "50",
            };
            command.args(["-q:v", quality]);
        }
        if attempt.has_audio {
            let audio_filter = audio_filter(edit, attempt)?;
            command.args(["-filter_complex", &audio_filter, "-map", "[audio_out]"]);
            command.args(["-c:a", "aac", "-b:a", &attempt.audio_bitrate.to_string()]);
            if edit.audio.mono_output || attempt.force_mono {
                command.args(["-ac", "1"]);
            }
        } else {
            command.arg("-an");
        }
        command.args(["-movflags", "+faststart"]).arg(output);
        run_command(&mut command, cancel, "FFmpeg")
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn run_openh264_export(
        &self,
        input: &Path,
        output: &Path,
        edit: &EditSpec,
        spec: &ExportSpec,
        attempt: &VideoAttempt,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let encoded_video = if attempt.has_audio {
            temporary_output_path(output, "openh264-video")
        } else {
            output.to_path_buf()
        };
        let result = self
            .encode_openh264_video(input, &encoded_video, edit, spec, attempt, cancel)
            .and_then(|()| {
                if attempt.has_audio {
                    self.mux_openh264_audio(input, &encoded_video, output, edit, attempt, cancel)
                } else {
                    Ok(())
                }
            });
        if result.is_err() {
            let _ = fs::remove_file(output);
        }
        if attempt.has_audio {
            let _ = fs::remove_file(encoded_video);
        }
        result
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn encode_openh264_video(
        &self,
        input: &Path,
        output: &Path,
        edit: &EditSpec,
        spec: &ExportSpec,
        attempt: &VideoAttempt,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        let (width, height) = fit_openh264_dimensions(attempt.width, attempt.height);
        let mut writer = H264Mp4Writer::create(
            output,
            width,
            height,
            attempt.frames_per_second,
            openh264_bitrate(attempt, spec),
        )
        .map_err(|error| MediaToolError::Process(error.to_string()))?;
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y"]);
        if edit.trim_start_ms > 0 {
            command.args(["-ss", &seconds(edit.trim_start_ms)]);
        }
        command.arg("-i").arg(input);
        if let Some(trim_end_ms) = edit.trim_end_ms {
            command.args([
                "-to",
                &seconds(trim_end_ms.saturating_sub(edit.trim_start_ms)),
            ]);
        }
        let video_filter = video_filter(edit, width, height, attempt.frames_per_second);
        command
            .args(["-vf", &video_filter, "-map", "0:v:0", "-an"])
            .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| map_spawn_error(error, "FFmpeg"))?;
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to read decoded video frames".to_owned(),
            ));
        };
        let Some(mut stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to capture media tool errors".to_owned(),
            ));
        };
        let mut stderr_reader = Some(thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        }));
        let frame_size = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or(MediaToolError::IncompleteMetadata)?;
        let mut frame = vec![0_u8; frame_size];
        let mut frame_index = 0_u64;
        let result = (|| -> Result<(), MediaToolError> {
            while read_complete_frame(&mut stdout, &mut frame)? {
                if cancel.is_cancelled() {
                    return Err(MediaToolError::Cancelled);
                }
                let timestamp_ms =
                    frame_index.saturating_mul(1_000) / u64::from(attempt.frames_per_second);
                writer
                    .encode_rgb(&frame, timestamp_ms)
                    .map_err(|error| MediaToolError::Process(error.to_string()))?;
                frame_index = frame_index.saturating_add(1);
            }
            let status = child.wait()?;
            let stderr = stderr_reader
                .take()
                .ok_or_else(|| {
                    MediaToolError::Process("media tool error reader was missing".to_owned())
                })?
                .join()
                .map_err(|_| {
                    MediaToolError::Process("media tool error reader panicked".to_owned())
                })??;
            complete_child(status, &stderr)?;
            let duration_ms =
                frame_index.saturating_mul(1_000) / u64::from(attempt.frames_per_second);
            writer
                .finish(duration_ms.max(1))
                .map_err(|error| MediaToolError::Process(error.to_string()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(stderr_reader) = stderr_reader.take() {
                let _ = stderr_reader.join();
            }
            let _ = fs::remove_file(output);
        }
        result
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn mux_openh264_audio(
        &self,
        input: &Path,
        encoded_video: &Path,
        output: &Path,
        edit: &EditSpec,
        attempt: &VideoAttempt,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y"]);
        if edit.trim_start_ms > 0 {
            command.args(["-ss", &seconds(edit.trim_start_ms)]);
        }
        command.arg("-i").arg(input).arg("-i").arg(encoded_video);
        if let Some(trim_end_ms) = edit.trim_end_ms {
            command.args([
                "-t",
                &seconds(trim_end_ms.saturating_sub(edit.trim_start_ms)),
            ]);
        }
        let audio_filter = audio_filter(edit, attempt)?;
        command
            .args([
                "-filter_complex",
                &audio_filter,
                "-map",
                "1:v:0",
                "-map",
                "[audio_out]",
                "-c:v",
                "copy",
                "-c:a",
                "aac",
                "-b:a",
                &attempt.audio_bitrate.to_string(),
            ])
            .args(["-movflags", "+faststart"]);
        if edit.audio.mono_output || attempt.force_mono {
            command.args(["-ac", "1"]);
        }
        command.arg(output);
        run_command(&mut command, cancel, "FFmpeg")
    }

    fn run_audio_only_export(
        &self,
        input: &Path,
        destination: &Path,
        edit: &EditSpec,
        attempt: &VideoAttempt,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let temporary = temporary_output_path(destination, "audio-edit");
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(input)
            .args(["-map", "0:v:0", "-c:v", "copy"]);
        if attempt.has_audio {
            let audio_filter = audio_filter(edit, attempt)?;
            command
                .args(["-filter_complex", &audio_filter, "-map", "[audio_out]"])
                .args(["-c:a", "aac", "-b:a", &attempt.audio_bitrate.to_string()]);
            if edit.audio.mono_output || attempt.force_mono {
                command.args(["-ac", "1"]);
            }
        } else {
            command.arg("-an");
        }
        command.args(["-movflags", "+faststart"]).arg(&temporary);
        let result = run_command(&mut command, cancel, "FFmpeg")
            .and_then(|()| commit_temporary(&temporary, destination));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn run_gif_export(
        &self,
        input: &Path,
        output: &Path,
        edit: &EditSpec,
        attempt: &VideoAttempt,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y"]);
        if edit.trim_start_ms > 0 {
            command.args(["-ss", &seconds(edit.trim_start_ms)]);
        }
        command.arg("-i").arg(input);
        if let Some(trim_end_ms) = edit.trim_end_ms {
            command.args([
                "-to",
                &seconds(trim_end_ms.saturating_sub(edit.trim_start_ms)),
            ]);
        }
        let filter = gif_export_filter(edit, attempt);
        command.args(["-filter_complex", &filter, "-loop", "0"]);
        command.arg(output);
        run_command(&mut command, cancel, "FFmpeg")
    }
}

#[derive(Clone, Copy, Debug)]
struct VideoAttempt {
    width: u32,
    height: u32,
    frames_per_second: u16,
    video_bitrate: Option<u64>,
    audio_bitrate: u64,
    has_audio: bool,
    system_audio: bool,
    microphone_audio: bool,
    audio_stream_count: usize,
    force_mono: bool,
    gif_colors: u16,
}

#[derive(Clone, Copy, Debug, Default)]
struct AttemptAudio {
    has_audio: bool,
    system_audio: bool,
    microphone_audio: bool,
    stream_count: usize,
}

fn export_attempts(
    probe: &ProbeResult,
    edit: &EditSpec,
    spec: &ExportSpec,
) -> Result<Vec<VideoAttempt>, MediaToolError> {
    let cropped_width = edit.crop.map_or(probe.metadata.width, |crop| crop.width);
    let cropped_height = edit.crop.map_or(probe.metadata.height, |crop| crop.height);
    let source_width = edit.output_width.unwrap_or(cropped_width);
    let source_height = edit.output_height.unwrap_or(cropped_height);
    let source_fps = if spec.format == ExportFormat::Gif {
        spec.frames_per_second.unwrap_or(15).clamp(1, 30)
    } else {
        spec.frames_per_second.unwrap_or(30).clamp(15, 60)
    };
    let source_has_microphone_audio = edit.audio.source_has_microphone_audio;
    let source_has_system_audio =
        edit.audio.source_has_system_audio || (probe.has_audio && !source_has_microphone_audio);
    let has_audio = probe.has_audio
        && ((source_has_system_audio && !edit.audio.mute_system_audio)
            || (source_has_microphone_audio && !edit.audio.mute_microphone))
        && spec.format != ExportFormat::Gif;
    let duration_ms = edit
        .trim_end_ms
        .unwrap_or(
            probe
                .metadata
                .duration_ms
                .ok_or(MediaToolError::IncompleteMetadata)?,
        )
        .saturating_sub(edit.trim_start_ms);
    let gif_colors = spec.gif_max_colors.unwrap_or(256).clamp(64, 256);
    let source_audio = AttemptAudio {
        has_audio,
        system_audio: source_has_system_audio,
        microphone_audio: source_has_microphone_audio,
        stream_count: probe.audio_stream_count,
    };

    if spec.format == ExportFormat::Gif {
        return Ok(vec![
            video_attempt(
                (source_width, source_height),
                source_fps,
                None,
                0,
                AttemptAudio::default(),
                gif_colors,
            ),
            video_attempt(
                (source_width, source_height),
                source_fps,
                None,
                0,
                AttemptAudio::default(),
                gif_colors.min(128),
            ),
            video_attempt(
                (source_width, source_height),
                source_fps.min(12),
                None,
                0,
                AttemptAudio::default(),
                gif_colors.min(96),
            ),
            video_attempt(
                (source_width.min(320), source_height),
                source_fps.min(8),
                None,
                0,
                AttemptAudio::default(),
                gif_colors.min(64),
            ),
        ]);
    }

    let Some(max_size_bytes) = spec.max_size_bytes else {
        return Ok(vec![video_attempt(
            (source_width, source_height),
            source_fps,
            None,
            128_000,
            source_audio,
            gif_colors,
        )]);
    };
    let budget = calculate_size_budget(max_size_bytes, duration_ms, has_audio)?;
    let dimensions = [
        fit_even(source_width, source_height, source_height),
        fit_even(source_width, source_height, source_height),
        fit_even(source_width, source_height, 720),
        fit_even(source_width, source_height, 480),
    ];
    let frames = [source_fps, source_fps, source_fps.min(30), 15];
    let video_bitrates = [
        budget.video_bitrate,
        (budget.video_bitrate.saturating_mul(85) / 100).max(MIN_VIDEO_BITRATE),
        (budget.video_bitrate.saturating_mul(70) / 100).max(MIN_VIDEO_BITRATE),
        (budget.video_bitrate.saturating_mul(60) / 100).max(MIN_VIDEO_BITRATE),
    ];
    let audio_bitrates = [
        budget.audio_bitrate,
        budget.audio_bitrate,
        budget.audio_bitrate,
        if has_audio {
            budget.audio_bitrate.clamp(MIN_AUDIO_BITRATE, 64_000)
        } else {
            0
        },
    ];
    Ok((0..4)
        .map(|index| {
            let mut attempt = video_attempt(
                dimensions[index],
                frames[index],
                Some(video_bitrates[index]),
                audio_bitrates[index],
                source_audio,
                gif_colors,
            );
            attempt.force_mono = has_audio && index == 3;
            attempt
        })
        .collect())
}

fn video_attempt(
    dimensions: (u32, u32),
    frames_per_second: u16,
    video_bitrate: Option<u64>,
    audio_bitrate: u64,
    audio: AttemptAudio,
    gif_colors: u16,
) -> VideoAttempt {
    let (width, height) = fit_even(dimensions.0, dimensions.1, dimensions.1);
    VideoAttempt {
        width,
        height,
        frames_per_second,
        video_bitrate,
        audio_bitrate,
        has_audio: audio.has_audio,
        system_audio: audio.system_audio,
        microphone_audio: audio.microphone_audio,
        audio_stream_count: audio.stream_count,
        force_mono: false,
        gif_colors,
    }
}

fn validate_edit_spec(probe: &ProbeResult, edit: &EditSpec) -> Result<(), MediaToolError> {
    let duration_ms = probe
        .metadata
        .duration_ms
        .ok_or(MediaToolError::IncompleteMetadata)?;
    let trim_end_ms = edit.trim_end_ms.unwrap_or(duration_ms);
    if edit.trim_start_ms >= trim_end_ms || trim_end_ms > duration_ms {
        return Err(MediaToolError::InvalidEdit(
            "trim bounds must select time within the source recording".to_owned(),
        ));
    }

    if let Some(crop) = edit.crop {
        let right = crop.x.checked_add(crop.width);
        let bottom = crop.y.checked_add(crop.height);
        if crop.width < 2
            || crop.height < 2
            || right.is_none_or(|value| value > probe.metadata.width)
            || bottom.is_none_or(|value| value > probe.metadata.height)
        {
            return Err(MediaToolError::InvalidEdit(
                "crop bounds must stay within the source recording".to_owned(),
            ));
        }
    }

    match (edit.output_width, edit.output_height) {
        (None, None) => {}
        (Some(width), Some(height)) if width >= 2 && height >= 2 => {}
        _ => {
            return Err(MediaToolError::InvalidEdit(
                "output width and height must both be at least two pixels".to_owned(),
            ));
        }
    }
    Ok(())
}

fn visual_edit_is_identity(probe: &ProbeResult, edit: &EditSpec) -> bool {
    edit.trim_start_ms == 0
        && edit
            .trim_end_ms
            .is_none_or(|end| Some(end) == probe.metadata.duration_ms)
        && edit.crop.is_none()
        && edit.output_width.is_none()
        && edit.output_height.is_none()
}

fn audio_edit_is_identity(edit: &EditSpec) -> bool {
    !edit.audio.mute_system_audio
        && !edit.audio.mute_microphone
        && !edit.audio.mono_output
        && (edit.audio.system_volume - 1.0).abs() < f32::EPSILON
        && (edit.audio.microphone_volume - 1.0).abs() < f32::EPSILON
}

fn fit_even(width: u32, height: u32, maximum_height: u32) -> (u32, u32) {
    let scale = if height > maximum_height {
        f64::from(maximum_height) / f64::from(height)
    } else {
        1.0
    };
    let width = (f64::from(width) * scale).round().max(2.0) as u32 & !1;
    let height = (f64::from(height) * scale).round().max(2.0) as u32 & !1;
    (width, height)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn fit_openh264_dimensions(width: u32, height: u32) -> (u32, u32) {
    let (maximum_width, maximum_height) = if width >= height {
        (3_840.0, 2_160.0)
    } else {
        (2_160.0, 3_840.0)
    };
    let scale = (maximum_width / f64::from(width.max(1)))
        .min(maximum_height / f64::from(height.max(1)))
        .min(1.0);
    (
        ((f64::from(width) * scale).floor() as u32 & !1).max(2),
        ((f64::from(height) * scale).floor() as u32 & !1).max(2),
    )
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn openh264_bitrate(attempt: &VideoAttempt, spec: &ExportSpec) -> u32 {
    let bits_per_pixel_percent = match spec.quality {
        QualityPreset::Preserve | QualityPreset::High => 12_u64,
        QualityPreset::Standard => 8,
        QualityPreset::Small => 5,
    };
    let estimated = u64::from(attempt.width)
        .saturating_mul(u64::from(attempt.height))
        .saturating_mul(u64::from(attempt.frames_per_second))
        .saturating_mul(bits_per_pixel_percent)
        / 100;
    let bitrate = attempt.video_bitrate.unwrap_or(estimated);
    u32::try_from(bitrate.clamp(250_000, 50_000_000)).unwrap_or(50_000_000)
}

fn video_filter(edit: &EditSpec, width: u32, height: u32, fps: u16) -> String {
    let mut filters = Vec::new();
    if let Some(crop) = edit.crop {
        filters.push(format!(
            "crop={}:{}:{}:{}",
            crop.width, crop.height, crop.x, crop.y
        ));
    }
    filters.push(format!("scale={width}:{height}:flags=lanczos"));
    filters.push(format!("fps={fps}"));
    filters.join(",")
}

fn audio_filter(edit: &EditSpec, attempt: &VideoAttempt) -> Result<String, MediaToolError> {
    let audio = &edit.audio;
    let mono_output = audio.mono_output || attempt.force_mono;
    if attempt.system_audio && attempt.microphone_audio && attempt.audio_stream_count < 2 {
        let unchanged = !audio.mute_system_audio
            && !audio.mute_microphone
            && (audio.system_volume - audio.microphone_volume).abs() < f32::EPSILON;
        if !unchanged {
            return Err(MediaToolError::SeparateAudioUnavailable);
        }
        let volume = audio.system_volume.clamp(0.0, 2.0);
        return Ok(single_audio_filter(0, volume, mono_output));
    }

    let mut filters = Vec::new();
    let mut labels = Vec::new();
    let separate_track_offset = usize::from(
        attempt.system_audio && attempt.microphone_audio && attempt.audio_stream_count >= 3,
    );
    if attempt.system_audio && !audio.mute_system_audio {
        filters.push(format!(
            "[0:a:{separate_track_offset}]volume={:.3},aresample=48000:async=1:first_pts=0[system_edit]",
            audio.system_volume.clamp(0.0, 2.0)
        ));
        labels.push("[system_edit]");
    }
    if attempt.microphone_audio && !audio.mute_microphone {
        let index = separate_track_offset + usize::from(attempt.system_audio);
        filters.push(format!(
            "[0:a:{index}]volume={:.3},aresample=48000:async=1:first_pts=0[microphone_edit]",
            audio.microphone_volume.clamp(0.0, 2.0)
        ));
        labels.push("[microphone_edit]");
    }
    match labels.as_slice() {
        [] => Err(MediaToolError::IncompleteMetadata),
        [label] => {
            let mono = if mono_output {
                ",aformat=channel_layouts=mono"
            } else {
                ""
            };
            filters.push(format!("{label}anull{mono}[audio_out]"));
            Ok(filters.join(";"))
        }
        _ => {
            let mono = if mono_output {
                ",aformat=channel_layouts=mono"
            } else {
                ""
            };
            filters.push(format!(
                "{}amix=inputs={}:normalize=0{mono}[audio_out]",
                labels.join(""),
                labels.len()
            ));
            Ok(filters.join(";"))
        }
    }
}

fn single_audio_filter(index: usize, volume: f32, mono: bool) -> String {
    let mono = if mono {
        ",aformat=channel_layouts=mono"
    } else {
        ""
    };
    format!("[0:a:{index}]volume={volume:.3},aresample=48000:async=1:first_pts=0{mono}[audio_out]")
}

fn gif_export_filter(edit: &EditSpec, attempt: &VideoAttempt) -> String {
    let crop = edit.crop.map_or_else(String::new, |crop| {
        format!("crop={}:{}:{}:{},", crop.width, crop.height, crop.x, crop.y)
    });
    format!(
        "{crop}fps={},scale='min({},iw)':-2:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors={}:stats_mode=diff[p];[s1][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle",
        attempt.frames_per_second, attempt.width, attempt.gif_colors
    )
}

fn gif_filter(frames_per_second: u16, max_width: u32, max_colors: u16) -> String {
    format!(
        "fps={frames_per_second},scale='min({max_width},iw)':-2:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors={max_colors}:stats_mode=diff[p];[s1][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle"
    )
}

fn run_command(
    command: &mut Command,
    cancel: &CancelToken,
    tool: &'static str,
) -> Result<(), MediaToolError> {
    if cancel.is_cancelled() {
        return Err(MediaToolError::Cancelled);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| map_spawn_error(error, tool))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| MediaToolError::Process("failed to capture media tool errors".to_owned()))?;
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr_reader.join();
            return Err(MediaToolError::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            let stderr = stderr_reader.join().map_err(|_| {
                MediaToolError::Process("media tool error reader panicked".to_owned())
            })??;
            return complete_child(status, &stderr);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn read_complete_frame(reader: &mut impl Read, frame: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < frame.len() {
        match reader.read(&mut frame[filled..])? {
            0 if filled == 0 => return Ok(false),
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "FFmpeg returned a partial raw video frame",
                ));
            }
            count => filled += count,
        }
    }
    Ok(true)
}

fn complete_child(status: ExitStatus, stderr: &[u8]) -> Result<(), MediaToolError> {
    if status.success() {
        Ok(())
    } else {
        Err(MediaToolError::Process(process_message(stderr)))
    }
}

fn verify_tool(path: &Path, name: &'static str) -> Result<(), MediaToolError> {
    let status = Command::new(path)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| MediaToolError::ToolUnavailable(name))?;
    status
        .success()
        .then_some(())
        .ok_or(MediaToolError::ToolUnavailable(name))
}

fn atomic_copy(source: &Path, destination: &Path) -> Result<(), MediaToolError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = temporary_output_path(destination, "copy");
    fs::copy(source, &temporary)?;
    commit_temporary(&temporary, destination)
}

fn commit_temporary(temporary: &Path, destination: &Path) -> Result<(), MediaToolError> {
    if destination.exists() {
        return Err(MediaToolError::Process(format!(
            "refusing to replace existing file {}",
            destination.display()
        )));
    }
    fs::rename(temporary, destination)?;
    Ok(())
}

fn temporary_output_path(destination: &Path, suffix: &str) -> PathBuf {
    static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(1);
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let extension = destination
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("tmp");
    parent.join(format!(
        ".captures-{}-{}-{suffix}.{extension}",
        std::process::id(),
        NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed),
    ))
}

fn escape_concat_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "'\\''")
}

fn seconds(milliseconds: u64) -> String {
    format!("{}.{:03}", milliseconds / 1_000, milliseconds % 1_000)
}

fn process_message(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr).trim().to_owned();
    if message.is_empty() {
        "the media tool exited unsuccessfully".to_owned()
    } else {
        message
    }
}

fn map_spawn_error(error: std::io::Error, tool: &'static str) -> MediaToolError {
    if error.kind() == std::io::ErrorKind::NotFound {
        MediaToolError::ToolUnavailable(tool)
    } else {
        MediaToolError::Io(error)
    }
}

fn progress(
    stage: ExportStage,
    completed_per_mille: u16,
    attempt: u8,
    message: Option<String>,
) -> ExportProgress {
    ExportProgress {
        stage,
        completed_per_mille,
        attempt,
        message,
    }
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    #[serde(default)]
    format: FfprobeFormat,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    size: Option<String>,
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::TimelineSpriteSpec;
    use super::{
        CancelToken, MediaToolchain, VideoAttempt, audio_edit_is_identity, audio_filter,
        escape_concat_path, export_attempts, fit_even, gif_export_filter, gif_filter, seconds,
        validate_edit_spec, visual_edit_is_identity,
    };
    use crate::{
        AudioEdit, CropRect, EditSpec, ExportFormat, ExportSpec, MediaKind, MediaMetadata,
        QualityPreset, toolchain::ProbeResult,
    };

    fn probe() -> ProbeResult {
        ProbeResult {
            metadata: MediaMetadata {
                kind: MediaKind::Video,
                mime_type: "video/mp4".to_owned(),
                width: 1_920,
                height: 1_080,
                duration_ms: Some(60_000),
                size_bytes: 10_000_000,
            },
            has_audio: true,
            audio_stream_count: 2,
        }
    }

    #[test]
    fn targeted_video_attempts_reach_the_quality_floor_in_order() {
        let attempts = export_attempts(
            &probe(),
            &EditSpec::default(),
            &ExportSpec {
                format: ExportFormat::Mp4,
                quality: QualityPreset::Standard,
                max_size_bytes: Some(10_000_000),
                frames_per_second: Some(60),
                gif_max_colors: None,
            },
        )
        .expect("attempts");
        assert_eq!(attempts.len(), 4);
        assert_eq!((attempts[0].width, attempts[0].height), (1_920, 1_080));
        assert_eq!((attempts[1].width, attempts[1].height), (1_920, 1_080));
        assert_eq!(attempts[1].frames_per_second, 60);
        assert_eq!((attempts[2].width, attempts[2].height), (1_280, 720));
        assert_eq!(attempts[2].frames_per_second, 30);
        assert_eq!((attempts[3].width, attempts[3].height), (852, 480));
        assert_eq!(attempts[3].frames_per_second, 15);
        assert_eq!(attempts[3].audio_bitrate, 64_000);
        assert!(attempts[3].force_mono);
    }

    #[test]
    fn muted_audio_is_not_budgeted() {
        let edit = EditSpec {
            audio: AudioEdit {
                mute_system_audio: true,
                mute_microphone: true,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };
        let attempts = export_attempts(
            &probe(),
            &edit,
            &ExportSpec {
                format: ExportFormat::Mp4,
                quality: QualityPreset::Standard,
                max_size_bytes: Some(10_000_000),
                frames_per_second: None,
                gif_max_colors: None,
            },
        )
        .expect("attempts");
        assert!(!attempts[0].has_audio);
        assert_eq!(attempts[0].audio_bitrate, 0);
    }

    #[test]
    fn helpers_keep_ffmpeg_arguments_deterministic() {
        assert_eq!(fit_even(1_919, 1_079, 720), (1_280, 720));
        assert_eq!(seconds(61_042), "61.042");
        assert_eq!(
            escape_concat_path(std::path::Path::new("a'b.mp4")),
            "a'\\''b.mp4"
        );
        assert!(gif_filter(15, 800, 256).contains("palettegen=max_colors=256"));
    }

    #[test]
    fn separate_audio_tracks_keep_independent_controls() {
        let edit = EditSpec {
            audio: AudioEdit {
                system_volume: 0.5,
                microphone_volume: 1.5,
                source_has_system_audio: true,
                source_has_microphone_audio: true,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };
        let filter = audio_filter(
            &edit,
            &VideoAttempt {
                width: 1_920,
                height: 1_080,
                frames_per_second: 30,
                video_bitrate: None,
                audio_bitrate: 128_000,
                has_audio: true,
                system_audio: true,
                microphone_audio: true,
                audio_stream_count: 3,
                force_mono: false,
                gif_colors: 256,
            },
        )
        .expect("audio filter");
        assert!(filter.contains("[0:a:1]volume=0.500"));
        assert!(filter.contains("[0:a:2]volume=1.500"));
        assert!(filter.contains("amix=inputs=2"));
    }

    #[test]
    fn gif_export_preserves_crop_and_duration_controls() {
        let filter = gif_export_filter(
            &EditSpec {
                crop: Some(CropRect {
                    x: 10,
                    y: 20,
                    width: 640,
                    height: 360,
                }),
                ..EditSpec::default()
            },
            &VideoAttempt {
                width: 640,
                height: 360,
                frames_per_second: 12,
                video_bitrate: None,
                audio_bitrate: 0,
                has_audio: false,
                system_audio: false,
                microphone_audio: false,
                audio_stream_count: 0,
                force_mono: false,
                gif_colors: 128,
            },
        );
        assert!(filter.starts_with("crop=640:360:10:20,fps=12"));
        assert!(filter.contains("palettegen=max_colors=128"));
    }

    #[test]
    fn strict_size_floor_forces_audio_to_mono() {
        let edit = EditSpec {
            audio: AudioEdit {
                source_has_system_audio: true,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };
        let attempts = export_attempts(
            &probe(),
            &edit,
            &ExportSpec {
                format: ExportFormat::Mp4,
                quality: QualityPreset::Standard,
                max_size_bytes: Some(10_000_000),
                frames_per_second: Some(60),
                gif_max_colors: None,
            },
        )
        .expect("attempts");
        let filter =
            audio_filter(&edit, attempts.last().expect("floor attempt")).expect("audio filter");
        assert!(filter.contains("channel_layouts=mono"));
    }

    #[test]
    fn edit_validation_rejects_out_of_bounds_requests() {
        let invalid_crop = EditSpec {
            crop: Some(CropRect {
                x: 1_900,
                y: 0,
                width: 100,
                height: 100,
            }),
            ..EditSpec::default()
        };
        assert!(validate_edit_spec(&probe(), &invalid_crop).is_err());

        let invalid_trim = EditSpec {
            trim_start_ms: 20_000,
            trim_end_ms: Some(20_000),
            ..EditSpec::default()
        };
        assert!(validate_edit_spec(&probe(), &invalid_trim).is_err());

        let incomplete_dimensions = EditSpec {
            output_width: Some(640),
            output_height: None,
            ..EditSpec::default()
        };
        assert!(validate_edit_spec(&probe(), &incomplete_dimensions).is_err());
    }

    #[test]
    fn preserve_identity_requires_no_visual_or_audio_changes() {
        let untouched = EditSpec::default();
        assert!(visual_edit_is_identity(&probe(), &untouched));
        assert!(audio_edit_is_identity(&untouched));

        let audio_edit = EditSpec {
            audio: AudioEdit {
                system_volume: 0.75,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };
        assert!(visual_edit_is_identity(&probe(), &audio_edit));
        assert!(!audio_edit_is_identity(&audio_edit));

        let crop = EditSpec {
            crop: Some(CropRect {
                x: 0,
                y: 0,
                width: 1_280,
                height: 720,
            }),
            ..EditSpec::default()
        };
        assert!(!visual_edit_is_identity(&probe(), &crop));
    }

    #[cfg(target_os = "macos")]
    fn bundled_toolchain() -> (MediaToolchain, std::path::PathBuf) {
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        let ffmpeg = repository.join("apps/desktop/src-tauri/binaries/ffmpeg-aarch64-apple-darwin");
        let ffprobe =
            repository.join("apps/desktop/src-tauri/binaries/ffprobe-aarch64-apple-darwin");
        (MediaToolchain::new(ffmpeg.clone(), ffprobe), ffmpeg)
    }

    #[cfg(target_os = "macos")]
    fn media_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("media test lock")
    }

    #[cfg(target_os = "macos")]
    fn video_toolbox_available() -> bool {
        let (_, ffmpeg) = bundled_toolchain();
        std::process::Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x64:rate=1",
                "-frames:v",
                "1",
                "-c:v",
                "h264_videotoolbox",
                "-allow_sw",
                "1",
                "-f",
                "null",
                "-",
            ])
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(target_os = "macos")]
    fn create_test_recording(path: &std::path::Path, with_audio: bool) {
        let (_, ffmpeg) = bundled_toolchain();
        let mut command = std::process::Command::new(ffmpeg);
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30",
        ]);
        if with_audio {
            command.args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"]);
        }
        command.args(["-t", "2", "-c:v", "mpeg4", "-q:v", "2"]);
        if with_audio {
            command.args(["-c:a", "aac", "-b:a", "128k", "-shortest"]);
        } else {
            command.arg("-an");
        }
        let status = command.arg(path).status().expect("bundled FFmpeg starts");
        assert!(status.success(), "test recording generated");
    }

    #[cfg(target_os = "macos")]
    fn preserve_spec(max_size_bytes: Option<u64>) -> ExportSpec {
        ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes,
            frames_per_second: None,
            gif_max_colors: None,
        }
    }

    #[cfg(target_os = "macos")]
    fn video_stream_hash(path: &std::path::Path) -> Vec<u8> {
        let (_, ffmpeg) = bundled_toolchain();
        let output = std::process::Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-map", "0:v:0", "-c", "copy", "-f", "md5", "-"])
            .output()
            .expect("video stream hash");
        assert!(output.status.success());
        output.stdout
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn preserve_export_is_byte_identical_when_untouched() {
        let _guard = media_test_guard();
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let destination = directory.path().join("copy.mp4");
        create_test_recording(&source, false);
        let toolchain = bundled_toolchain().0;

        toolchain
            .export(
                &source,
                &destination,
                &EditSpec::default(),
                &preserve_spec(None),
                &CancelToken::default(),
                |_| {},
            )
            .expect("preserve export");

        assert_eq!(
            std::fs::read(source).expect("source bytes"),
            std::fs::read(destination).expect("copy bytes")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn audio_only_edits_copy_the_video_stream() {
        let _guard = media_test_guard();
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let destination = directory.path().join("audio-edit.mp4");
        create_test_recording(&source, true);
        let toolchain = bundled_toolchain().0;
        let edit = EditSpec {
            audio: AudioEdit {
                system_volume: 0.5,
                source_has_system_audio: true,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };

        toolchain
            .export(
                &source,
                &destination,
                &edit,
                &preserve_spec(None),
                &CancelToken::default(),
                |_| {},
            )
            .expect("audio-only export");

        assert_eq!(video_stream_hash(&source), video_stream_hash(&destination));
        assert!(
            toolchain
                .probe(&destination)
                .expect("edited probe")
                .has_audio
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn visual_edits_use_the_high_quality_h264_path() {
        let _guard = media_test_guard();
        if !video_toolbox_available() {
            eprintln!("VideoToolbox is unavailable in this execution context");
            return;
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let destination = directory.path().join("crop.mp4");
        create_test_recording(&source, false);
        let toolchain = bundled_toolchain().0;
        let edit = EditSpec {
            crop: Some(CropRect {
                x: 10,
                y: 10,
                width: 300,
                height: 160,
            }),
            ..EditSpec::default()
        };

        toolchain
            .export(
                &source,
                &destination,
                &edit,
                &preserve_spec(None),
                &CancelToken::default(),
                |_| {},
            )
            .expect("high-quality crop");

        let metadata = toolchain
            .probe(&destination)
            .expect("cropped probe")
            .metadata;
        assert_eq!((metadata.width, metadata.height), (300, 160));
        assert_ne!(video_stream_hash(&source), video_stream_hash(&destination));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn timeline_sprite_contains_twelve_sampled_frames() {
        let _guard = media_test_guard();
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let destination = directory.path().join("timeline.png");
        create_test_recording(&source, false);
        let toolchain = bundled_toolchain().0;

        toolchain
            .create_timeline_sprite(
                &source,
                &destination,
                TimelineSpriteSpec {
                    duration_ms: 2_000,
                    frame_count: 12,
                    frame_width: 160,
                    frame_height: 90,
                },
                &CancelToken::default(),
            )
            .expect("timeline sprite");
        let png = std::fs::read(destination).expect("timeline bytes");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 1_920);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 90);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn maximum_size_export_never_exceeds_the_exact_ceiling() {
        let _guard = media_test_guard();
        if !video_toolbox_available() {
            eprintln!("VideoToolbox is unavailable in this execution context");
            return;
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let destination = directory.path().join("limited.mp4");
        create_test_recording(&source, true);
        let toolchain = bundled_toolchain().0;
        let mut edit = EditSpec::default();
        edit.audio.source_has_system_audio = true;
        let maximum = 200_000;

        let outcome = toolchain
            .export(
                &source,
                &destination,
                &edit,
                &preserve_spec(Some(maximum)),
                &CancelToken::default(),
                |_| {},
            )
            .expect("size-limited export");

        assert!(outcome.size_bytes <= maximum);
        assert!(std::fs::metadata(destination).unwrap().len() <= maximum);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn cross_platform_visual_edits_encode_h264_and_keep_audio() {
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        let target = if cfg!(target_os = "windows") {
            "x86_64-pc-windows-msvc.exe"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let ffmpeg = std::env::var_os("CAPTURES_TEST_FFMPEG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                repository
                    .join("apps/desktop/src-tauri/binaries")
                    .join(format!("ffmpeg-{target}"))
            });
        let ffprobe = std::env::var_os("CAPTURES_TEST_FFPROBE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                repository
                    .join("apps/desktop/src-tauri/binaries")
                    .join(format!("ffprobe-{target}"))
            });
        if !ffmpeg.is_file() || !ffprobe.is_file() {
            eprintln!("cross-platform media sidecars are not prepared in this checkout");
            return;
        }

        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let destination = directory.path().join("crop.mp4");
        let status = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "1",
                "-c:v",
                "mpeg4",
                "-q:v",
                "2",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source)
            .status()
            .expect("bundled FFmpeg starts");
        assert!(status.success(), "test recording generated");

        let mut edit = EditSpec {
            crop: Some(CropRect {
                x: 10,
                y: 10,
                width: 300,
                height: 160,
            }),
            ..EditSpec::default()
        };
        edit.audio.source_has_system_audio = true;
        let toolchain = MediaToolchain::new(ffmpeg, ffprobe.clone());
        toolchain
            .export(
                &source,
                &destination,
                &edit,
                &ExportSpec {
                    format: ExportFormat::Mp4,
                    quality: QualityPreset::Preserve,
                    max_size_bytes: None,
                    frames_per_second: Some(30),
                    gif_max_colors: None,
                },
                &CancelToken::default(),
                |_| {},
            )
            .expect("OpenH264 crop and audio mux");

        let probe = toolchain.probe(&destination).expect("edited probe");
        assert_eq!((probe.metadata.width, probe.metadata.height), (300, 160));
        assert!(probe.has_audio);
        let codec = std::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(destination)
            .output()
            .expect("codec probe");
        assert!(codec.status.success());
        assert_eq!(String::from_utf8_lossy(&codec.stdout).trim(), "h264");
    }
}
