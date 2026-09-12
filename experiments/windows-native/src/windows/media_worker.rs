use captures_media::{
    AudioEdit, CancelToken, EditSpec, ExportFormat, ExportOutcome, ExportSpec, MediaToolchain,
    ProbeResult, QualityPreset, extrapolate_sampled_size,
};
use image::RgbaImage;
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread,
};

pub enum Event {
    Probe {
        epoch: u64,
        source: PathBuf,
        result: Result<ProbeResult, String>,
    },
    Frame {
        epoch: u64,
        request: u64,
        result: Result<RgbaImage, String>,
    },
    Comparison {
        epoch: u64,
        request: u64,
        result: Result<(RgbaImage, u64), String>,
    },
    Export {
        epoch: u64,
        request: u64,
        source: PathBuf,
        destination: PathBuf,
        save_as_new: bool,
        result: Result<ExportOutcome, String>,
    },
}

pub struct MediaWorker {
    sender: SyncSender<Event>,
    receiver: Receiver<Event>,
    playback_cancel: Option<Arc<AtomicBool>>,
    frame_cancel: Option<CancelToken>,
    comparison_cancel: Option<CancelToken>,
    export_cancel: Option<CancelToken>,
}

impl MediaWorker {
    pub fn new() -> Self {
        let (sender, receiver) = sync_channel(3);
        Self {
            sender,
            receiver,
            playback_cancel: None,
            frame_cancel: None,
            comparison_cancel: None,
            export_cancel: None,
        }
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }

    pub fn probe(&self, epoch: u64, source: PathBuf) {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = MediaToolchain::from_command_names()
                .verify()
                .and_then(|()| MediaToolchain::from_command_names().probe(&source))
                .map_err(|error| error.to_string());
            let _ = sender.send(Event::Probe {
                epoch,
                source,
                result,
            });
        });
    }

    pub fn still(&mut self, epoch: u64, request: u64, source: PathBuf, at_ms: u64) {
        if let Some(token) = self.frame_cancel.take() {
            token.cancel();
        }
        let token = CancelToken::default();
        self.frame_cancel = Some(token.clone());
        let sender = self.sender.clone();
        thread::spawn(move || {
            let scratch = std::env::temp_dir().join(format!(
                "captures-windows-frame-{}-{}.png",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let result = MediaToolchain::from_command_names()
                .extract_frame(&source, at_ms, &scratch, &token)
                .map_err(|error| error.to_string())
                .and_then(|()| image::open(&scratch).map_err(|error| error.to_string()))
                .map(|image| image.to_rgba8());
            let _ = std::fs::remove_file(scratch);
            let _ = sender.send(Event::Frame {
                epoch,
                request,
                result,
            });
        });
    }

    pub fn play(&mut self, spec: PlaybackSpec) {
        self.cancel_playback();
        if let Some(token) = self.frame_cancel.take() {
            token.cancel();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.playback_cancel = Some(cancelled.clone());
        let sender = self.sender.clone();
        thread::spawn(move || decode_playback(spec, cancelled, sender));
    }

    pub fn cancel_playback(&mut self) {
        if let Some(cancelled) = self.playback_cancel.take() {
            cancelled.store(true, Ordering::Release);
        }
    }

    pub fn cancel_jobs(&mut self) {
        self.cancel_playback();
        if let Some(token) = self.frame_cancel.take() {
            token.cancel();
        }
        if let Some(token) = self.comparison_cancel.take() {
            token.cancel();
        }
        if let Some(token) = self.export_cancel.take() {
            token.cancel();
        }
    }

    pub fn compare(&mut self, spec: ComparisonSpec) {
        if let Some(token) = self.comparison_cancel.take() {
            token.cancel();
        }
        let token = CancelToken::default();
        self.comparison_cancel = Some(token.clone());
        let sender = self.sender.clone();
        thread::spawn(move || {
            let epoch = spec.epoch;
            let request = spec.request;
            let result = build_comparison(spec, &token);
            let _ = sender.send(Event::Comparison {
                epoch,
                request,
                result,
            });
        });
    }

    pub fn export(&mut self, spec: ExportJob) {
        if let Some(token) = self.export_cancel.take() {
            token.cancel();
        }
        let token = CancelToken::default();
        self.export_cancel = Some(token.clone());
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = MediaToolchain::from_command_names()
                .export(
                    &spec.source,
                    &spec.destination,
                    &spec.edit,
                    &spec.export,
                    &token,
                    |_| {},
                )
                .map_err(|error| error.to_string());
            let destination = spec.destination.clone();
            let delivered = sender
                .send(Event::Export {
                    epoch: spec.epoch,
                    request: spec.request,
                    source: spec.source,
                    destination: spec.destination,
                    save_as_new: spec.save_as_new,
                    result,
                })
                .is_ok();
            captures_windows_native::async_state::cleanup_undelivered_staged_file(
                &destination,
                delivered,
            );
        });
    }
}

impl Drop for MediaWorker {
    fn drop(&mut self) {
        self.cancel_jobs();
    }
}

