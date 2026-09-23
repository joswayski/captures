use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use captures_video::H264Mp4Writer;
use serde::Deserialize;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use thiserror::Error;

use crate::playback_audio::{AudioProducerControl, PreparedAudioOutput, read_audio_samples};
use crate::{
    EditSpec, ExportEstimate, ExportFormat, ExportProgress, ExportSpec, ExportStage, MediaKind,
    MediaMetadata, QualityPreset, SizeBudgetError, calculate_size_budget, estimate_sample_windows,
    export::{MIN_AUDIO_BITRATE, MIN_VIDEO_BITRATE},
    extrapolate_sampled_size, sampled_export_spec,
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

const PLAYBACK_MAX_WIDTH: u32 = 1_280;
const PLAYBACK_MAX_HEIGHT: u32 = 720;
const PLAYBACK_MAX_FRAMES_PER_SECOND: u16 = 30;
const PLAYBACK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PLAYBACK_MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;

/// One decoded silent-playback frame with a source-relative timestamp.
pub struct MediaPlaybackFrame {
    pub position_ms: u64,
    pixels: Vec<u8>,
}

impl MediaPlaybackFrame {
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

struct BufferedPlaybackFrame {
    frame: MediaPlaybackFrame,
    present_at: Option<Instant>,
}

#[derive(Clone)]
enum PlaybackReaderEnd {
    Eof,
    Error(String),
}

#[derive(Clone)]
enum PlaybackTerminal {
    Eof,
    Cancelled,
    Error(String),
}

#[derive(Default)]
struct PlaybackReaderState {
    frame: Option<BufferedPlaybackFrame>,
    end: Option<PlaybackReaderEnd>,
}

#[derive(Default)]
struct PlaybackShared {
    state: Mutex<PlaybackReaderState>,
    changed: Condvar,
    stop: AtomicBool,
}

/// One persistent FFmpeg raw-RGBA decoder. Frames are paced against a shared
/// monotonic clock and only the latest pending frame is retained.
pub struct MediaPlayback {
    width: u32,
    height: u32,
    frames_per_second: u16,
    start_position_ms: u64,
    cancel: CancelToken,
    shared: Arc<PlaybackShared>,
    child: Option<Child>,
    reader: Option<thread::JoinHandle<()>>,
    stderr_reader: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    audio: Option<MediaPlaybackAudio>,
    terminal: Option<PlaybackTerminal>,
    closed: bool,
}

struct MediaPlaybackAudio {
    output: PreparedAudioOutput,
    child: Option<Child>,
    reader: Option<thread::JoinHandle<()>>,
    stderr_reader: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    closed: bool,
}

impl MediaPlayback {
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub const fn frames_per_second(&self) -> u16 {
        self.frames_per_second
    }

    #[must_use]
    pub const fn start_position_ms(&self) -> u64 {
        self.start_position_ms
    }

    #[must_use]
    pub const fn audio_enabled(&self) -> bool {
        self.audio.is_some()
    }

    /// Return the next clock-paced frame, or `None` after the exclusive trim
    /// end. A lagging consumer skips stale pending frames instead of building a
    /// queue.
    pub fn next_frame(&mut self) -> Result<Option<MediaPlaybackFrame>, MediaToolError> {
        loop {
            if let Some(terminal) = self.terminal.clone() {
                return terminal_result(terminal);
            }
            if self.cancel.is_cancelled() {
                let _ = self.finish(true);
                self.terminal = Some(PlaybackTerminal::Cancelled);
                return Err(MediaToolError::Cancelled);
            }

            let mut state = self.shared.state.lock().map_err(|_| {
                MediaToolError::Process("playback frame buffer was poisoned".to_owned())
            })?;
            if let Some(audio) = self.audio.as_mut() {
                let ready = audio
                    .check_error()
                    .and_then(|()| audio.output.start_if_ready(state.frame.is_some()));
                let ready = match ready {
                    Ok(ready) => ready,
                    Err(error) => {
                        drop(state);
                        let _ = self.finish(true);
                        let error = error.to_string();
                        self.terminal = Some(PlaybackTerminal::Error(error.clone()));
                        return Err(MediaToolError::Process(error));
                    }
                };
                if state.frame.is_some() && !ready {
                    drop(
                        self.shared
                            .changed
                            .wait_timeout(state, PLAYBACK_POLL_INTERVAL)
                            .map_err(|_| {
                                MediaToolError::Process(
                                    "playback frame buffer was poisoned".to_owned(),
                                )
                            })?,
                    );
                    continue;
                }
            }
            if let Some(frame) = state.frame.as_ref() {
                let due = if let Some(audio) = self.audio.as_ref() {
                    audio
                        .output
                        .position_ms(self.start_position_ms)
                        .is_some_and(|position| position >= frame.frame.position_ms)
                } else {
                    frame
                        .present_at
                        .is_some_and(|present_at| Instant::now() >= present_at)
                };
                if due {
                    let frame = state.frame.take().expect("checked pending playback frame");
                    self.shared.changed.notify_all();
                    return Ok(Some(frame.frame));
                }
                let wait = frame
                    .present_at
                    .map_or(PLAYBACK_POLL_INTERVAL, |present_at| {
                        present_at
                            .saturating_duration_since(Instant::now())
                            .min(PLAYBACK_POLL_INTERVAL)
                    });
                drop(self.shared.changed.wait_timeout(state, wait).map_err(|_| {
                    MediaToolError::Process("playback frame buffer was poisoned".to_owned())
                })?);
                continue;
            }

            if let Some(end) = state.end.clone() {
                if self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| !audio.output.drained())
                {
                    drop(
                        self.shared
                            .changed
                            .wait_timeout(state, PLAYBACK_POLL_INTERVAL)
                            .map_err(|_| {
                                MediaToolError::Process(
                                    "playback frame buffer was poisoned".to_owned(),
                                )
                            })?,
                    );
                    continue;
                }
                drop(state);
                return match end {
                    PlaybackReaderEnd::Eof => match self.finish(false) {
                        Ok(()) => {
                            self.terminal = Some(PlaybackTerminal::Eof);
                            Ok(None)
                        }
                        Err(MediaToolError::Cancelled) => {
                            self.terminal = Some(PlaybackTerminal::Cancelled);
                            Err(MediaToolError::Cancelled)
                        }
                        Err(error) => {
                            let error = error.to_string();
                            self.terminal = Some(PlaybackTerminal::Error(error.clone()));
                            Err(MediaToolError::Process(error))
                        }
                    },
                    PlaybackReaderEnd::Error(error) => {
                        let _ = self.finish(true);
                        self.terminal = Some(PlaybackTerminal::Error(error.clone()));
                        Err(MediaToolError::Process(error))
                    }
                };
            }

            drop(
                self.shared
                    .changed
                    .wait_timeout(state, PLAYBACK_POLL_INTERVAL)
                    .map_err(|_| {
                        MediaToolError::Process("playback frame buffer was poisoned".to_owned())
                    })?,
            );
        }
    }

    fn finish(&mut self, kill: bool) -> Result<(), MediaToolError> {
        if self.closed {
            return Ok(());
        }
        self.shared.stop.store(true, Ordering::Release);
        self.shared.changed.notify_all();

        let mut result = Ok(());
        if let Some(audio) = self.audio.as_mut()
            && let Err(error) = audio.finish(kill, &self.cancel)
        {
            result = Err(error);
        }
        let status = if let Some(mut child) = self.child.take() {
            if kill {
                let _ = child.kill();
            }
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) if !kill && self.cancel.is_cancelled() => {
                        let _ = child.kill();
                        result = Err(MediaToolError::Cancelled);
                    }
                    Ok(None) => thread::sleep(PLAYBACK_POLL_INTERVAL),
                    Err(error) => {
                        result = Err(MediaToolError::Io(error));
                        let _ = child.kill();
                        break child.wait().ok();
                    }
                }
            }
        } else {
            None
        };
        if let Some(reader) = self.reader.take()
            && reader.join().is_err()
            && result.is_ok()
        {
            result = Err(MediaToolError::Process(
                "playback frame reader panicked".to_owned(),
            ));
        }
        let stderr = match self.stderr_reader.take() {
            None => Vec::new(),
            Some(reader) => match reader.join() {
                Ok(Ok(stderr)) => stderr,
                Ok(Err(error)) => {
                    if result.is_ok() {
                        result = Err(MediaToolError::Io(error));
                    }
                    Vec::new()
                }
                Err(_) => {
                    if result.is_ok() {
                        result = Err(MediaToolError::Process(
                            "media tool error reader panicked".to_owned(),
                        ));
                    }
                    Vec::new()
                }
            },
        };
        self.closed = true;
        if !kill
            && result.is_ok()
            && let Some(status) = status
        {
            result = complete_child(status, &stderr);
        }
        result
    }
}

impl MediaPlaybackAudio {
    fn check_error(&mut self) -> Result<(), MediaToolError> {
        self.output.check_error()?;
        if let Some(child) = self.child.as_mut()
            && let Some(status) = child.try_wait()?
            && !status.success()
        {
            return Err(MediaToolError::Process(
                "audio media decoder exited before playback completed".to_owned(),
            ));
        }
        Ok(())
    }

    fn finish(&mut self, kill: bool, cancel: &CancelToken) -> Result<(), MediaToolError> {
        if self.closed {
            return Ok(());
        }
        self.output.stop();
        let mut result = Ok(());
        let status = if let Some(mut child) = self.child.take() {
            if kill {
                let _ = child.kill();
            }
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) if !kill && cancel.is_cancelled() => {
                        let _ = child.kill();
                        result = Err(MediaToolError::Cancelled);
                    }
                    Ok(None) => thread::sleep(PLAYBACK_POLL_INTERVAL),
                    Err(error) => {
                        result = Err(MediaToolError::Io(error));
                        let _ = child.kill();
                        break child.wait().ok();
                    }
                }
            }
        } else {
            None
        };
        if let Some(reader) = self.reader.take()
            && reader.join().is_err()
            && result.is_ok()
        {
            result = Err(MediaToolError::Process(
                "audio playback reader panicked".to_owned(),
            ));
        }
        let stderr = match self.stderr_reader.take() {
            None => Vec::new(),
            Some(reader) => match reader.join() {
                Ok(Ok(stderr)) => stderr,
                Ok(Err(error)) => {
                    if result.is_ok() {
                        result = Err(MediaToolError::Io(error));
                    }
                    Vec::new()
                }
                Err(_) => {
                    if result.is_ok() {
                        result = Err(MediaToolError::Process(
                            "audio media tool error reader panicked".to_owned(),
                        ));
                    }
                    Vec::new()
                }
            },
        };
        self.closed = true;
        if !kill
            && result.is_ok()
            && let Some(status) = status
        {
            result = complete_child(status, &stderr);
        }
        result
    }
}

