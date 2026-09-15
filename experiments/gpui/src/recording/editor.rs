#[path = "playback.rs"]
mod playback;

use self::playback::{AudioPlayer, VideoDecoder};
use super::model::{EditorState, Rect, timestamped};
use crate::{
    Launch,
    preferences::input::TextInput,
    theme::{self, Theme},
};
use captures_media::{
    CancelToken, CropRect, ExportFormat, MediaToolchain, QualityPreset, RecordingAudioLayout,
};
use gpui::{prelude::*, *};
use std::{
    cell::Cell,
    collections::hash_map::DefaultHasher,
    fs::File,
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

fn source_cache_key(source: &Path) -> anyhow::Result<String> {
    let canonical = source.canonicalize()?;
    let metadata = canonical.metadata()?;
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    let mut file = File::open(&canonical)?;
    let mut sample = vec![0; 64 * 1024];
    for offset in [
        0,
        metadata.len() / 2,
        metadata.len().saturating_sub(sample.len() as u64),
    ] {
        file.seek(SeekFrom::Start(offset))?;
        let read = file.read(&mut sample)?;
        sample[..read].hash(&mut hasher);
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn source_frame_rate(source: &Path) -> f64 {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=avg_frame_rate",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(source)
        .output();
    let rate = output
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok());
    rate.and_then(|value| {
        let (n, d) = value.trim().split_once('/')?;
        Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok()?)
    })
    .filter(|v| v.is_finite() && *v > 0.)
    .unwrap_or(30.)
}

fn configured_output_directory(profile: &Path) -> Option<PathBuf> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profile.join("settings.json")).ok()?).ok()?;
    let path = value.get("output_directory")?.as_str()?;
    (!path.trim().is_empty()).then(|| PathBuf::from(path))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct AudioTracks {
    system: bool,
    microphone: bool,
}

fn audio_tracks(source: &Path) -> AudioTracks {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream_tags=title",
            "-of",
            "json",
        ])
        .arg(source)
        .output();
    output
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let json: serde_json::Value = serde_json::from_slice(&o.stdout).ok()?;
            Some(audio_tracks_from_probe(&json))
        })
        .unwrap_or_default()
}