pub struct PlaybackSpec {
    pub epoch: u64,
    pub request: u64,
    pub source: PathBuf,
    pub start_ms: u64,
    pub end_ms: u64,
    pub width: u32,
    pub height: u32,
}

pub struct ComparisonSpec {
    pub epoch: u64,
    pub request: u64,
    pub source: PathBuf,
    pub before: RgbaImage,
    pub position_ms: u64,
    pub trim_start_ms: u64,
    pub trim_end_ms: u64,
    pub quality: QualityPreset,
    pub has_audio: bool,
    pub frames_per_second: u16,
    pub gif_max_colors: u16,
}

pub struct ExportJob {
    pub epoch: u64,
    pub request: u64,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub save_as_new: bool,
    pub edit: EditSpec,
    pub export: ExportSpec,
}

fn build_comparison(spec: ComparisonSpec, token: &CancelToken) -> Result<(RgbaImage, u64), String> {
    let sample_start = spec.position_ms.saturating_sub(500).max(spec.trim_start_ms);
    let sample_end = (sample_start + 1_000).min(spec.trim_end_ms);
    let sample_duration = sample_end.saturating_sub(sample_start);
    if sample_duration == 0 {
        return Err("compression comparison range is empty".into());
    }
    let format = if spec
        .source
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("gif"))
    {
        ExportFormat::Gif
    } else {
        ExportFormat::Mp4
    };
    let extension = if format == ExportFormat::Gif {
        "gif"
    } else {
        "mp4"
    };
    let scratch = std::env::temp_dir();
    let id = uuid::Uuid::new_v4();
    let sample = scratch.join(format!("captures-compare-{id}.{extension}"));
    let after_path = scratch.join(format!("captures-compare-{id}.png"));
    let tools = MediaToolchain::from_command_names();
    let result = (|| {
        let outcome = tools
            .export(
                &spec.source,
                &sample,
                &EditSpec {
                    trim_start_ms: sample_start,
                    trim_end_ms: Some(sample_end),
                    audio: AudioEdit {
                        source_has_system_audio: spec.has_audio,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                &ExportSpec {
                    format,
                    quality: spec.quality,
                    max_size_bytes: None,
                    frames_per_second: Some(spec.frames_per_second),
                    gif_max_colors: (format == ExportFormat::Gif).then_some(spec.gif_max_colors),
                },
                token,
                |_| {},
            )
            .map_err(|error| error.to_string())?;
        tools
            .extract_frame(
                &sample,
                spec.position_ms.saturating_sub(sample_start),
                &after_path,
                token,
            )
            .map_err(|error| error.to_string())?;
        let mut after = image::open(&after_path)
            .map_err(|error| error.to_string())?
            .to_rgba8();
        if after.dimensions() != spec.before.dimensions() {
            after = image::imageops::resize(
                &after,
                spec.before.width(),
                spec.before.height(),
                image::imageops::FilterType::Triangle,
            );
        }
        let mut comparison = spec.before;
        let split = comparison.width() / 2;
        for y in 0..comparison.height() {
            for x in split..comparison.width() {
                comparison.put_pixel(x, y, *after.get_pixel(x, y));
            }
        }
        Ok((
            comparison,
            extrapolate_sampled_size(
                outcome.size_bytes,
                sample_duration,
                spec.trim_end_ms.saturating_sub(spec.trim_start_ms),
            ),
        ))
    })();
    let _ = std::fs::remove_file(sample);
    let _ = std::fs::remove_file(after_path);
    result
}

fn decode_playback(spec: PlaybackSpec, cancelled: Arc<AtomicBool>, sender: SyncSender<Event>) {
    let duration_ms = spec.end_ms.saturating_sub(spec.start_ms);
    let mut child = match Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{:.3}", spec.start_ms as f64 / 1_000.0))
        .args(["-re", "-i"])
        .arg(&spec.source)
        .args(["-t", &format!("{:.3}", duration_ms as f64 / 1_000.0)])
        .args([
            "-vf", "fps=15", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(Event::Frame {
                epoch: spec.epoch,
                request: spec.request,
                result: Err(format!("could not start FFmpeg playback decoder: {error}")),
            });
            return;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let frame_size = usize::try_from(spec.width)
        .ok()
        .and_then(|width| {
            usize::try_from(spec.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4));
    let Some(frame_size) = frame_size else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let mut bytes = vec![0; frame_size];
    while !cancelled.load(Ordering::Acquire) {
        if let Err(error) = stdout.read_exact(&mut bytes) {
            if error.kind() != std::io::ErrorKind::UnexpectedEof {
                let _ = sender.try_send(Event::Frame {
                    epoch: spec.epoch,
                    request: spec.request,
                    result: Err(format!("FFmpeg playback decode failed: {error}")),
                });
            }
            break;
        }
        let Some(image) = RgbaImage::from_raw(spec.width, spec.height, bytes.clone()) else {
            break;
        };
        let event = Event::Frame {
            epoch: spec.epoch,
            request: spec.request,
            result: Ok(image),
        };
        match sender.try_send(event) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