impl Drop for MediaPlaybackAudio {
    fn drop(&mut self) {
        let _ = self.finish(true, &CancelToken::default());
    }
}

impl Drop for MediaPlayback {
    fn drop(&mut self) {
        let _ = self.finish(true);
    }
}

fn terminal_result(
    terminal: PlaybackTerminal,
) -> Result<Option<MediaPlaybackFrame>, MediaToolError> {
    match terminal {
        PlaybackTerminal::Eof => Ok(None),
        PlaybackTerminal::Cancelled => Err(MediaToolError::Cancelled),
        PlaybackTerminal::Error(error) => Err(MediaToolError::Process(error)),
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
    pub system_audio_path: Option<PathBuf>,
    pub system_audio_offset_ms: i64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordingAssemblyKind {
    Video {
        capture_system_audio: bool,
    },
    Gif {
        frames_per_second: u16,
        max_width: u32,
        max_colors: u16,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordingAssemblyOutcome {
    pub probe: ProbeResult,
    pub has_microphone_audio: bool,
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

    pub fn assemble_recording(
        &self,
        segments: &[RecordingSegmentInput],
        destination: &Path,
        session_directory: &Path,
        kind: RecordingAssemblyKind,
        cancel: &CancelToken,
    ) -> Result<RecordingAssemblyOutcome, MediaToolError> {
        let has_microphone_audio = matches!(kind, RecordingAssemblyKind::Video { .. })
            && segments
                .iter()
                .any(|segment| segment.microphone_path.is_some());
        match kind {
            RecordingAssemblyKind::Video {
                capture_system_audio,
            } => self.assemble_recording_segments(
                segments,
                destination,
                RecordingAudioLayout {
                    system_audio: capture_system_audio,
                    microphone_audio: has_microphone_audio,
                },
                cancel,
            )?,
            RecordingAssemblyKind::Gif {
                frames_per_second,
                max_width,
                max_colors,
            } => {
                let paths = segments
                    .iter()
                    .map(|segment| segment.video_path.clone())
                    .collect::<Vec<_>>();
                let master = if paths.len() == 1 {
                    paths[0].clone()
                } else {
                    let master = session_directory.join("master.mp4");
                    if !master.exists() {
                        self.concatenate_segments(&paths, &master, cancel)?;
                    }
                    master
                };
                self.create_gif(
                    &master,
                    destination,
                    frames_per_second,
                    max_width,
                    max_colors,
                    cancel,
                )?;
            }
        }
        Ok(RecordingAssemblyOutcome {
            probe: self.probe(destination)?,
            has_microphone_audio,
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
        let has_external_system_audio = segments
            .iter()
            .any(|segment| segment.system_audio_path.is_some());
        let has_missing_system_audio = if audio.system_audio && !has_external_system_audio {
            segments.iter().try_fold(false, |missing, segment| {
                self.probe(&segment.video_path)
                    .map(|probe| missing || !probe.has_audio)
            })?
        } else {
            false
        };
        if !audio.microphone_audio && !has_external_system_audio && !has_missing_system_audio {
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
        let embedded_system_audio =
            audio.system_audio && self.probe(&segment.video_path)?.has_audio;
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]);
        command.arg(&segment.video_path);
        let mut next_input = 1_usize;
        let system_audio_input = segment.system_audio_path.as_ref().map(|system_audio_path| {
            command.arg("-i").arg(system_audio_path);
            let input = next_input;
            next_input += 1;
            input
        });
        let microphone_input = segment.microphone_path.as_ref().map(|microphone_path| {
            command.arg("-i").arg(microphone_path);
            next_input
        });

        let duration = seconds(segment.duration_ms.max(1));
        let (filters, audio_labels) = recording_segment_audio_graph(
            &duration,
            audio,
            system_audio_input,
            microphone_input,
            embedded_system_audio,
            segment.system_audio_offset_ms,
            segment.microphone_offset_ms,
        );

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
            "-ch_layout:a",
            aac_encoder_channel_layout(false),
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

    /// Extract one decoded frame at `at_ms` from an already-encoded file as a
    /// PNG, without applying any edits.
    pub fn extract_frame(
        &self,
        input: &Path,
        at_ms: u64,
        destination: &Path,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let mut command = Command::new(&self.ffmpeg);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-ss",
                &seconds(at_ms),
                "-i",
            ])
            .arg(input)
            .args(["-frames:v", "1"])
            .arg(destination);
        run_command(&mut command, cancel, "FFmpeg")
    }

    /// Extract one source frame at `at_ms` as a PNG with the export's crop and
    /// scaling applied, so it lines up with a frame from the encoded output.
    pub fn extract_edited_frame(
        &self,
        input: &Path,
        edit: &EditSpec,
        spec: &ExportSpec,
        at_ms: u64,
        destination: &Path,
        cancel: &CancelToken,
    ) -> Result<(), MediaToolError> {
        let probe = self.probe(input)?;
        validate_edit_spec(&probe, edit)?;
        let attempts = export_attempts(&probe, edit, spec)?;
        let attempt = attempts.first().ok_or(MediaToolError::IncompleteMetadata)?;
        let filter = preview_video_filter(&probe, edit, spec, attempt)?;
        let mut command = Command::new(&self.ffmpeg);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-ss",
                &seconds(at_ms),
                "-i",
            ])
            .arg(input)
            .args(["-frames:v", "1", "-vf", &filter])
            .arg(destination);
        run_command(&mut command, cancel, "FFmpeg")
    }

    /// Start one persistent, silent raw-RGBA decoder for the first export
    /// attempt represented by `spec`.
    pub fn playback(
        &self,
        input: &Path,
        probe: &ProbeResult,
        edit: &EditSpec,
        spec: &ExportSpec,
        position_ms: u64,
        cancel: &CancelToken,
    ) -> Result<MediaPlayback, MediaToolError> {
        self.playback_inner(input, probe, edit, spec, position_ms, cancel, false)
    }

    /// Start persistent raw-RGBA and PCM decoders for accepted video and audio
    /// edits. Media without audible accepted audio retains video-only playback
    /// and reports [`MediaPlayback::audio_enabled`] as false.
    pub fn playback_with_audio(
        &self,
        input: &Path,
        probe: &ProbeResult,
        edit: &EditSpec,
        spec: &ExportSpec,
        position_ms: u64,
        cancel: &CancelToken,
    ) -> Result<MediaPlayback, MediaToolError> {
        self.playback_inner(input, probe, edit, spec, position_ms, cancel, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn playback_inner(
        &self,
        input: &Path,
        probe: &ProbeResult,
        edit: &EditSpec,
        spec: &ExportSpec,
        position_ms: u64,
        cancel: &CancelToken,
        request_audio: bool,
    ) -> Result<MediaPlayback, MediaToolError> {
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        validate_edit_spec(probe, edit)?;
        let attempts = export_attempts(probe, edit, spec)?;
        let attempt = attempts.first().ok_or(MediaToolError::IncompleteMetadata)?;
        let mut filter = preview_video_filter(probe, edit, spec, attempt)?;
        let (planned_width, planned_height) = preview_dimensions(probe, edit, spec, attempt);
        let (width, height) = fit_playback_dimensions(planned_width, planned_height);
        if (width, height) != (planned_width, planned_height) {
            filter.push_str(&format!(",scale={width}:{height}:flags=lanczos"));
        }
        let frames_per_second = attempt
            .frames_per_second
            .clamp(1, PLAYBACK_MAX_FRAMES_PER_SECOND);
        filter.push_str(&format!(",fps={frames_per_second}"));

        let duration_ms = probe
            .metadata
            .duration_ms
            .ok_or(MediaToolError::IncompleteMetadata)?;
        let end_position_ms = edit.trim_end_ms.unwrap_or(duration_ms);
        let start_position_ms =
            if position_ms < edit.trim_start_ms || position_ms >= end_position_ms {
                edit.trim_start_ms
            } else {
                position_ms
            };
        let playback_duration_ms = end_position_ms.saturating_sub(start_position_ms);
        let audible = request_audio && accepted_audio_is_audible(edit, attempt);
        let audio_filter = audible.then(|| audio_filter(edit, attempt)).transpose()?;
        let mut audio = if let Some(audio_filter) = audio_filter {
            Some(self.start_playback_audio(
                input,
                &audio_filter,
                start_position_ms,
                playback_duration_ms,
                cancel,
            )?)
        } else {
            None
        };
        let frame_size = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(MediaToolError::IncompleteMetadata)?;

        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error"]);
        if start_position_ms > 0 {
            command.args(["-ss", &seconds(start_position_ms)]);
        }
        command
            .arg("-i")
            .arg(input)
            .args(["-t", &seconds(playback_duration_ms), "-map", "0:v:0"])
            .args(["-vf", &filter, "-an", "-pix_fmt", "rgba"])
            .args(["-f", "rawvideo", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        command.creation_flags(0x0800_0000);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                drop(audio.take());
                return Err(map_spawn_error(error, "FFmpeg"));
            }
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to read decoded playback frames".to_owned(),
            ));
        };
        let Some(mut stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to capture media tool errors".to_owned(),
            ));
        };
        let stderr_reader = thread::spawn(move || {
            read_bounded_diagnostics(&mut stderr, PLAYBACK_MAX_DIAGNOSTIC_BYTES)
        });
        let shared = Arc::new(PlaybackShared::default());
        let reader_shared = shared.clone();
        let reader_cancel = cancel.clone();
        let audio_clock = audio.as_ref().map(|audio| audio.output.producer_control());
        let reader = thread::spawn(move || {
            read_playback_frames(
                stdout,
                frame_size,
                frames_per_second,
                start_position_ms,
                end_position_ms,
                audio_clock,
                &reader_cancel,
                &reader_shared,
            );
        });
        Ok(MediaPlayback {
            width,
            height,
            frames_per_second,
            start_position_ms,
            cancel: cancel.clone(),
            shared,
            child: Some(child),
            reader: Some(reader),
            stderr_reader: Some(stderr_reader),
            audio,
            terminal: None,
            closed: false,
        })
    }

    fn start_playback_audio(
        &self,
        input: &Path,
        accepted_filter: &str,
        start_position_ms: u64,
        playback_duration_ms: u64,
        cancel: &CancelToken,
    ) -> Result<MediaPlaybackAudio, MediaToolError> {
        let mut output = PreparedAudioOutput::prepare(cancel)?;
        let format = output.format();
        let producer = output.take_producer()?;
        let filter = playback_audio_filter(accepted_filter, playback_duration_ms);
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error"]);
        if start_position_ms > 0 {
            command.args(["-ss", &seconds(start_position_ms)]);
        }
        command
            .arg("-i")
            .arg(input)
            .args(["-t", &seconds(playback_duration_ms)])
            .args(["-filter_complex", &filter, "-map", "[playback_audio]"])
            .args(["-vn", "-c:a", "pcm_f32le", "-f", "f32le"])
            .args(["-ar", &format.sample_rate.to_string()])
            .args(["-ac", &format.channels.to_string()])
            .arg("pipe:1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        command.creation_flags(0x0800_0000);
        let mut child = command
            .spawn()
            .map_err(|error| map_spawn_error(error, "FFmpeg"))?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to read decoded playback audio".to_owned(),
            ));
        };
        let Some(mut stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to capture audio media tool errors".to_owned(),
            ));
        };
        let stderr_reader = thread::spawn(move || {
            read_bounded_diagnostics(&mut stderr, PLAYBACK_MAX_DIAGNOSTIC_BYTES)
        });
        let producer_control = output.producer_control();
        let reader_control = producer_control.clone();
        let reader_cancel = cancel.clone();
        let reader = thread::spawn(move || {
            if let Err(error) = read_audio_samples(
                stdout,
                producer,
                &reader_control,
                &reader_cancel,
                format.channels,
            ) {
                reader_control.fail(format!("failed to decode playback audio: {error}"));
            }
        });
        Ok(MediaPlaybackAudio {
            output,
            child: Some(child),
            reader: Some(reader),
            stderr_reader: Some(stderr_reader),
            closed: false,
        })
    }

    /// Estimate one export with the same encode plan as [`Self::export`].
    /// Short ranges are fully encoded; longer ranges encode two sample windows.
    pub fn estimate_export_size(
        &self,
        input: &Path,
        edit: &EditSpec,
        spec: &ExportSpec,
        cancel: &CancelToken,
    ) -> Result<ExportEstimate, MediaToolError> {
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        let probe = self.probe(input)?;
        validate_edit_spec(&probe, edit)?;
        let extension = match spec.format {
            ExportFormat::Mp4 => "mp4",
            ExportFormat::Gif => "gif",
            ExportFormat::WebM => {
                return Err(MediaToolError::Process(
                    "size estimates are not available for WebM".to_owned(),
                ));
            }
        };
        // Build the real attempt plan before any shortcut so invalid size
        // budgets fail the same way as an export instead of returning a size.
        drop(export_attempts(&probe, edit, spec)?);
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        if export_preserves_source_bytes(&probe, edit, spec) {
            return Ok(ExportEstimate {
                size_bytes: probe.metadata.size_bytes,
                exact: true,
            });
        }
        if spec.format == ExportFormat::Mp4
            && spec.quality == QualityPreset::Preserve
            && spec.max_size_bytes.is_none()
            && visual_edit_is_identity(&probe, edit)
        {
            // Only audio is re-encoded. The copied video dominates the file,
            // so shipping behavior uses source size as an approximate result.
            return Ok(ExportEstimate {
                size_bytes: probe.metadata.size_bytes,
                exact: false,
            });
        }

        let source_duration_ms = probe
            .metadata
            .duration_ms
            .ok_or(MediaToolError::IncompleteMetadata)?;
        let trim_end_ms = edit.trim_end_ms.unwrap_or(source_duration_ms);
        let trimmed_ms = trim_end_ms - edit.trim_start_ms;
        let windows = estimate_sample_windows(edit.trim_start_ms, trimmed_ms);
        let exact = windows.len() == 1;
        let scratch = tempfile::Builder::new()
            .prefix("captures-export-estimate-")
            .tempdir()?;
        let mut sampled_bytes = 0_u64;
        let mut sampled_ms = 0_u64;
        for (index, (start_ms, window_ms)) in windows.into_iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(MediaToolError::Cancelled);
            }
            let mut sample_edit = edit.clone();
            sample_edit.trim_start_ms = start_ms;
            sample_edit.trim_end_ms = Some(start_ms + window_ms);
            let sample_spec = sampled_export_spec(spec, window_ms, trimmed_ms);
            let destination = scratch.path().join(format!("sample-{index}.{extension}"));
            let outcome = self.export(
                input,
                &destination,
                &sample_edit,
                &sample_spec,
                cancel,
                |_| {},
            )?;
            sampled_bytes = sampled_bytes.saturating_add(outcome.size_bytes);
            sampled_ms = sampled_ms.saturating_add(window_ms);
        }
        Ok(ExportEstimate {
            size_bytes: extrapolate_sampled_size(sampled_bytes, sampled_ms, trimmed_ms),
            exact,
        })
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
        if mp4_preserves_video_stream(&probe, edit, spec) {
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
                if let Err(error) = commit_temporary(&temporary, destination) {
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
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
        let (width, height) = mp4_attempt_dimensions(attempt);
        let video_filter = video_filter(edit, width, height, attempt.frames_per_second);
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
                QualityPreset::Highest => "95",
                QualityPreset::Preserve | QualityPreset::High => "90",
                QualityPreset::Standard => "70",
                QualityPreset::Small => "50",
                QualityPreset::Tiny => "35",
            };
            command.args(["-q:v", quality]);
        }
        if attempt.has_audio {
            let audio_filter = audio_filter(edit, attempt)?;
            command.args(["-filter_complex", &audio_filter, "-map", "[audio_out]"]);
            apply_aac_encoder(
                &mut command,
                &attempt.audio_bitrate.to_string(),
                edit.audio.mono_output || attempt.force_mono,
            );
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
        let (width, height) = mp4_attempt_dimensions(attempt);
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
        command.args([
            "-filter_complex",
            &audio_filter,
            "-map",
            "1:v:0",
            "-map",
            "[audio_out]",
            "-c:v",
            "copy",
        ]);
        apply_aac_encoder(
            &mut command,
            &attempt.audio_bitrate.to_string(),
            edit.audio.mono_output || attempt.force_mono,
        );
        command.args(["-movflags", "+faststart"]);
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
            command.args(["-filter_complex", &audio_filter, "-map", "[audio_out]"]);
            apply_aac_encoder(
                &mut command,
                &attempt.audio_bitrate.to_string(),
                edit.audio.mono_output || attempt.force_mono,
            );
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

fn accepted_audio_is_audible(edit: &EditSpec, attempt: &VideoAttempt) -> bool {
    attempt.has_audio
        && ((attempt.system_audio
            && !edit.audio.mute_system_audio
            && edit.audio.system_volume.clamp(0.0, 2.0) > 0.0)
            || (attempt.microphone_audio
                && !edit.audio.mute_microphone
                && edit.audio.microphone_volume.clamp(0.0, 2.0) > 0.0))
}

fn playback_audio_filter(accepted_filter: &str, playback_duration_ms: u64) -> String {
    format!(
        "{accepted_filter};[audio_out]apad,atrim=duration={},asetpts=N/SR/TB[playback_audio]",
        seconds(playback_duration_ms)
    )
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
        // Fit the last retry by width, using the same even-dimension rules as
        // the first attempt. Custom output must not lose its aspect ratio.
        let (retry_height, retry_width) = fit_even(source_height, source_width, 320);
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
                (retry_width, retry_height),
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

fn mp4_preserves_video_stream(probe: &ProbeResult, edit: &EditSpec, spec: &ExportSpec) -> bool {
    probe.metadata.kind == MediaKind::Video
        && spec.format == ExportFormat::Mp4
        && spec.quality == QualityPreset::Preserve
        && visual_edit_is_identity(probe, edit)
        && spec
            .max_size_bytes
            .is_none_or(|maximum| probe.metadata.size_bytes <= maximum)
}

fn preview_video_filter(
    probe: &ProbeResult,
    edit: &EditSpec,
    spec: &ExportSpec,
    attempt: &VideoAttempt,
) -> Result<String, MediaToolError> {
    match spec.format {
        ExportFormat::WebM => Err(MediaToolError::Process(
            "WebM export is not available in the bundled media tools".to_owned(),
        )),
        ExportFormat::Gif => Ok(spatial_video_filter(edit, &gif_scale_filter(edit, attempt))),
        ExportFormat::Mp4 => {
            let (width, height) = if mp4_preserves_video_stream(probe, edit, spec) {
                (probe.metadata.width, probe.metadata.height)
            } else {
                mp4_attempt_dimensions(attempt)
            };
            Ok(spatial_video_filter(
                edit,
                &format!("{width}:{height}:flags=lanczos"),
            ))
        }
    }
}

fn preview_dimensions(
    probe: &ProbeResult,
    edit: &EditSpec,
    spec: &ExportSpec,
    attempt: &VideoAttempt,
) -> (u32, u32) {
    match spec.format {
        ExportFormat::Gif if edit.output_width.is_none() => {
            let source_width = edit.crop.map_or(probe.metadata.width, |crop| crop.width);
            let source_height = edit.crop.map_or(probe.metadata.height, |crop| crop.height);
            let width = attempt.width.min(source_width);
            let scaled_height =
                f64::from(source_height) * f64::from(width) / f64::from(source_width);
            (width, nearest_even(scaled_height))
        }
        ExportFormat::Gif => (attempt.width, attempt.height),
        ExportFormat::Mp4 if mp4_preserves_video_stream(probe, edit, spec) => {
            (probe.metadata.width, probe.metadata.height)
        }
        ExportFormat::Mp4 => mp4_attempt_dimensions(attempt),
        ExportFormat::WebM => (attempt.width, attempt.height),
    }
}

fn nearest_even(value: f64) -> u32 {
    ((value / 2.0).round().max(1.0) as u32).saturating_mul(2)
}

fn fit_playback_dimensions(width: u32, height: u32) -> (u32, u32) {
    let scale = (f64::from(PLAYBACK_MAX_WIDTH) / f64::from(width))
        .min(f64::from(PLAYBACK_MAX_HEIGHT) / f64::from(height))
        .min(1.0);
    if scale >= 1.0 {
        return (width, height);
    }
    (
        nearest_even(f64::from(width) * scale),
        nearest_even(f64::from(height) * scale),
    )
}

fn spatial_video_filter(edit: &EditSpec, scale: &str) -> String {
    let mut filters = Vec::new();
    if let Some(crop) = edit.crop {
        filters.push(format!(
            "crop={}:{}:{}:{}",
            crop.width, crop.height, crop.x, crop.y
        ));
    }
    filters.push(format!("scale={scale}"));
    filters.join(",")
}

/// Validate one edit against already-probed source metadata. Callers that retain
/// a trusted probe can use this without duplicating trim/crop/output rules.
pub fn validate_edit_spec(probe: &ProbeResult, edit: &EditSpec) -> Result<(), MediaToolError> {
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

/// True when [`MediaToolchain::export`] would copy the source file unchanged,
/// so the saved file's size equals the source size exactly.
pub fn export_preserves_source_bytes(
    probe: &ProbeResult,
    edit: &EditSpec,
    spec: &ExportSpec,
) -> bool {
    mp4_preserves_video_stream(probe, edit, spec) && audio_edit_is_identity(edit)
}

/// True when the edit leaves the video stream untouched (no trim, crop, or
/// resize), so a Preserve-quality MP4 export copies it instead of re-encoding.
pub fn visual_edit_is_identity(probe: &ProbeResult, edit: &EditSpec) -> bool {
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
fn mp4_attempt_dimensions(attempt: &VideoAttempt) -> (u32, u32) {
    fit_openh264_dimensions(attempt.width, attempt.height)
}

#[cfg(target_os = "macos")]
const fn mp4_attempt_dimensions(attempt: &VideoAttempt) -> (u32, u32) {
    (attempt.width, attempt.height)
}

/// Windows/Linux capture masters use 12% bits-per-pixel (`recording_bitrate`
/// in captures-recording-xcap). Compress re-encodes must stay at or below
/// that so a quality preset cannot enlarge the file.
#[cfg(any(target_os = "windows", target_os = "linux"))]
const CAPTURE_MASTER_BITS_PER_PIXEL_PERCENT: u64 = 12;

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn openh264_bitrate(attempt: &VideoAttempt, spec: &ExportSpec) -> u32 {
    let bits_per_pixel_percent = match spec.quality {
        QualityPreset::Preserve => CAPTURE_MASTER_BITS_PER_PIXEL_PERCENT,
        // Modest cut under the capture master. Must stay below 12% or Highest
        // is a larger, lossier file than the recording it came from.
        QualityPreset::Highest => 10,
        QualityPreset::High => 8,
        QualityPreset::Standard => 6,
        QualityPreset::Small => 5,
        QualityPreset::Tiny => 3,
    };
    let estimated = u64::from(attempt.width)
        .saturating_mul(u64::from(attempt.height))
        .saturating_mul(u64::from(attempt.frames_per_second))
        .saturating_mul(bits_per_pixel_percent)
        / 100;
    let bitrate = attempt.video_bitrate.unwrap_or(estimated);
    u32::try_from(bitrate.clamp(250_000, 50_000_000)).unwrap_or(50_000_000)
}

fn apply_aac_encoder(command: &mut Command, bitrate: &str, mono: bool) {
    command.args([
        "-c:a",
        "aac",
        "-b:a",
        bitrate,
        "-ch_layout:a",
        aac_encoder_channel_layout(mono),
    ]);
}

fn aac_encoder_channel_layout(mono: bool) -> &'static str {
    if mono { "mono" } else { "stereo" }
}

/// Native AAC rejects a 1-channel Front Left layout, which is how FFmpeg
/// labels many microphone captures. Relabel those sources as centered mono
/// before expanding to stereo so the signal is in both ears; going straight
/// to stereo can keep Front Left identity (left channel only). Do not run
/// this on stereo system audio, which would collapse left/right.
fn aac_centered_stereo_layout_filter() -> &'static str {
    ",aformat=sample_fmts=fltp:channel_layouts=mono,aformat=channel_layouts=stereo"
}

fn aac_stereo_layout_filter() -> &'static str {
    ",aformat=sample_fmts=fltp:channel_layouts=stereo"
}

fn aac_mono_layout_filter() -> &'static str {
    ",aformat=sample_fmts=fltp:channel_layouts=mono"
}