fn audio_tracks_from_probe(probe: &serde_json::Value) -> AudioTracks {
    let mut tracks = AudioTracks::default();
    for title in probe["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|stream| stream["tags"]["title"].as_str())
    {
        let title = title.to_ascii_lowercase();
        tracks.microphone |= title.contains("microphone") || title.contains("mic");
        tracks.system |= title.contains("system") || title.contains("desktop");
    }
    tracks
}

struct PreparedEditor {
    cache_key: String,
    poster: Option<Arc<Path>>,
    decoder: VideoDecoder,
    audio: AudioPlayer,
    source_width: u32,
    source_height: u32,
    timeline_frames: Vec<Arc<Path>>,
    waveform: Option<Arc<Path>>,
    draft_path: PathBuf,
    state: EditorState,
    has_audio: bool,
    audio_tracks: AudioTracks,
}

fn prepare_editor(
    source: &Path,
    profile: &Path,
    cancel: &CancelToken,
) -> anyhow::Result<PreparedEditor> {
    let tools = MediaToolchain::from_command_names();
    tools
        .verify()
        .map_err(|e| anyhow::anyhow!("Recording editor requires FFmpeg and ffprobe: {e}"))?;
    let probe = tools.probe(source)?;
    let duration = probe.metadata.duration_ms.unwrap_or(1).max(1);
    std::fs::create_dir_all(profile)?;
    let cache_key = source_cache_key(source)?;
    let cache_dir = profile.join("recording-editor").join(&cache_key);
    std::fs::create_dir_all(&cache_dir)?;
    let poster_path = cache_dir.join("poster.png");
    let poster = tools
        .create_poster(source, &poster_path, cancel)
        .ok()
        .map(|_| Arc::<Path>::from(poster_path));
    let timeline_dir = cache_dir.join("timeline");
    std::fs::create_dir_all(&timeline_dir)?;
    let mut timeline_frames = Vec::with_capacity(12);
    let last_frame_ms = duration.saturating_sub((1000. / source_frame_rate(source)).ceil() as u64);
    for index in 0..12 {
        if cancel.is_cancelled() {
            anyhow::bail!("Editor loading cancelled");
        }
        let path = timeline_dir.join(format!("frame-{index}.png"));
        let at = duration.saturating_mul(index) / 11;
        tools.extract_frame(source, at.min(last_frame_ms), &path, cancel)?;
        image::image_dimensions(&path)?;
        timeline_frames.push(Arc::<Path>::from(path));
    }
    let waveform_path = timeline_dir.join("waveform.png");
    let waveform = if probe.has_audio && !cancel.is_cancelled() {
        let mut child = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(source)
            .args([
                "-filter_complex",
                "aformat=channel_layouts=mono,showwavespic=s=1200x80:colors=#8b8f98",
                "-frames:v",
                "1",
            ])
            .arg(&waveform_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let succeeded = loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
            if let Some(status) = child.try_wait()? {
                break status.success();
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        succeeded.then(|| Arc::<Path>::from(waveform_path))
    } else {
        None
    };
    if cancel.is_cancelled() {
        anyhow::bail!("Editor loading cancelled");
    }
    let format = if source
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
    {
        ExportFormat::Gif
    } else {
        ExportFormat::Mp4
    };
    let draft_path = cache_dir.join("draft.json");
    let state = std::fs::read(&draft_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<EditorState>(&bytes).ok())
        .filter(|state| state.duration_ms == duration)
        .unwrap_or_else(|| EditorState::new(duration, format));
    Ok(PreparedEditor {
        cache_key,
        poster,
        decoder: VideoDecoder::new(
            source,
            probe.metadata.width,
            probe.metadata.height,
            duration,
            source_frame_rate(source),
        ),
        audio: AudioPlayer::new(source, probe.audio_stream_count),
        source_width: probe.metadata.width,
        source_height: probe.metadata.height,
        timeline_frames,
        waveform,
        draft_path,
        state,
        has_audio: probe.has_audio,
        audio_tracks: audio_tracks(source),
    })
}

fn fitted_video_bounds(container: Bounds<Pixels>, source: (u32, u32)) -> Bounds<Pixels> {
    let scale = (f32::from(container.size.width) / source.0.max(1) as f32)
        .min(f32::from(container.size.height) / source.1.max(1) as f32);
    let size = size(px(source.0 as f32 * scale), px(source.1 as f32 * scale));
    Bounds {
        origin: point(
            container.origin.x + (container.size.width - size.width) / 2.,
            container.origin.y + (container.size.height - size.height) / 2.,
        ),
        size,
    }
}

fn source_point(position: Point<Pixels>, fitted: Bounds<Pixels>, source: (u32, u32)) -> (f32, f32) {
    (
        ((f32::from(position.x - fitted.origin.x) / f32::from(fitted.size.width).max(1.))
            * source.0 as f32)
            .clamp(0., source.0 as f32),
        ((f32::from(position.y - fitted.origin.y) / f32::from(fitted.size.height).max(1.))
            * source.1 as f32)
            .clamp(0., source.1 as f32),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineDrag {
    Playhead,
    TrimStart,
    TrimEnd,
}

#[derive(Clone, Copy)]
struct CropDrag {
    handle: usize,
    start: (f32, f32),
    initial: Rect,
}

fn timeline_value(x: f32, viewport_width: f32, duration_ms: u64) -> u64 {
    let usable = (viewport_width - 48.).max(1.);
    (((x - 24.) / usable).clamp(0., 1.) * duration_ms as f32).round() as u64
}

fn loading_result_matches(
    current_request: u64,
    result_request: u64,
    current_source: &Path,
    result_source: &Path,
) -> bool {
    current_request == result_request && current_source == result_source
}

pub struct RecordingEditor {
    launch: Launch,
    focus: Option<FocusHandle>,
    source: PathBuf,
    poster: Option<Arc<Path>>,
    frame: Option<Arc<RenderImage>>,
    decoder: VideoDecoder,
    audio: AudioPlayer,
    source_width: u32,
    source_height: u32,
    timeline_frames: Vec<Arc<Path>>,
    waveform: Option<Arc<Path>>,
    draft_path: PathBuf,
    generation: u64,
    state: EditorState,
    has_audio: bool,
    audio_tracks: AudioTracks,
    preview_bounds: Rc<Cell<Bounds<Pixels>>>,
    crop_drag: Option<CropDrag>,
    aspect_locked: bool,
    width_input: Option<Entity<TextInput>>,
    height_input: Option<Entity<TextInput>>,
    destination: PathBuf,
    saved_path: Option<PathBuf>,
    close_hook_installed: bool,
    playing: bool,
    playhead_ms: u64,
    clock_started: Option<(Instant, u64)>,
    tick_generation: u64,
    pending_frame: bool,
    drag: Option<TimelineDrag>,
    exporting: bool,
    export_cancel: Option<CancelToken>,
    export_progress: Arc<AtomicU64>,
    status: String,
    loading: bool,
    load_started: bool,
    load_request: u64,
    source_identity: Option<String>,
    load_cancel: Option<CancelToken>,
}

impl RecordingEditor {
    pub fn new(launch: Launch) -> anyhow::Result<Self> {
        let source = launch
            .path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("recording-editor requires --open FILE"))?;
        let destination = configured_output_directory(&launch.profile)
            .unwrap_or_else(|| launch.profile.join("captures"));
        let saved_path = (source.parent() != Some(launch.profile.join("captures").as_path()))
            .then(|| source.clone());
        Ok(Self {
            decoder: VideoDecoder::new(&source, 1, 1, 1, 30.),
            audio: AudioPlayer::new(&source, 0),
            source_width: 1,
            source_height: 1,
            timeline_frames: Vec::new(),
            waveform: None,
            draft_path: launch.profile.join("recording-editor.pending.json"),
            launch,
            focus: None,
            source,
            poster: None,
            frame: None,
            generation: 0,
            state: EditorState::new(1, ExportFormat::Mp4),
            has_audio: false,
            audio_tracks: AudioTracks::default(),
            preview_bounds: Rc::new(Cell::new(Bounds::default())),
            crop_drag: None,
            aspect_locked: true,
            width_input: None,
            height_input: None,
            destination,
            saved_path,
            close_hook_installed: false,
            playing: false,
            playhead_ms: 0,
            clock_started: None,
            tick_generation: 0,
            pending_frame: false,
            drag: None,
            exporting: false,
            export_cancel: None,
            export_progress: Arc::new(AtomicU64::new(0)),
            status: "Preparing preview…".into(),
            loading: true,
            load_started: false,
            load_request: 0,
            source_identity: None,
            load_cancel: None,
        })
    }

    fn start_loading(&mut self, cx: &mut Context<Self>) {
        if self.load_started {
            return;
        }
        self.load_started = true;
        self.load_request += 1;
        let request = self.load_request;
        let source = self.source.clone();
        let result_source = source.clone();
        let profile = self.launch.profile.clone();
        let cancel = CancelToken::default();
        self.load_cancel = Some(cancel.clone());
        let task = cx
            .background_executor()
            .spawn(async move { prepare_editor(&source, &profile, &cancel) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                if !loading_result_matches(s.load_request, request, &s.source, &result_source) {
                    return;
                }
                s.loading = false;
                s.load_cancel = None;
                match result {
                    Ok(prepared) => {
                        s.source_identity = Some(prepared.cache_key);
                        s.poster = prepared.poster;
                        s.decoder = prepared.decoder;
                        s.audio = prepared.audio;
                        s.source_width = prepared.source_width;
                        s.source_height = prepared.source_height;
                        s.timeline_frames = prepared.timeline_frames;
                        s.waveform = prepared.waveform;
                        s.draft_path = prepared.draft_path;
                        s.state = prepared.state;
                        s.has_audio = prepared.has_audio;
                        s.audio_tracks = prepared.audio_tracks;
                        s.status.clear();
                        s.width_input = None;
                        s.height_input = None;
                        s.seek(s.state.trim_start_ms, cx);
                    }
                    Err(error) => s.status = format!("Could not load recording: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn seek(&mut self, timestamp_ms: u64, cx: &mut Context<Self>) {
        self.playhead_ms = timestamp_ms.min(self.state.duration_ms.saturating_sub(1));
        self.pending_frame = true;
        self.clock_started = self.playing.then(|| (Instant::now(), self.playhead_ms));
        match self
            .decoder
            .restart(self.playhead_ms, self.playing, self.state.playback_rate)
        {
            Ok(generation) => {
                self.generation = generation;
                let layout = RecordingAudioLayout {
                    system_audio: self.audio_tracks.system || !self.audio_tracks.microphone,
                    microphone_audio: self.audio_tracks.microphone,
                };
                let audible = (layout.system_audio && !self.state.mute_system_audio)
                    || (layout.microphone_audio && !self.state.mute_microphone);
                if self.playing && self.has_audio && audible {
                    let mut edit = self.state.edit(self.has_audio).audio;
                    edit.source_has_system_audio = layout.system_audio;
                    edit.source_has_microphone_audio = layout.microphone_audio;
                    if let Err(error) =
                        self.audio
                            .play(self.playhead_ms, self.state.playback_rate, &edit, layout)
                    {
                        self.status = format!("Audio preview failed: {error}");
                    }
                } else {
                    self.audio.stop();
                }
                self.start_ticks(cx);
            }
            Err(error) => {
                self.playing = false;
                self.pending_frame = false;
                self.status = format!("Preview failed: {error}");
            }
        }
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.playing = false;
            self.clock_started = None;
            self.tick_generation += 1;
            self.decoder.stop();
            self.audio.stop();
        } else {
            if self.playhead_ms >= self.state.trim_end_ms.saturating_sub(1) {
                self.playhead_ms = self.state.trim_start_ms;
            }
            self.playing = true;
            self.seek(self.playhead_ms.max(self.state.trim_start_ms), cx);
        }
        cx.notify();
    }

    fn start_ticks(&mut self, cx: &mut Context<Self>) {
        self.tick_generation += 1;
        let tick = self.tick_generation;
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(16)).await;
                let keep_going = this
                    .update(cx, |s, cx| {
                        if s.tick_generation != tick {
                            return false;
                        }
                        if s.playing
                            && let Err(error) = s.audio.check()
                        {
                            s.playing = false;
                            s.clock_started = None;
                            s.decoder.stop();
                            s.status = format!("Audio preview failed: {error}");
                            cx.notify();
                            return false;
                        }
                        let target = s
                            .clock_started
                            .map(|(at, start)| {
                                start.saturating_add(
                                    (at.elapsed().as_millis() as f32 * s.state.playback_rate)
                                        as u64,
                                )
                            })
                            .unwrap_or(s.playhead_ms);
                        if let Some((at, image)) =
                            s.decoder.latest_at_or_before(s.generation, target)
                        {
                            if s.pending_frame || at != s.playhead_ms || s.frame.is_none() {
                                s.playhead_ms = at;
                                s.frame = Some(image);
                                cx.notify();
                            }
                            s.pending_frame = false;
                            if !s.playing {
                                s.decoder.stop();
                            }
                        }
                        if s.pending_frame && s.decoder.finished() {
                            s.pending_frame = false;
                            s.playing = false;
                            s.status =
                                "FFmpeg ended without a preview frame at this position".into();
                            cx.notify();
                        }
                        if s.playing && target >= s.state.trim_end_ms {
                            s.playing = false;
                            s.clock_started = None;
                            s.decoder.stop();
                            s.audio.stop();
                            cx.notify();
                            return false;
                        }
                        s.playing || s.pending_frame
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    fn export(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let ext = if self.state.format == ExportFormat::Gif {
            "gif"
        } else if self.state.format == ExportFormat::WebM {
            "webm"
        } else {
            "mp4"
        };
        let dir = self.destination.clone();
        let destination = timestamped(&dir, ext);
        if destination == self.source {
            self.status = "Export destination cannot overwrite the source".into();
            return;
        }
        let source = self.source.clone();
        let mut edit = self.state.edit(self.has_audio);
        edit.audio.source_has_system_audio = self.audio_tracks.system;
        edit.audio.source_has_microphone_audio = self.audio_tracks.microphone;
        let export = self.state.export();
        let cancel = CancelToken::default();
        self.export_cancel = Some(cancel.clone());
        self.exporting = true;
        self.export_progress.store(0, Ordering::Release);
        self.status = "Preparing export…".into();
        let progress = self.export_progress.clone();
        let task = cx.background_executor().spawn(async move {
            std::fs::create_dir_all(&dir).map_err(anyhow::Error::from)?;
            MediaToolchain::from_command_names()
                .export(&source, &destination, &edit, &export, &cancel, |p| {
                    progress.store(p.completed_per_mille as u64, Ordering::Release);
                })
                .map_err(anyhow::Error::from)?;
            Ok::<_, anyhow::Error>(destination)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.exporting = false;
                s.export_cancel = None;
                s.status = match result {
                    Ok(path) => {
                        s.saved_path = Some(path.clone());
                        format!("Saved {}", path.display())
                    }
                    Err(e) => format!("Export failed: {e}"),
                };
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                let active = this
                    .update(cx, |s, cx| {
                        if s.exporting {
                            cx.notify();
                        }
                        s.exporting
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn timeline_update(&mut self, event_x: f32, width: f32, cx: &mut Context<Self>) {
        let value = timeline_value(event_x, width, self.state.duration_ms);
        match self.drag.unwrap_or(TimelineDrag::Playhead) {
            TimelineDrag::Playhead => self.seek(value, cx),
            TimelineDrag::TrimStart => {
                self.state.trim_start_ms = value.min(self.state.trim_end_ms.saturating_sub(1));
                self.seek(self.state.trim_start_ms, cx);
            }
            TimelineDrag::TrimEnd => {
                self.state.trim_end_ms = value
                    .max(self.state.trim_start_ms + 1)
                    .min(self.state.duration_ms);
                self.seek(self.state.trim_end_ms.saturating_sub(1), cx);
            }
        }
        self.save_draft();
    }

    fn save_draft(&self) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.state) {
            let temporary = self.draft_path.with_extension("json.tmp");
            if std::fs::write(&temporary, bytes).is_ok() {
                let _ = std::fs::rename(temporary, &self.draft_path);
            }
        }
    }

    fn timeline_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let width = f32::from(window.viewport_size().width);
        let x = f32::from(e.position.x);
        let value = timeline_value(x, width, self.state.duration_ms);
        let tolerance = self.state.duration_ms / 40 + 1;
        self.drag = Some(if value.abs_diff(self.state.trim_start_ms) <= tolerance {
            TimelineDrag::TrimStart
        } else if value.abs_diff(self.state.trim_end_ms) <= tolerance {
            TimelineDrag::TrimEnd
        } else {
            TimelineDrag::Playhead
        });
        self.timeline_update(x, width, cx);
    }
    fn timeline_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            self.timeline_update(
                f32::from(e.position.x),
                f32::from(window.viewport_size().width),
                cx,
            );
        }
    }

    fn crop_down(&mut self, e: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(crop) = self.state.crop else { return };
        let fitted = self.preview_bounds.get();
        let point = source_point(e.position, fitted, (self.source_width, self.source_height));
        let rect = Rect {
            x: crop.x as f32,
            y: crop.y as f32,
            width: crop.width as f32,
            height: crop.height as f32,
        };
        let radius = 14. * self.source_width as f32 / f32::from(fitted.size.width).max(1.);
        let handle = rect
            .handles()
            .iter()
            .position(|p| (p.0 - point.0).hypot(p.1 - point.1) <= radius)
            .unwrap_or(8);
        self.crop_drag = Some(CropDrag {
            handle,
            start: point,
            initial: rect,
        });
    }

    fn crop_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.crop_drag else { return };
        let point = source_point(
            e.position,
            self.preview_bounds.get(),
            (self.source_width, self.source_height),
        );
        let adjusted = drag.initial.adjusted(
            drag.handle,
            (point.0 - drag.start.0, point.1 - drag.start.1),
            (self.source_width as f32, self.source_height as f32),
        );
        self.state.set_crop(
            Some(CropRect {
                x: adjusted.x.round() as u32,
                y: adjusted.y.round() as u32,
                width: adjusted.width.round() as u32,
                height: adjusted.height.round() as u32,
            }),
            self.source_width,
            self.source_height,
        );
        cx.notify();
    }

    fn apply_dimensions(&mut self, cx: &mut Context<Self>) {
        let Some(width) = self
            .width_input
            .as_ref()
            .and_then(|v| v.read(cx).value().parse::<u32>().ok())
        else {
            self.status = "Enter a numeric width".into();
            return;
        };
        let Some(mut height) = self
            .height_input
            .as_ref()
            .and_then(|v| v.read(cx).value().parse::<u32>().ok())
        else {
            self.status = "Enter a numeric height".into();
            return;
        };
        let mut width = width.max(2) & !1;
        if self.aspect_locked {
            let crop = self.state.crop;
            let source_width = crop.map_or(self.source_width, |c| c.width);
            let source_height = crop.map_or(self.source_height, |c| c.height);
            height = ((width as f64 * source_height as f64 / source_width as f64).round() as u32)
                .max(2)
                & !1;
            if let Some(input) = &self.height_input {
                input.update(cx, |input, cx| {
                    *input = TextInput::new(height.to_string(), "Height", cx)
                });
            }
        }
        width = width.max(2);
        height = height.max(2) & !1;
        self.state.output_width = Some(width);
        self.state.output_height = Some(height);
        self.save_draft();
        self.status.clear();
        cx.notify();
    }
    fn key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "space" => self.toggle_play(cx),
            "left" => self.seek(
                self.playhead_ms
                    .saturating_sub(if e.keystroke.modifiers.shift {
                        1000
                    } else {
                        100
                    }),
                cx,
            ),
            "right" => self.seek(
                self.playhead_ms
                    .saturating_add(if e.keystroke.modifiers.shift {
                        1000
                    } else {
                        100
                    })
                    .min(self.state.duration_ms.saturating_sub(1)),
                cx,
            ),
            _ => {}
        }
    }
    fn button(&self, id: &'static str, label: impl IntoElement, t: Theme) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded_md()
            .bg(t.hover)
            .cursor_pointer()
            .child(label)
    }
}

impl Render for RecordingEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.close_hook_installed {
            self.close_hook_installed = true;
            let weak = cx.entity().downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                if let Some(entity) = weak.upgrade() {
                    let editor = entity.read(cx);
                    if !editor.launch.mock && editor.source_identity.is_some() {
                        let launch = editor.launch.clone();
                        let saved = editor.saved_path.is_some();
                        let path = editor
                            .saved_path
                            .clone()
                            .unwrap_or_else(|| editor.source.clone());
                        // Leave the native close callback before creating a
                        // new platform window (GPUI X11 still holds its client
                        // borrow while flushing ordinary App::defer effects).
                        cx.spawn(async move |cx| {
                            Timer::after(Duration::from_millis(16)).await;
                            let _ = cx.update(move |cx| {
                                if let Err(error) =
                                    super::show_ready_notice(launch, path, saved, cx)
                                {
                                    eprintln!("Could not show recording notice: {error:#}");
                                }
                            });
                        })
                        .detach();
                    }
                }
                true
            });
        }
        self.start_loading(cx);
        let t = Theme::for_window(&self.launch, window, cx);
        if self.loading || self.source_identity.is_none() {
            return div()
                .size_full()
                .bg(t.canvas)
                .text_color(t.muted)
                .font_family(theme::font())
                .text_size(px(13.))
                .flex()
                .items_center()
                .justify_center()
                .child(if self.loading {
                    "Preparing video preview, filmstrip, and waveform…".to_owned()
                } else {
                    self.status.clone()
                });
        }
        let compact = window.viewport_size().width <= px(820.);
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let width_input = self
            .width_input
            .get_or_insert_with(|| {
                cx.new(|cx| TextInput::new(self.source_width.to_string(), "Width", cx))
            })
            .clone();
        let height_input = self
            .height_input
            .get_or_insert_with(|| {
                cx.new(|cx| TextInput::new(self.source_height.to_string(), "Height", cx))
            })
            .clone();
        let poster = self.poster.clone();
        let frame = self.frame.clone();
        let duration = self.state.duration_ms as f32;
        let start = self.state.trim_start_ms as f32 / duration;
        let end = self.state.trim_end_ms as f32 / duration;
        let playhead = self.playhead_ms as f32 / duration;
        let progress = self.export_progress.load(Ordering::Acquire) as f32 / 10.;
        let frames = self.timeline_frames.clone();
        let waveform = self.waveform.clone();
        let crop = self.state.crop;
        let source_size = (self.source_width, self.source_height);
        let recorded_bounds = self.preview_bounds.clone();
        let paint_bounds = recorded_bounds.clone();
        div()
            .track_focus(&focus)
            .on_mouse_down(MouseButton::Left, move |_, window, _| focus.focus(window))
            .on_key_down(cx.listener(Self::key_down))
            .font_family(theme::font())
            .text_size(px(13.))
            .size_full()
            .bg(t.canvas)
            .text_color(t.text)
            .p_6()
            .relative()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_size(px(24.)).child(
                        if self.state.format == ExportFormat::Gif {
                            "Edit GIF"
                        } else {
                            "Edit recording"
                        },
                    ))
                    .child(div().text_color(t.muted).child(format!(
                        "{} × {} · {}",
                        self.source_width,
                        self.source_height,
                        if self.has_audio { "Audio" } else { "No audio" }
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(240.))
                    .rounded_lg()
                    .bg(t.field)
                    .flex()
                    .items_center()
                    .justify_center()
                    .relative()
                    .when_some(frame, |d, image| {
                        d.child(img(image).size_full().object_fit(ObjectFit::Contain))
                    })
                    .when(self.frame.is_none(), |d| {
                        d.when_some(poster, |d, p| {
                            d.child(img(p).size_full().object_fit(ObjectFit::Contain))
                        })
                    })
                    .when_some(crop, |d, crop| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(
                                    canvas(
                                        move |bounds, _, _| {
                                            recorded_bounds
                                                .set(fitted_video_bounds(bounds, source_size))
                                        },
                                        move |_, _, window, _| {
                                            let fitted = paint_bounds.get();
                                            let sx =
                                                f32::from(fitted.size.width) / source_size.0 as f32;
                                            let sy = f32::from(fitted.size.height)
                                                / source_size.1 as f32;
                                            let rect = Rect {
                                                x: crop.x as f32,
                                                y: crop.y as f32,
                                                width: crop.width as f32,
                                                height: crop.height as f32,
                                            };
                                            let crop_bounds = Bounds {
                                                origin: point(
                                                    fitted.origin.x + px(rect.x * sx),
                                                    fitted.origin.y + px(rect.y * sy),
                                                ),
                                                size: size(
                                                    px(rect.width * sx),
                                                    px(rect.height * sy),
                                                ),
                                            };
                                            window.paint_quad(quad(
                                                crop_bounds,
                                                Corners::default(),
                                                hsla(0., 0., 0., 0.),
                                                Edges::all(px(2.)),
                                                t.accent,
                                                BorderStyle::Solid,
                                            ));
                                            for handle in rect.handles() {
                                                let center = point(
                                                    fitted.origin.x + px(handle.0 * sx),
                                                    fitted.origin.y + px(handle.1 * sy),
                                                );
                                                window.paint_quad(quad(
                                                    Bounds {
                                                        origin: point(
                                                            center.x - px(5.),
                                                            center.y - px(5.),
                                                        ),
                                                        size: size(px(10.), px(10.)),
                                                    },
                                                    Corners::all(px(3.)),
                                                    t.raised,
                                                    Edges::all(px(2.)),
                                                    t.accent,
                                                    BorderStyle::Solid,
                                                ));
                                            }
                                        },
                                    )
                                    .size_full(),
                                )
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::crop_down))
                                .on_mouse_move(cx.listener(Self::crop_move))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|s, _, _, _| {
                                        s.crop_drag = None;
                                        s.save_draft();
                                    }),
                                )
                                .on_mouse_up_out(
                                    MouseButton::Left,
                                    cx.listener(|s, _, _, _| {
                                        s.crop_drag = None;
                                        s.save_draft();
                                    }),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(format!(
                        "{:.2}s – {:.2}s",
                        self.state.trim_start_ms as f64 / 1000.,
                        self.state.trim_end_ms as f64 / 1000.
                    ))
                    .child(format!(
                        "{:.2}s selected",
                        (self.state.trim_end_ms - self.state.trim_start_ms) as f64 / 1000.
                    )),
            )
            .child(
                div()
                    .id("timeline")
                    .relative()
                    .h(px(76.))
                    .rounded_md()
                    .bg(t.field)
                    .cursor_pointer()
                    .child(
                        div()
                            .absolute()
                            .inset_1()
                            .flex()
                            .overflow_hidden()
                            .children(frames.into_iter().map(|frame| {
                                img(frame).flex_1().h_full().object_fit(ObjectFit::Cover)
                            })),
                    )
                    .when_some(waveform, |d, waveform| {
                        d.child(
                            img(waveform)
                                .absolute()
                                .left_1()
                                .right_1()
                                .bottom_1()
                                .h(px(24.))
                                .opacity(0.72),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(start))
                            .right(relative(1. - end))
                            .bg(t.accent)
                            .opacity(0.25),
                    )
                    .child(
                        self.button(
                            "crop",
                            if crop.is_some() { "Reset crop" } else { "Crop" },
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            if s.state.crop.is_some() {
                                s.state.set_crop(None, s.source_width, s.source_height);
                            } else {
                                s.state.set_crop(
                                    Some(CropRect {
                                        x: s.source_width / 10,
                                        y: s.source_height / 10,
                                        width: s.source_width * 4 / 5,
                                        height: s.source_height * 4 / 5,
                                    }),
                                    s.source_width,
                                    s.source_height,
                                );
                            }
                            s.save_draft();
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(start))
                            .w(px(5.))
                            .bg(t.accent),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(end))
                            .w(px(5.))
                            .bg(t.accent),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(playhead))
                            .w(px(2.))
                            .bg(t.text),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::timeline_down))
                    .on_mouse_move(cx.listener(Self::timeline_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|s, _, _, _| s.drag = None))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|s, _, _, _| s.drag = None)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        self.button("play", if self.playing { "Pause" } else { "Play" }, t)
                            .on_click(cx.listener(|s, _, _, cx| s.toggle_play(cx))),
                    )
                    .child(
                        self.button(
                            "format",
                            match self.state.format {
                                ExportFormat::Gif => "GIF",
                                ExportFormat::Mp4 => "MP4",
                                ExportFormat::WebM => "WebM",
                            },
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.state.format = match s.state.format {
                                ExportFormat::Mp4 => ExportFormat::WebM,
                                ExportFormat::WebM => ExportFormat::Gif,
                                ExportFormat::Gif => ExportFormat::Mp4,
                            };
                            s.save_draft();
                            cx.notify();
                        })),
                    )
                    .child(
                        self.button(
                            "quality",
                            if self.state.quality == QualityPreset::Preserve {
                                "Original quality"
                            } else {
                                "High quality"
                            },
                            t,
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.state.quality = if s.state.quality == QualityPreset::Preserve {
                                QualityPreset::High
                            } else {
                                QualityPreset::Preserve
                            };
                            cx.notify();
                        })),
                    )
                    .when(self.has_audio, |d| {
                        d.child(
                            self.button(
                                "audio",
                                if self.state.mute_system_audio {
                                    "Audio muted"
                                } else {
                                    "Include audio"
                                },
                                t,
                            )
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.state.mute_system_audio = !s.state.mute_system_audio;
                                if s.playing {
                                    s.seek(s.playhead_ms, cx);
                                }
                                s.save_draft();
                                cx.notify()
                            })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        self.button("rate", format!("{:.2}×", self.state.playback_rate), t)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.state.playback_rate = match s.state.playback_rate {
                                    x if x < 0.75 => 1.,
                                    x if x < 1.25 => 1.5,
                                    x if x < 1.75 => 2.,
                                    _ => 0.5,
                                };
                                if s.playing {
                                    s.seek(s.playhead_ms, cx);
                                }
                                s.save_draft();
                                cx.notify();
                            })),
                    )
                    .when(self.exporting, |d| {
                        d.child(self.button("cancel", "Cancel", t).on_click(cx.listener(
                            |s, _, _, cx| {
                                if let Some(c) = &s.export_cancel {
                                    c.cancel()
                                }
                                s.status = "Cancelling export…".into();
                                cx.notify()
                            },
                        )))
                    })
                    .child(
                        self.button(
                            "export",
                            if self.exporting {
                                format!("Exporting {progress:.0}%")
                            } else {
                                "Export".into()
                            },
                            t,
                        )
                        .bg(t.accent)
                        .text_color(gpui::black())
                        .on_click(cx.listener(Self::export)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .when(compact, |d| d.flex_col())
                    .gap_4()
                    .children([
                        div()
                            .flex_1()
                            .p_4()
                            .rounded_lg()
                            .bg(t.field)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Crop & size")
                            .child(
                                crop.map(|c| {
                                    format!("X {}  Y {}  {} × {}", c.x, c.y, c.width, c.height)
                                })
                                .unwrap_or_else(|| "Full frame".into()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(self.button("crop-16-9", "16:9", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            let h = s.source_height * 4 / 5;
                                            let w = (h * 16 / 9).min(s.source_width);
                                            s.state.set_crop(
                                                Some(CropRect {
                                                    x: (s.source_width - w) / 2,
                                                    y: (s.source_height - h) / 2,
                                                    width: w,
                                                    height: h,
                                                }),
                                                s.source_width,
                                                s.source_height,
                                            );
                                            s.save_draft();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.button("crop-square", "1:1", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            let side = s.source_width.min(s.source_height) * 4 / 5;
                                            s.state.set_crop(
                                                Some(CropRect {
                                                    x: (s.source_width - side) / 2,
                                                    y: (s.source_height - side) / 2,
                                                    width: side,
                                                    height: side,
                                                }),
                                                s.source_width,
                                                s.source_height,
                                            );
                                            s.save_draft();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.button("crop-reset", "Reset", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            s.state.crop = None;
                                            s.save_draft();
                                            cx.notify();
                                        }),
                                    )),
                            )
                            .child(format!(
                                "Output {} × {}",
                                self.state
                                    .output_width
                                    .unwrap_or(crop.map_or(self.source_width, |c| c.width)),
                                self.state
                                    .output_height
                                    .unwrap_or(crop.map_or(self.source_height, |c| c.height))
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .child(width_input)
                                    .child("×")
                                    .child(height_input)
                                    .child(
                                        self.button(
                                            "aspect-lock",
                                            if self.aspect_locked {
                                                "Ratio locked"
                                            } else {
                                                "Free ratio"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.aspect_locked = !s.aspect_locked;
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(self.button("apply-size", "Apply", t).on_click(
                                        cx.listener(|s, _, _, cx| s.apply_dimensions(cx)),
                                    )),
                            )
                            .into_any_element(),
                        div()
                            .flex_1()
                            .p_4()
                            .rounded_lg()
                            .bg(t.field)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Save quality")
                            .child(format!(
                                "{} · {:?}",
                                match self.state.format {
                                    ExportFormat::Mp4 => "MP4",
                                    ExportFormat::Gif => "GIF",
                                    ExportFormat::WebM => "WebM",
                                },
                                self.state.quality
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(self.button("quality-preserve", "Preserve", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            s.state.quality = QualityPreset::Preserve;
                                            s.save_draft();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.button("quality-high", "High", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            s.state.quality = QualityPreset::High;
                                            s.save_draft();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.button("quality-small", "Small", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            s.state.quality = QualityPreset::Small;
                                            s.save_draft();
                                            cx.notify();
                                        }),
                                    )),
                            )
                            .child(
                                self.button(
                                    "max-size",
                                    self.state
                                        .max_size_bytes
                                        .map(|v| format!("Maximum {:.0} MB", v as f64 / 1_000_000.))
                                        .unwrap_or_else(|| "No maximum size".into()),
                                    t,
                                )
                                .on_click(cx.listener(
                                    |s, _, _, cx| {
                                        s.state.max_size_bytes = if s.state.max_size_bytes.is_some()
                                        {
                                            None
                                        } else {
                                            Some(10_000_000)
                                        };
                                        s.save_draft();
                                        cx.notify();
                                    },
                                )),
                            )
                            .when(self.state.format == ExportFormat::Gif, |d| {
                                d.child(format!(
                                    "GIF {} FPS · {} colors",
                                    self.state.gif_fps, self.state.gif_colors
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(self.button("gif-fps", "Frame rate", t).on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.state.gif_fps = match s.state.gif_fps {
                                                    8 => 15,
                                                    15 => 24,
                                                    24 => 30,
                                                    _ => 8,
                                                };
                                                s.save_draft();
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.button("gif-palette", "Palette", t).on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.state.gif_colors = match s.state.gif_colors {
                                                    32 => 64,
                                                    64 => 128,
                                                    128 => 256,
                                                    _ => 32,
                                                };
                                                s.save_draft();
                                                cx.notify();
                                            }),
                                        )),
                                )
                            })
                            .into_any_element(),
                        div()
                            .flex_1()
                            .p_4()
                            .rounded_lg()
                            .bg(t.field)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Audio & destination")
                            .child(format!(
                                "System {:.0}% · Microphone {:.0}%",
                                self.state.system_volume * 100.,
                                self.state.microphone_volume * 100.
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(self.button("system-volume", "System +25%", t).on_click(
                                        cx.listener(|s, _, _, cx| {
                                            s.state.system_volume =
                                                (s.state.system_volume + 0.25) % 2.25;
                                            if s.playing {
                                                s.seek(s.playhead_ms, cx)
                                            }
                                            s.save_draft();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        self.button("microphone-volume", "Mic +25%", t).on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.state.microphone_volume =
                                                    (s.state.microphone_volume + 0.25) % 2.25;
                                                if s.playing {
                                                    s.seek(s.playhead_ms, cx)
                                                }
                                                s.save_draft();
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        self.button(
                                            "microphone-mute",
                                            if self.state.mute_microphone {
                                                "Mic muted"
                                            } else {
                                                "Mic included"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.state.mute_microphone = !s.state.mute_microphone;
                                                if s.playing {
                                                    s.seek(s.playhead_ms, cx)
                                                }
                                                s.save_draft();
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        self.button(
                                            "mono",
                                            if self.state.mono_audio {
                                                "Mono"
                                            } else {
                                                "Stereo"
                                            },
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(|s, _, _, cx| {
                                                s.state.mono_audio = !s.state.mono_audio;
                                                if s.playing {
                                                    s.seek(s.playhead_ms, cx)
                                                }
                                                s.save_draft();
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(format!("Saving to {}", self.destination.display()))
                            .child(
                                self.button("choose-destination", "Choose save location…", t)
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        let receiver = cx.prompt_for_paths(PathPromptOptions {
                                            files: false,
                                            directories: true,
                                            multiple: false,
                                            prompt: Some("Choose save location".into()),
                                        });
                                        cx.spawn_in(window, async move |this, cx| {
                                            if let Ok(Ok(Some(paths))) = receiver.await {
                                                let _ = this.update(cx, |s, cx| {
                                                    if let Some(path) = paths.first() {
                                                        s.destination = path.clone();
                                                        s.status.clear();
                                                        cx.notify();
                                                    }
                                                });
                                            }
                                        })
                                        .detach();
                                    })),
                            )
                            .into_any_element(),
                    ]),
            )
            .child(div().text_color(t.muted).child(self.status.clone()))
    }
}

impl Drop for RecordingEditor {
    fn drop(&mut self) {
        if let Some(cancel) = &self.export_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &self.load_cancel {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use std::io::Write;

    #[test]
    fn timeline_boundaries_are_asymmetric_and_clamped() {
        assert_eq!(timeline_value(24., 1048., 10_000), 0);
        assert_eq!(timeline_value(1048., 1048., 10_000), 10_000);
        assert_eq!(timeline_value(280., 1048., 10_000), 2_560);
        assert_eq!(timeline_value(-50., 1048., 10_000), 0);
    }

    #[test]
    fn source_cache_separates_equal_duration_paths_and_changed_content() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.mp4");
        let second = dir.path().join("second.mp4");
        std::fs::write(&first, b"same-duration-source-a").unwrap();
        std::fs::write(&second, b"same-duration-source-b").unwrap();
        let first_key = source_cache_key(&first).unwrap();
        assert_ne!(first_key, source_cache_key(&second).unwrap());

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&first)
            .unwrap();
        file.write_all(b"changed-source-bytes!").unwrap();
        file.sync_all().unwrap();
        assert_ne!(first_key, source_cache_key(&first).unwrap());
    }

    #[test]
    fn background_results_are_fenced_by_request_and_source() {
        let first = Path::new("first.mp4");
        let second = Path::new("second.mp4");
        assert!(loading_result_matches(7, 7, first, first));
        assert!(!loading_result_matches(8, 7, first, first));
        assert!(!loading_result_matches(7, 7, second, first));
    }

    #[test]
    fn reads_editor_destination_from_shared_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"output_directory":"/tmp/captures-output"}"#,
        )
        .unwrap();
        assert_eq!(
            configured_output_directory(dir.path()),
            Some(PathBuf::from("/tmp/captures-output"))
        );
    }

    #[test]
    fn letterboxed_preview_maps_to_source_pixels_not_container_percentages() {
        let fitted = fitted_video_bounds(
            Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1000.), px(1000.)),
            },
            (1920, 1080),
        );
        assert_eq!(fitted.origin.y, px(218.75));
        let point = source_point(point(px(250.), px(359.375)), fitted, (1920, 1080));
        assert!((point.0 - 480.).abs() < 0.01 && (point.1 - 270.).abs() < 0.01);
        assert_ne!(
            (point.0.round(), point.1.round()),
            (480., 388.),
            "container-percentage interpretation must fail"
        );
    }

    #[test]
    fn named_audio_tracks_are_exported_independently() {
        let probe = serde_json::json!({"streams": [{"tags": {"title": "System Audio"}}, {"tags": {"title": "Microphone"}}]});
        assert_eq!(
            audio_tracks_from_probe(&probe),
            AudioTracks {
                system: true,
                microphone: true
            }
        );
        let wrong = serde_json::json!({"streams": [{"tags": {"title": "Microphone"}}]});
        assert_eq!(
            audio_tracks_from_probe(&wrong),
            AudioTracks {
                system: false,
                microphone: true
            }
        );
        assert_ne!(
            audio_tracks_from_probe(&wrong),
            AudioTracks {
                system: true,
                microphone: false
            }
        );
    }

    #[test]
    fn probes_native_sixty_fps_timing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("sixty.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=94x58:rate=60:duration=0.4",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        assert!((source_frame_rate(&source) - 60.).abs() < 0.01);
        let prepared = prepare_editor(
            &source,
            &dir.path().join("profile"),
            &CancelToken::default(),
        )
        .unwrap();
        assert_eq!(prepared.timeline_frames.len(), 12);
        for path in &prepared.timeline_frames {
            // Seeking to duration minus 1 ms returns no final video frame;
            // every thumbnail must be a real image, including the right edge.
            assert_eq!(image::image_dimensions(path.as_ref()).unwrap(), (94, 58));
        }
    }
}