fn aac_output_layout_filter(mono: bool) -> &'static str {
    if mono {
        aac_mono_layout_filter()
    } else {
        aac_stereo_layout_filter()
    }
}

fn recording_segment_audio_graph(
    duration: &str,
    audio: RecordingAudioLayout,
    system_audio_input: Option<usize>,
    microphone_input: Option<usize>,
    embedded_system_audio: bool,
    system_audio_offset_ms: i64,
    microphone_offset_ms: i64,
) -> (Vec<String>, Vec<(&'static str, &'static str)>) {
    let mut filters = Vec::new();
    let mut audio_labels = Vec::new();
    let independent_tracks = audio.system_audio && audio.microphone_audio;
    let system_layout = aac_stereo_layout_filter();
    let microphone_layout = aac_centered_stereo_layout_filter();
    let system_output = if independent_tracks {
        "[system-source]"
    } else {
        "[system]"
    };
    let microphone_output = if independent_tracks {
        "[microphone-source]"
    } else {
        "[microphone]"
    };
    if audio.system_audio {
        if let Some(input) = system_audio_input {
            let delay = system_audio_offset_ms.max(0);
            filters.push(format!(
                "[{input}:a:0]adelay={delay}:all=1,aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration}{system_layout}{system_output}"
            ));
        } else if embedded_system_audio {
            filters.push(format!(
                "[0:a:0]aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration}{system_layout}{system_output}"
            ));
        } else {
            filters.push(format!(
                "anullsrc=r=48000:cl=stereo,atrim=duration={duration}{system_layout}{system_output}"
            ));
        }
        audio_labels.push(("[system]", "System Audio"));
    }
    if audio.microphone_audio {
        if let Some(input) = microphone_input {
            let delay = microphone_offset_ms.max(0);
            filters.push(format!(
                "[{input}:a:0]adelay={delay}:all=1,aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration}{microphone_layout}{microphone_output}"
            ));
        } else {
            filters.push(format!(
                "anullsrc=r=48000:cl=stereo,atrim=duration={duration}{microphone_layout}{microphone_output}"
            ));
        }
        audio_labels.push(("[microphone]", "Microphone"));
    }

    if independent_tracks {
        filters.push("[system-source]asplit=2[system-playback][system]".to_owned());
        filters.push("[microphone-source]asplit=2[microphone-playback][microphone]".to_owned());
        filters.push(
            "[system-playback][microphone-playback]amix=inputs=2:normalize=0:dropout_transition=0[playback]".to_owned(),
        );
        audio_labels.insert(0, ("[playback]", "System Audio + Microphone"));
    }

    (filters, audio_labels)
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
            "[0:a:{index}]volume={:.3},aresample=48000:async=1:first_pts=0{}[microphone_edit]",
            audio.microphone_volume.clamp(0.0, 2.0),
            aac_centered_stereo_layout_filter()
        ));
        labels.push("[microphone_edit]");
    }
    match labels.as_slice() {
        [] => Err(MediaToolError::IncompleteMetadata),
        [label] => {
            filters.push(format!(
                "{label}anull{}[audio_out]",
                aac_output_layout_filter(mono_output)
            ));
            Ok(filters.join(";"))
        }
        _ => {
            filters.push(format!(
                "{}amix=inputs={}:normalize=0{}[audio_out]",
                labels.join(""),
                labels.len(),
                aac_output_layout_filter(mono_output)
            ));
            Ok(filters.join(";"))
        }
    }
}

fn single_audio_filter(index: usize, volume: f32, mono: bool) -> String {
    format!(
        "[0:a:{index}]volume={volume:.3},aresample=48000:async=1:first_pts=0{}[audio_out]",
        aac_output_layout_filter(mono)
    )
}

// Full-frame statistics keep scene colors represented in the global palette.
// Diff statistics can spend the palette on compression noise in large frames
// and omit a later scene color entirely.
fn gif_export_filter(edit: &EditSpec, attempt: &VideoAttempt) -> String {
    let crop = edit.crop.map_or_else(String::new, |crop| {
        format!("crop={}:{}:{}:{},", crop.width, crop.height, crop.x, crop.y)
    });
    let scale = gif_scale_filter(edit, attempt);
    format!(
        "{crop}fps={},scale={scale},split[s0][s1];[s0]palettegen=max_colors={}:stats_mode=full[p];[s1][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle",
        attempt.frames_per_second, attempt.gif_colors
    )
}

fn gif_scale_filter(edit: &EditSpec, attempt: &VideoAttempt) -> String {
    if edit.output_width.zip(edit.output_height).is_some() {
        format!(
            "{}:{}:flags=lanczos,setsar=1",
            attempt.width, attempt.height
        )
    } else {
        format!("'min({},iw)':-2:flags=lanczos", attempt.width)
    }
}

fn gif_filter(frames_per_second: u16, max_width: u32, max_colors: u16) -> String {
    format!(
        "fps={frames_per_second},scale='min({max_width},iw)':-2:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors={max_colors}:stats_mode=full[p];[s1][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle"
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
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to capture media tool errors".to_owned(),
            ));
        }
    };
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

#[allow(clippy::too_many_arguments)]
fn read_playback_frames(
    mut stdout: impl Read,
    frame_size: usize,
    frames_per_second: u16,
    start_position_ms: u64,
    end_position_ms: u64,
    audio_clock: Option<AudioProducerControl>,
    cancel: &CancelToken,
    shared: &PlaybackShared,
) {
    let mut frame_index = 0_u64;
    let mut clock = None;
    loop {
        if cancel.is_cancelled() || shared.stop.load(Ordering::Acquire) {
            return;
        }
        let mut pixels = vec![0_u8; frame_size];
        match read_complete_frame(&mut stdout, &mut pixels) {
            Ok(true) => {}
            Ok(false) => {
                set_playback_end(shared, PlaybackReaderEnd::Eof);
                return;
            }
            Err(error) => {
                set_playback_end(
                    shared,
                    PlaybackReaderEnd::Error(format!(
                        "failed to read a complete decoded playback frame: {error}"
                    )),
                );
                return;
            }
        }
        let elapsed_ms = frame_index.saturating_mul(1_000) / u64::from(frames_per_second);
        let position_ms = start_position_ms.saturating_add(elapsed_ms);
        frame_index = frame_index.saturating_add(1);
        if position_ms >= end_position_ms {
            continue;
        }
        let present_at = audio_clock.is_none().then(|| {
            let clock = *clock.get_or_insert_with(Instant::now);
            clock
                .checked_add(Duration::from_millis(elapsed_ms))
                .unwrap_or(clock)
        });
        let mut pending = Some(BufferedPlaybackFrame {
            frame: MediaPlaybackFrame {
                position_ms,
                pixels,
            },
            present_at,
        });
        let Ok(mut state) = shared.state.lock() else {
            return;
        };
        while state.frame.is_some() {
            if cancel.is_cancelled() || shared.stop.load(Ordering::Acquire) {
                return;
            }
            let stale = if let Some(audio_clock) = audio_clock.as_ref() {
                audio_clock
                    .position_ms(start_position_ms)
                    .is_some_and(|position| position >= position_ms)
            } else {
                present_at.is_some_and(|present_at| Instant::now() >= present_at)
            };
            if stale {
                state.frame = pending.take();
                shared.changed.notify_all();
                break;
            }
            let wait = present_at.map_or(PLAYBACK_POLL_INTERVAL, |present_at| {
                present_at
                    .saturating_duration_since(Instant::now())
                    .min(PLAYBACK_POLL_INTERVAL)
            });
            let Ok((next, _)) = shared.changed.wait_timeout(state, wait) else {
                return;
            };
            state = next;
        }
        if let Some(frame) = pending.take() {
            state.frame = Some(frame);
            shared.changed.notify_all();
        }
        drop(state);
    }
}

fn set_playback_end(shared: &PlaybackShared, end: PlaybackReaderEnd) {
    if let Ok(mut state) = shared.state.lock() {
        state.end = Some(end);
        shared.changed.notify_all();
    }
}

fn read_bounded_diagnostics(reader: &mut impl Read, maximum: usize) -> io::Result<Vec<u8>> {
    let mut retained = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            return Ok(retained);
        }
        retained.extend_from_slice(&chunk[..count]);
        if retained.len() > maximum {
            retained.drain(..retained.len() - maximum);
        }
    }
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
    let result = commit_temporary(&temporary, destination);
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn commit_temporary(temporary: &Path, destination: &Path) -> Result<(), MediaToolError> {
    let file = fs::File::open(temporary)?;
    let path = tempfile::TempPath::try_from_path(temporary)?;
    tempfile::NamedTempFile::from_parts(file, path)
        .persist_noclobber(destination)
        .map_err(|error| error.error)?;
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
    #[cfg(unix)]
    use super::MediaPlayback;
    #[cfg(target_os = "macos")]
    use super::TimelineSpriteSpec;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use super::{
        CAPTURE_MASTER_BITS_PER_PIXEL_PERCENT, MediaToolError, RecordingAssemblyKind,
        RecordingSegmentInput, openh264_bitrate,
    };
    use super::{
        CancelToken, MediaToolchain, RecordingAudioLayout, VideoAttempt,
        aac_centered_stereo_layout_filter, aac_output_layout_filter, accepted_audio_is_audible,
        audio_edit_is_identity, audio_filter, commit_temporary, escape_concat_path,
        export_attempts, export_preserves_source_bytes, fit_even, fit_playback_dimensions,
        gif_export_filter, gif_filter, playback_audio_filter, preview_dimensions,
        preview_video_filter, read_bounded_diagnostics, read_complete_frame,
        recording_segment_audio_graph, seconds, validate_edit_spec, visual_edit_is_identity,
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
    fn playback_geometry_preserves_preview_rounding_and_bounds() {
        let portrait = ProbeResult {
            metadata: MediaMetadata {
                width: 640,
                height: 1_440,
                ..probe().metadata
            },
            ..probe()
        };
        let gif = ExportSpec {
            format: ExportFormat::Gif,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };
        let attempts = export_attempts(&portrait, &EditSpec::default(), &gif).unwrap();
        assert_eq!(
            preview_dimensions(&portrait, &EditSpec::default(), &gif, &attempts[0]),
            (640, 1_440)
        );
        assert_eq!(fit_playback_dimensions(640, 1_440), (320, 720));

        let odd = ProbeResult {
            metadata: MediaMetadata {
                width: 160,
                height: 91,
                ..probe().metadata
            },
            ..probe()
        };
        let attempts = export_attempts(&odd, &EditSpec::default(), &gif).unwrap();
        assert_eq!(
            preview_dimensions(&odd, &EditSpec::default(), &gif, &attempts[0]),
            (160, 92),
            "automatic GIF playback keeps FFmpeg -2 rounding"
        );
        assert_eq!(fit_playback_dimensions(4_000, 600), (1_280, 192));
    }

    #[test]
    fn audible_playback_requires_a_positive_accepted_track() {
        let mut edit = EditSpec {
            audio: AudioEdit {
                source_has_system_audio: true,
                source_has_microphone_audio: true,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };
        let attempt = export_attempts(
            &probe(),
            &edit,
            &ExportSpec {
                format: ExportFormat::Mp4,
                quality: QualityPreset::Preserve,
                max_size_bytes: None,
                frames_per_second: None,
                gif_max_colors: None,
            },
        )
        .unwrap()
        .remove(0);
        assert!(accepted_audio_is_audible(&edit, &attempt));
        edit.audio.system_volume = 0.0;
        edit.audio.microphone_volume = -1.0;
        assert!(!accepted_audio_is_audible(&edit, &attempt));
        edit.audio.microphone_volume = 0.25;
        assert!(accepted_audio_is_audible(&edit, &attempt));
        edit.audio.mute_microphone = true;
        assert!(!accepted_audio_is_audible(&edit, &attempt));

        let mut gif_attempt = attempt;
        gif_attempt.has_audio = false;
        assert!(!accepted_audio_is_audible(&edit, &gif_attempt));
    }

    #[test]
    fn playback_reads_complete_frames_and_bounds_diagnostics() {
        let mut partial = &b"12345678"[..];
        let mut frame = [0_u8; 16];
        assert_eq!(
            read_complete_frame(&mut partial, &mut frame)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );

        let diagnostics = vec![b'x'; 80 * 1024];
        let retained = read_bounded_diagnostics(&mut diagnostics.as_slice(), 64 * 1024).unwrap();
        assert_eq!(retained.len(), 64 * 1024);
    }

    #[cfg(unix)]
    fn scripted_playback(script: &str) -> (tempfile::TempDir, MediaPlayback, CancelToken) {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("ffmpeg-test");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let input = directory.path().join("source.mp4");
        std::fs::write(&input, b"immutable").unwrap();
        let probe = ProbeResult {
            metadata: MediaMetadata {
                kind: MediaKind::Video,
                mime_type: "video/mp4".into(),
                width: 2,
                height: 2,
                duration_ms: Some(100),
                size_bytes: 9,
            },
            has_audio: false,
            audio_stream_count: 0,
        };
        let edit = EditSpec {
            trim_end_ms: Some(100),
            ..EditSpec::default()
        };
        let spec = ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: Some(30),
            gif_max_colors: None,
        };
        let cancel = CancelToken::default();
        let playback = MediaToolchain::new(executable, "unused".into())
            .playback(&input, &probe, &edit, &spec, 0, &cancel)
            .unwrap();
        (directory, playback, cancel)
    }

    #[cfg(unix)]
    #[test]
    fn playback_terminal_results_repeat_and_clock_starts_with_first_frame() {
        let started = std::time::Instant::now();
        // One frame isolates startup-clock behavior from intentional latest-frame
        // coalescing when the consumer is descheduled on a busy runner.
        let (_directory, mut playback, cancel) =
            scripted_playback("sleep 0.2; printf '0000000000000000'");
        let first = playback.next_frame().unwrap().unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(150));
        assert_eq!(
            first.position_ms, 0,
            "slow startup must not skip the first frame"
        );
        assert!(playback.next_frame().unwrap().is_none());
        assert!(playback.next_frame().unwrap().is_none());
        assert!(!cancel.is_cancelled());
        assert!(playback.child.is_none());
        assert!(playback.reader.is_none());
        assert!(playback.stderr_reader.is_none());

        let (_directory, mut playback, cancel) =
            scripted_playback("printf '000000000000000000000000000000000000000000000000'");
        // Wait for decoding, not an assumed scheduling interval. A consumer that
        // has not polled must receive the final 66 ms frame rather than a queue.
        let state = playback.shared.state.lock().unwrap();
        let (state, timeout) = playback
            .shared
            .changed
            .wait_timeout_while(state, std::time::Duration::from_secs(5), |state| {
                state.end.is_none()
            })
            .unwrap();
        assert!(!timeout.timed_out(), "scripted decoder must reach EOF");
        drop(state);
        assert_eq!(playback.next_frame().unwrap().unwrap().position_ms, 66);
        assert!(playback.next_frame().unwrap().is_none());
        assert!(playback.next_frame().unwrap().is_none());
        assert!(!cancel.is_cancelled());
        assert!(playback.child.is_none());
        assert!(playback.reader.is_none());
        assert!(playback.stderr_reader.is_none());

        let (_directory, mut playback, _cancel) = scripted_playback("printf '12345678'");
        let first = match playback.next_frame() {
            Ok(_) => panic!("partial frame must fail"),
            Err(error) => error.to_string(),
        };
        let second = match playback.next_frame() {
            Ok(_) => panic!("terminal error must repeat"),
            Err(error) => error.to_string(),
        };
        assert_eq!(first, second, "terminal decoder errors are deterministic");
        assert!(first.contains("partial raw video frame"));

        let (_directory, mut playback, _cancel) = scripted_playback(
            "i=0; while [ $i -lt 5000 ]; do echo 'discard-old-diagnostic-line' >&2; i=$((i+1)); done; echo 'retained-tail-marker' >&2; exit 7",
        );
        let error = match playback.next_frame() {
            Ok(_) => panic!("nonzero decoder must fail"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("retained-tail-marker"));
        assert!(error.len() <= super::PLAYBACK_MAX_DIAGNOSTIC_BYTES + 64);
    }

    #[cfg(unix)]
    #[test]
    fn playback_cancel_interrupts_open_and_closed_stdout_stalls() {
        for script in ["exec sleep 30", "exec 1>&-; exec sleep 30"] {
            let (_directory, mut playback, cancel) = scripted_playback(script);
            let cancellation = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                cancellation.cancel();
            });
            let started = std::time::Instant::now();
            assert!(matches!(
                playback.next_frame(),
                Err(super::MediaToolError::Cancelled)
            ));
            assert!(started.elapsed() < std::time::Duration::from_millis(500));
            assert!(matches!(
                playback.next_frame(),
                Err(super::MediaToolError::Cancelled)
            ));
            assert!(playback.child.is_none());
            assert!(playback.reader.is_none());
            assert!(playback.stderr_reader.is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn playback_pre_cancel_refuses_to_spawn() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("ffmpeg-test");
        let marker = directory.path().join("spawned");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\ntouch '{}'\nexec sleep 30\n", marker.display()),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let probe = ProbeResult {
            metadata: MediaMetadata {
                kind: MediaKind::Video,
                mime_type: "video/mp4".into(),
                width: 2,
                height: 2,
                duration_ms: Some(100),
                size_bytes: 1,
            },
            has_audio: false,
            audio_stream_count: 0,
        };
        let cancel = CancelToken::default();
        cancel.cancel();
        let tools = MediaToolchain::new(executable, "unused".into());
        let result = tools.playback(
            &directory.path().join("source.mp4"),
            &probe,
            &EditSpec {
                trim_end_ms: Some(100),
                ..EditSpec::default()
            },
            &ExportSpec {
                format: ExportFormat::Mp4,
                quality: QualityPreset::Preserve,
                max_size_bytes: None,
                frames_per_second: None,
                gif_max_colors: None,
            },
            0,
            &cancel,
        );
        assert!(matches!(result, Err(super::MediaToolError::Cancelled)));
        let mut audible_probe = probe;
        audible_probe.has_audio = true;
        audible_probe.audio_stream_count = 1;
        let result = tools.playback_with_audio(
            &directory.path().join("source.mp4"),
            &audible_probe,
            &EditSpec {
                trim_end_ms: Some(100),
                audio: AudioEdit {
                    source_has_system_audio: true,
                    ..AudioEdit::default()
                },
                ..EditSpec::default()
            },
            &ExportSpec {
                format: ExportFormat::Mp4,
                quality: QualityPreset::Preserve,
                max_size_bytes: None,
                frames_per_second: None,
                gif_max_colors: None,
            },
            0,
            &cancel,
        );
        assert!(matches!(result, Err(super::MediaToolError::Cancelled)));
        assert!(!marker.exists());
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

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn bitrate_probe_attempt() -> VideoAttempt {
        VideoAttempt {
            width: 1_920,
            height: 1_080,
            frames_per_second: 30,
            video_bitrate: None,
            audio_bitrate: 128_000,
            has_audio: false,
            system_audio: false,
            microphone_audio: false,
            audio_stream_count: 0,
            force_mono: false,
            gif_colors: 256,
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn bitrate_spec(quality: QualityPreset) -> ExportSpec {
        ExportSpec {
            format: ExportFormat::Mp4,
            quality,
            max_size_bytes: None,
            frames_per_second: Some(30),
            gif_max_colors: None,
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn compress_bitrates_stay_below_the_capture_master() {
        let attempt = bitrate_probe_attempt();
        let master = u64::from(attempt.width)
            * u64::from(attempt.height)
            * u64::from(attempt.frames_per_second)
            * CAPTURE_MASTER_BITS_PER_PIXEL_PERCENT
            / 100;
        let highest = u64::from(openh264_bitrate(
            &attempt,
            &bitrate_spec(QualityPreset::Highest),
        ));
        let high = u64::from(openh264_bitrate(
            &attempt,
            &bitrate_spec(QualityPreset::High),
        ));
        let standard = u64::from(openh264_bitrate(
            &attempt,
            &bitrate_spec(QualityPreset::Standard),
        ));
        assert!(
            highest <= master,
            "Highest ({highest}) must not exceed the 12% capture master ({master})"
        );
        assert!(
            high < highest,
            "High ({high}) should be a stronger compress than Highest ({highest})"
        );
        assert!(
            standard < high,
            "Balanced ({standard}) should be stronger than High ({high})"
        );
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
        let gif = gif_filter(15, 800, 256);
        assert!(gif.contains("palettegen=max_colors=256:stats_mode=full"));
    }

    #[test]
    fn publication_refuses_a_destination_created_after_encoding_and_cleans_up() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let temporary = directory.path().join(".captures-encoded.mp4");
        let destination = directory.path().join("saved.mp4");
        std::fs::write(&temporary, b"new export").expect("temporary export");
        // Simulate another writer publishing after the caller's initial
        // destination validation but before the encoded file is committed.
        std::fs::write(&destination, b"other writer").expect("racing destination");

        assert!(commit_temporary(&temporary, &destination).is_err());
        assert_eq!(
            std::fs::read(&destination).expect("destination remains"),
            b"other writer"
        );
        assert!(!temporary.exists(), "failed encoded temporary is removed");
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
        assert!(filter.contains(
            "[0:a:2]volume=1.500,aresample=48000:async=1:first_pts=0,aformat=sample_fmts=fltp:channel_layouts=mono,aformat=channel_layouts=stereo[microphone_edit]"
        ));
        assert!(
            !filter.contains("[0:a:1]volume=0.500,aresample=48000:async=1:first_pts=0,aformat")
        );
        assert!(filter.contains("channel_layouts=stereo"));
    }

    #[test]
    fn recording_and_export_audio_graphs_rematrix_front_left_sources() {
        assert_eq!(
            aac_output_layout_filter(false),
            ",aformat=sample_fmts=fltp:channel_layouts=stereo"
        );
        assert_eq!(
            aac_output_layout_filter(true),
            ",aformat=sample_fmts=fltp:channel_layouts=mono"
        );
        assert_eq!(
            aac_centered_stereo_layout_filter(),
            ",aformat=sample_fmts=fltp:channel_layouts=mono,aformat=channel_layouts=stereo"
        );

        let (filters, labels) = recording_segment_audio_graph(
            "1.000",
            RecordingAudioLayout {
                system_audio: false,
                microphone_audio: true,
            },
            None,
            Some(1),
            false,
            0,
            12,
        );
        assert_eq!(labels, vec![("[microphone]", "Microphone")]);
        assert_eq!(filters.len(), 1);
        assert!(
            filters[0].contains(
                "aformat=sample_fmts=fltp:channel_layouts=mono,aformat=channel_layouts=stereo[microphone]"
            ),
            "microphone graph should center Front Left before stereo AAC: {}",
            filters[0]
        );

        let (filters, labels) = recording_segment_audio_graph(
            "2.500",
            RecordingAudioLayout {
                system_audio: true,
                microphone_audio: true,
            },
            Some(1),
            Some(2),
            false,
            0,
            0,
        );
        assert_eq!(
            labels,
            vec![
                ("[playback]", "System Audio + Microphone"),
                ("[system]", "System Audio"),
                ("[microphone]", "Microphone"),
            ]
        );
        let system = filters
            .iter()
            .find(|filter| filter.contains("[system-source]"))
            .expect("system source");
        assert!(system.contains("aformat=sample_fmts=fltp:channel_layouts=stereo[system-source]"));
        assert!(
            !system.contains("channel_layouts=mono"),
            "system audio must stay stereo: {system}"
        );
        assert!(filters.iter().any(|filter| {
            filter.contains(
                "aformat=sample_fmts=fltp:channel_layouts=mono,aformat=channel_layouts=stereo[microphone-source]",
            )
        }));
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
    fn gif_custom_dimensions_match_preview_and_keep_the_ratio_in_size_retries() {
        for (width, height, first, last) in [
            (81, 61, "scale=80:60:", "scale=80:60:"),
            (801, 601, "scale=800:600:", "scale=320:240:"),
        ] {
            let edit = EditSpec {
                crop: Some(CropRect {
                    x: 10,
                    y: 6,
                    width: 160,
                    height: 90,
                }),
                output_width: Some(width),
                output_height: Some(height),
                ..EditSpec::default()
            };
            let attempts = export_attempts(
                &probe(),
                &edit,
                &ExportSpec {
                    format: ExportFormat::Gif,
                    quality: QualityPreset::Preserve,
                    max_size_bytes: Some(100_000),
                    frames_per_second: None,
                    gif_max_colors: None,
                },
            )
            .unwrap();
            for (attempt, expected) in [(&attempts[0], first), (&attempts[3], last)] {
                let filter = gif_export_filter(&edit, attempt);
                assert!(filter.starts_with("crop=160:90:10:6,"), "{filter}");
                assert!(filter.contains(expected), "{filter}");
                assert!(filter.contains("setsar=1"), "{filter}");
            }
            let uncapped = gif_export_filter(&EditSpec::default(), &attempts[0]);
            assert!(uncapped.contains("':-2:flags=lanczos"), "{uncapped}");
        }
    }

    #[test]
    fn preview_filter_uses_the_first_real_format_plan() {
        let mut oversized = probe();
        oversized.metadata.width = 4_000;
        oversized.metadata.height = 2_200;
        let preserve = ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };
        let identity_attempts =
            export_attempts(&oversized, &EditSpec::default(), &preserve).unwrap();
        let identity = preview_video_filter(
            &oversized,
            &EditSpec::default(),
            &preserve,
            &identity_attempts[0],
        )
        .unwrap();
        assert_eq!(identity, "scale=4000:2200:flags=lanczos");

        let edit = EditSpec {
            output_width: Some(4_001),
            output_height: Some(601),
            ..EditSpec::default()
        };
        let standard = ExportSpec {
            quality: QualityPreset::Standard,
            ..preserve.clone()
        };
        let mp4_attempts = export_attempts(&oversized, &edit, &standard).unwrap();
        let mp4 = preview_video_filter(&oversized, &edit, &standard, &mp4_attempts[0]).unwrap();
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        assert_eq!(mp4, "scale=3840:576:flags=lanczos");
        #[cfg(target_os = "macos")]
        assert_eq!(mp4, "scale=4000:600:flags=lanczos");

        let gif = ExportSpec {
            format: ExportFormat::Gif,
            ..preserve.clone()
        };
        let gif_attempts = export_attempts(&oversized, &edit, &gif).unwrap();
        let gif_preview = preview_video_filter(&oversized, &edit, &gif, &gif_attempts[0]).unwrap();
        assert_eq!(gif_preview, "scale=4000:600:flags=lanczos,setsar=1");
        assert!(gif_export_filter(&edit, &gif_attempts[0]).contains(&gif_preview));

        let automatic_gif_edit = EditSpec {
            crop: Some(CropRect {
                x: 4,
                y: 6,
                width: 160,
                height: 91,
            }),
            ..EditSpec::default()
        };
        let automatic_attempts = export_attempts(&oversized, &automatic_gif_edit, &gif).unwrap();
        let automatic_preview = preview_video_filter(
            &oversized,
            &automatic_gif_edit,
            &gif,
            &automatic_attempts[0],
        )
        .unwrap();
        assert_eq!(
            automatic_preview,
            "crop=160:91:4:6,scale='min(160,iw)':-2:flags=lanczos"
        );
        assert!(
            gif_export_filter(&automatic_gif_edit, &automatic_attempts[0])
                .contains("scale='min(160,iw)':-2:flags=lanczos")
        );

        let webm = ExportSpec {
            format: ExportFormat::WebM,
            ..preserve
        };
        let webm_attempts = export_attempts(&oversized, &edit, &webm).unwrap();
        assert!(preview_video_filter(&oversized, &edit, &webm, &webm_attempts[0]).is_err());
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
        assert!(!filter.contains("channel_layouts=stereo"));
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
        let preserve = ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };
        assert!(export_preserves_source_bytes(
            &probe(),
            &untouched,
            &preserve
        ));
        let mut gif_probe = probe();
        gif_probe.metadata.kind = MediaKind::Gif;
        gif_probe.metadata.mime_type = "image/gif".into();
        assert!(!export_preserves_source_bytes(
            &gif_probe, &untouched, &preserve
        ));

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
    fn media_test_guard() -> Option<std::sync::MutexGuard<'static, ()>> {
        let (toolchain, _) = bundled_toolchain();
        if !toolchain.ffmpeg.is_file() || !toolchain.ffprobe.is_file() {
            eprintln!("macOS media sidecars are not prepared in this checkout");
            return None;
        }
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        Some(
            LOCK.get_or_init(|| std::sync::Mutex::new(()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
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
        let Some(_guard) = media_test_guard() else {
            return;
        };
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
        let Some(_guard) = media_test_guard() else {
            return;
        };
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
        let Some(_guard) = media_test_guard() else {
            return;
        };
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
        let Some(_guard) = media_test_guard() else {
            return;
        };
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
        let Some(_guard) = media_test_guard() else {
            return;
        };
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
    fn cross_platform_toolchain() -> Option<(MediaToolchain, std::path::PathBuf, std::path::PathBuf)>
    {
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
            return None;
        }
        Some((
            MediaToolchain::new(ffmpeg.clone(), ffprobe.clone()),
            ffmpeg,
            ffprobe,
        ))
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn create_cross_platform_segment(ffmpeg: &std::path::Path, path: &std::path::Path) {
        let status = std::process::Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=10",
                "-t",
                "0.5",
                "-c:v",
                "mpeg4",
                "-q:v",
                "2",
                "-an",
            ])
            .arg(path)
            .status()
            .expect("bundled FFmpeg starts");
        assert!(status.success(), "test recording segment generated");
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn playback_pcm_uses_accepted_gain_mono_trim_and_frequency() {
        let Some((toolchain, ffmpeg, _ffprobe)) = cross_platform_toolchain() else {
            return;
        };
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("audio-source.mp4");
        let status = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=black:size=32x24:rate=10:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1",
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source)
            .status()
            .expect("FFmpeg starts");
        assert!(status.success());

        let probe = toolchain.probe(&source).expect("probe audio source");
        let edit = EditSpec {
            trim_start_ms: 250,
            trim_end_ms: Some(750),
            audio: AudioEdit {
                source_has_system_audio: true,
                system_volume: 0.5,
                mono_output: true,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };
        let spec = ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };
        let attempt = export_attempts(&probe, &edit, &spec).unwrap().remove(0);
        let accepted = audio_filter(&edit, &attempt).expect("accepted audio filter");
        let filter = playback_audio_filter(&accepted, 500);
        let output = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-ss", "0.250"])
            .arg("-i")
            .arg(&source)
            .args([
                "-t",
                "0.500",
                "-filter_complex",
                &filter,
                "-map",
                "[playback_audio]",
                "-vn",
                "-c:a",
                "pcm_f32le",
                "-f",
                "f32le",
                "-ar",
                "48000",
                "-ac",
                "1",
                "pipe:1",
            ])
            .output()
            .expect("decode playback PCM");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let samples = output
            .stdout
            .chunks_exact(4)
            .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 24_000, "500ms of mono 48kHz PCM");
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        assert!(
            (0.035..0.055).contains(&rms),
            "accepted 0.5 gain RMS: {rms}"
        );
        let positive_crossings = samples
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        assert!(
            (215..=225).contains(&positive_crossings),
            "440Hz retained across 500ms: {positive_crossings} crossings"
        );
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn playback_pcm_preserves_delayed_audio_and_pads_through_video_end() {
        let Some((toolchain, ffmpeg, _ffprobe)) = cross_platform_toolchain() else {
            return;
        };
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("delayed-short-audio.mp4");
        let status = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=black:size=32x24:rate=10:duration=1",
                "-itsoffset",
                "0.2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=0.4",
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
            ])
            .arg(&source)
            .status()
            .expect("FFmpeg starts");
        assert!(status.success());

        let probe = toolchain
            .probe(&source)
            .expect("probe delayed audio source");
        let edit = EditSpec {
            trim_start_ms: 100,
            trim_end_ms: Some(900),
            audio: AudioEdit {
                source_has_system_audio: true,
                mono_output: true,
                ..AudioEdit::default()
            },
            ..EditSpec::default()
        };
        let spec = ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };
        let attempt = export_attempts(&probe, &edit, &spec).unwrap().remove(0);
        let filter = playback_audio_filter(
            &audio_filter(&edit, &attempt).expect("accepted audio filter"),
            800,
        );
        let output = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-ss", "0.100"])
            .arg("-i")
            .arg(&source)
            .args([
                "-t",
                "0.800",
                "-filter_complex",
                &filter,
                "-map",
                "[playback_audio]",
                "-vn",
                "-c:a",
                "pcm_f32le",
                "-f",
                "f32le",
                "-ar",
                "48000",
                "-ac",
                "1",
                "pipe:1",
            ])
            .output()
            .expect("decode delayed playback PCM");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let samples = output
            .stdout
            .chunks_exact(4)
            .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 38_400, "audio clock spans the 800ms trim");
        let window_rms = |start_ms: usize, end_ms: usize| {
            let window = &samples[start_ms * 48..end_ms * 48];
            (window.iter().map(|sample| sample * sample).sum::<f32>() / window.len() as f32).sqrt()
        };
        assert!(
            window_rms(0, 80) < 0.02,
            "source-relative delay is retained before the tone"
        );
        assert!(
            window_rms(150, 450) > 0.07,
            "the asymmetric tone occupies its source-relative interval"
        );
        assert!(
            window_rms(600, 800) < 0.001,
            "clean audio EOF is padded with clock-advancing silence"
        );
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn create_export_estimate_recording(ffmpeg: &std::path::Path, path: &std::path::Path) {
        let status = std::process::Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=15:duration=8",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=8",
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "mpeg4",
                "-q:v",
                "3",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(path)
            .status()
            .expect("FFmpeg starts");
        assert!(status.success(), "export estimate recording generated");
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn export_estimate_scratch_entries() -> std::collections::BTreeSet<std::ffi::OsString> {
        std::fs::read_dir(std::env::temp_dir())
            .expect("temporary directory is readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| {
                name.to_string_lossy()
                    .starts_with("captures-export-estimate-")
            })
            .collect()
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn create_portrait_color_recording(ffmpeg: &std::path::Path, path: &std::path::Path) {
        let filter = "[0:v]drawbox=x=15:y=10:w=45:h=25:color=white:t=fill[red];\
                      [1:v]drawbox=x=15:y=10:w=45:h=25:color=white:t=fill[green];\
                      [2:v]drawbox=x=15:y=10:w=45:h=25:color=white:t=fill[blue];\
                      [red][green][blue]concat=n=3:v=1:a=0[video]";
        let status = std::process::Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "color=red:size=640x1440:rate=10:duration=1",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "color=green:size=640x1440:rate=10:duration=1",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "color=blue:size=640x1440:rate=10:duration=1",
            ])
            .args(["-filter_complex", filter, "-map", "[video]"])
            .args(["-c:v", "mpeg4", "-q:v", "2", "-an"])
            .arg(path)
            .status()
            .expect("bundled FFmpeg starts");
        assert!(status.success(), "portrait color recording generated");
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn sample_rgb(ffmpeg: &std::path::Path, path: &std::path::Path, at_ms: u64) -> [u8; 3] {
        let output = std::process::Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &seconds(at_ms),
                "-i",
            ])
            .arg(path)
            .args([
                "-vf",
                "crop=1:1:100:100,format=rgb24",
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-",
            ])
            .output()
            .expect("sample frame decoded");
        assert!(output.status.success(), "sample frame decoded");
        output.stdout[..3].try_into().expect("one RGB pixel")
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn export_size_estimates_preserve_shipping_exactness_and_clean_scratch() {
        let Some((toolchain, ffmpeg, ffprobe)) = cross_platform_toolchain() else {
            return;
        };
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        create_export_estimate_recording(&ffmpeg, &source);
        let source_bytes = std::fs::read(&source).expect("source bytes");
        let source_size = source_bytes.len() as u64;
        let mut identity = EditSpec::default();
        identity.audio.source_has_system_audio = true;
        let preserve_mp4 = ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };

        let estimate = toolchain
            .estimate_export_size(&source, &identity, &preserve_mp4, &CancelToken::default())
            .expect("identity estimate");
        assert_eq!(estimate.size_bytes, source_size);
        assert!(estimate.exact);

        let cancelled = CancelToken::default();
        cancelled.cancel();
        assert!(matches!(
            toolchain.estimate_export_size(&source, &identity, &preserve_mp4, &cancelled),
            Err(MediaToolError::Cancelled)
        ));

        let mut audio_only = identity.clone();
        audio_only.audio.system_volume = 0.5;
        let estimate = toolchain
            .estimate_export_size(&source, &audio_only, &preserve_mp4, &CancelToken::default())
            .expect("audio-only estimate");
        assert_eq!(estimate.size_bytes, source_size);
        assert!(!estimate.exact);

        let short_edit = EditSpec {
            trim_start_ms: 1_000,
            trim_end_ms: Some(4_500),
            crop: Some(CropRect {
                x: 10,
                y: 10,
                width: 600,
                height: 330,
            }),
            output_width: Some(320),
            output_height: Some(176),
            audio: identity.audio.clone(),
        };
        for (name, spec) in [
            (
                "mp4",
                ExportSpec {
                    quality: QualityPreset::Standard,
                    ..preserve_mp4.clone()
                },
            ),
            (
                "gif",
                ExportSpec {
                    format: ExportFormat::Gif,
                    quality: QualityPreset::Preserve,
                    max_size_bytes: None,
                    frames_per_second: Some(10),
                    gif_max_colors: Some(128),
                },
            ),
        ] {
            let estimate = toolchain
                .estimate_export_size(&source, &short_edit, &spec, &CancelToken::default())
                .unwrap_or_else(|error| panic!("{name} estimate failed: {error}"));
            assert!(estimate.exact, "short {name} estimate must be exact");
            let destination = directory.path().join(format!("short.{name}"));
            let exported = toolchain
                .export(
                    &source,
                    &destination,
                    &short_edit,
                    &spec,
                    &CancelToken::default(),
                    |_| {},
                )
                .unwrap_or_else(|error| panic!("{name} export failed: {error}"));
            assert_eq!(estimate.size_bytes, exported.size_bytes, "{name} size");
            assert_eq!(
                estimate.size_bytes,
                std::fs::metadata(destination).unwrap().len()
            );
        }

        let long_edit = EditSpec {
            trim_start_ms: 0,
            trim_end_ms: Some(8_000),
            crop: short_edit.crop,
            output_width: short_edit.output_width,
            output_height: short_edit.output_height,
            audio: identity.audio.clone(),
        };
        let estimate = toolchain
            .estimate_export_size(
                &source,
                &long_edit,
                &ExportSpec {
                    quality: QualityPreset::High,
                    ..preserve_mp4.clone()
                },
                &CancelToken::default(),
            )
            .expect("sampled estimate");
        assert!(estimate.size_bytes > 0);
        assert!(!estimate.exact);

        let invalid_edit = EditSpec {
            trim_start_ms: 4_000,
            trim_end_ms: Some(3_000),
            ..identity.clone()
        };
        assert!(matches!(
            toolchain.estimate_export_size(
                &source,
                &invalid_edit,
                &preserve_mp4,
                &CancelToken::default()
            ),
            Err(MediaToolError::InvalidEdit(_))
        ));
        assert!(
            toolchain
                .estimate_export_size(
                    &source,
                    &identity,
                    &ExportSpec {
                        format: ExportFormat::WebM,
                        ..preserve_mp4.clone()
                    },
                    &CancelToken::default(),
                )
                .is_err()
        );
        assert!(matches!(
            toolchain.estimate_export_size(
                &source,
                &identity,
                &ExportSpec {
                    max_size_bytes: Some(1),
                    ..preserve_mp4.clone()
                },
                &CancelToken::default(),
            ),
            Err(MediaToolError::UnattainableTarget)
        ));

        let scratch_before = export_estimate_scratch_entries();
        let broken = MediaToolchain::new(directory.path().join("missing-ffmpeg"), ffprobe);
        assert!(
            broken
                .estimate_export_size(&source, &short_edit, &preserve_mp4, &CancelToken::default(),)
                .is_err()
        );
        assert_eq!(export_estimate_scratch_entries(), scratch_before);
        assert_eq!(std::fs::read(source).unwrap(), source_bytes);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn portrait_gif_palette_preserves_late_scene_colors() {
        let Some((toolchain, ffmpeg, _)) = cross_platform_toolchain() else {
            return;
        };
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let preview = directory.path().join("preview.png");
        let destination = directory.path().join("trimmed.gif");
        create_portrait_color_recording(&ffmpeg, &source);
        let source_bytes = std::fs::read(&source).expect("source bytes");
        let edit = EditSpec {
            trim_start_ms: 1_100,
            trim_end_ms: Some(2_300),
            ..EditSpec::default()
        };
        let export = ExportSpec {
            format: ExportFormat::Gif,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: None,
        };

        toolchain
            .extract_edited_frame(
                &source,
                &edit,
                &export,
                2_100,
                &preview,
                &CancelToken::default(),
            )
            .expect("blue preview");
        toolchain
            .export(
                &source,
                &destination,
                &edit,
                &export,
                &CancelToken::default(),
                |_| {},
            )
            .expect("portrait GIF export");

        for path in [&preview, &destination] {
            let metadata = toolchain.probe(path).expect("output probe").metadata;
            assert_eq!((metadata.width, metadata.height), (640, 1_440));
        }
        let green = sample_rgb(&ffmpeg, &destination, 200);
        assert!(
            green[1] > 100 && green[0] < 20 && green[2] < 20,
            "early trimmed frame should remain green, got {green:?}"
        );
        let preview_blue = sample_rgb(&ffmpeg, &preview, 0);
        let gif_blue = sample_rgb(&ffmpeg, &destination, 1_000);
        for (name, blue) in [("preview", preview_blue), ("GIF", gif_blue)] {
            assert!(
                blue[2] > 220 && blue[0] < 20 && blue[1] < 20,
                "late {name} frame should remain blue, got {blue:?}"
            );
        }
        assert_eq!(
            std::fs::read(source).expect("unchanged source bytes"),
            source_bytes
        );
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn cross_platform_media_pipeline_encodes_and_muxes_audio_tracks() {
        let Some((toolchain, ffmpeg, _)) = cross_platform_toolchain() else {
            return;
        };

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
        let codec = std::process::Command::new(&toolchain.ffprobe)
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

        let system_audio = directory.path().join("system.wav");
        let microphone = directory.path().join("microphone.wav");
        for (path, frequency) in [(&system_audio, 440), (&microphone, 880)] {
            let source = format!("sine=frequency={frequency}:sample_rate=48000");
            let status = std::process::Command::new(&ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &source,
                    "-t",
                    "1",
                    "-c:a",
                    "pcm_f32le",
                ])
                .arg(path)
                .status()
                .expect("audio fixture generated");
            assert!(status.success());
        }
        let assembled = directory.path().join("assembled.mp4");
        let outcome = toolchain
            .assemble_recording(
                &[RecordingSegmentInput {
                    video_path: source.clone(),
                    system_audio_path: Some(system_audio),
                    system_audio_offset_ms: 0,
                    microphone_path: Some(microphone),
                    microphone_offset_ms: 0,
                    duration_ms: 1_000,
                }],
                &assembled,
                directory.path(),
                RecordingAssemblyKind::Video {
                    capture_system_audio: true,
                },
                &CancelToken::default(),
            )
            .expect("independent audio tracks assembled");
        assert!(outcome.has_microphone_audio);
        assert_eq!(outcome.probe.audio_stream_count, 3);

        let front_left_mic = directory.path().join("microphone-fl.wav");
        let status = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "1",
                "-ac",
                "1",
                "-ch_layout",
                "FL",
                "-c:a",
                "pcm_f32le",
            ])
            .arg(&front_left_mic)
            .status()
            .expect("front-left microphone fixture generated");
        assert!(status.success());
        let assembled_front_left = directory.path().join("assembled-fl.mp4");
        toolchain
            .assemble_recording_segments(
                &[RecordingSegmentInput {
                    video_path: source,
                    system_audio_path: None,
                    system_audio_offset_ms: 0,
                    microphone_path: Some(front_left_mic),
                    microphone_offset_ms: 0,
                    duration_ms: 1_000,
                }],
                &assembled_front_left,
                RecordingAudioLayout {
                    system_audio: false,
                    microphone_audio: true,
                },
                &CancelToken::default(),
            )
            .expect("front-left microphone audio assembled");
        let probe = toolchain
            .probe(&assembled_front_left)
            .expect("front-left recording probe");
        assert!(probe.has_audio);
        assert_eq!(probe.audio_stream_count, 1);
        let (left_rms, right_rms) =
            stereo_channel_rms(&ffmpeg, &assembled_front_left).expect("channel energy");
        assert!(
            left_rms > 0.01 && right_rms > 0.01,
            "centered microphone should be audible on both channels, got L={left_rms} R={right_rms}"
        );
        let louder = left_rms.max(right_rms);
        let quieter = left_rms.min(right_rms);
        assert!(
            quieter / louder > 0.9,
            "Front Left microphone should be duplicated, not left-only, got L={left_rms} R={right_rms}"
        );
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn recording_assembly_produces_silent_video_and_multisegment_gif() {
        let Some((toolchain, ffmpeg, _)) = cross_platform_toolchain() else {
            return;
        };
        let directory = tempfile::tempdir().expect("temporary directory");
        let first = directory.path().join("first.mp4");
        let second = directory.path().join("second.mp4");
        create_cross_platform_segment(&ffmpeg, &first);
        create_cross_platform_segment(&ffmpeg, &second);
        let segment = |video_path| RecordingSegmentInput {
            video_path,
            system_audio_path: None,
            system_audio_offset_ms: 37,
            microphone_path: None,
            microphone_offset_ms: -19,
            duration_ms: 500,
        };

        let video = directory.path().join("assembled.mp4");
        let video_outcome = toolchain
            .assemble_recording(
                &[segment(first.clone())],
                &video,
                directory.path(),
                RecordingAssemblyKind::Video {
                    capture_system_audio: false,
                },
                &CancelToken::default(),
            )
            .expect("silent video assembled");
        assert!(!video_outcome.probe.has_audio);
        assert!(!video_outcome.has_microphone_audio);
        assert_eq!(video_outcome.probe.metadata.kind, MediaKind::Video);

        let gif = directory.path().join("assembled.gif");
        let gif_outcome = toolchain
            .assemble_recording(
                &[segment(first), segment(second)],
                &gif,
                directory.path(),
                RecordingAssemblyKind::Gif {
                    frames_per_second: 10,
                    max_width: 120,
                    max_colors: 64,
                },
                &CancelToken::default(),
            )
            .expect("multi-segment GIF assembled");
        assert_eq!(gif_outcome.probe.metadata.kind, MediaKind::Gif);
        assert_eq!(gif_outcome.probe.metadata.width, 120);
        assert!(!gif_outcome.has_microphone_audio);
        assert!(directory.path().join("master.mp4").is_file());
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn recording_assembly_cancellation_does_not_publish_gif() {
        let Some((toolchain, ffmpeg, _)) = cross_platform_toolchain() else {
            return;
        };
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        let destination = directory.path().join("assembled.gif");
        create_cross_platform_segment(&ffmpeg, &source);
        let cancel = CancelToken::default();
        cancel.cancel();

        let error = toolchain
            .assemble_recording(
                &[RecordingSegmentInput {
                    video_path: source,
                    system_audio_path: None,
                    system_audio_offset_ms: 0,
                    microphone_path: None,
                    microphone_offset_ms: 0,
                    duration_ms: 500,
                }],
                &destination,
                directory.path(),
                RecordingAssemblyKind::Gif {
                    frames_per_second: 10,
                    max_width: 120,
                    max_colors: 64,
                },
                &cancel,
            )
            .expect_err("cancelled GIF assembly");
        assert!(matches!(error, super::MediaToolError::Cancelled));
        assert!(!destination.exists());
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn stereo_channel_rms(
        ffmpeg: &std::path::Path,
        input: &std::path::Path,
    ) -> Result<(f32, f32), String> {
        let output = std::process::Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(input)
            .args(["-map", "0:a:0", "-f", "f32le", "-ac", "2", "-"])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        let samples = output
            .stdout
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("f32le sample")))
            .collect::<Vec<_>>();
        if samples.len() < 2 {
            return Err("PCM dump was empty".to_owned());
        }
        let mut left_sum = 0.0_f32;
        let mut right_sum = 0.0_f32;
        let mut frames = 0.0_f32;
        for pair in samples.chunks_exact(2) {
            left_sum += pair[0] * pair[0];
            right_sum += pair[1] * pair[1];
            frames += 1.0;
        }
        Ok(((left_sum / frames).sqrt(), (right_sum / frames).sqrt()))
    }
}
