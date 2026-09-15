#[path = "playback.rs"]
mod playback;
#[path = "quality.rs"]
mod quality;

use self::playback::{AudioPlayer, VideoDecoder};
use super::model::{EditorState, Rect};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AudioSlider {
    System,
    Microphone,
}

fn bounded_percent(value: f32) -> f32 {
    value.clamp(0., 200.)
}

fn parse_size_limit(value: &str, multiplier: u64) -> Option<u64> {
    let value = value.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.).then(|| {
        (value * multiplier as f64)
            .round()
            .clamp(1., u64::MAX as f64) as u64
    })
}

fn trim_keyboard_delta(key: &str, duration_ms: u64) -> Option<i64> {
    let step = if duration_ms < 60_000 { 1 } else { 10 };
    match key {
        "left" | "down" => Some(-step),
        "right" | "up" => Some(step),
        "pagedown" => Some(-1_000),
        "pageup" => Some(1_000),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct CropDrag {
    handle: usize,
    start: (f32, f32),
    initial: Rect,
}

fn timeline_value(x: f32, track_width: f32, duration_ms: u64) -> u64 {
    let usable = (track_width - 8.).max(1.);
    (((x - 4.) / usable).clamp(0., 1.) * duration_ms as f32).round() as u64
}

fn editor_time(milliseconds: u64) -> String {
    let minutes = milliseconds / 60_000;
    let seconds = (milliseconds / 1_000) % 60;
    let millis = milliseconds % 1_000;
    format!("{minutes}:{seconds:02}.{millis:03}")
}

fn video_export_path(
    directory: &Path,
    stem: &str,
    extension: &str,
) -> Result<PathBuf, &'static str> {
    let stem = stem.trim();
    if stem.is_empty() || matches!(stem, "." | "..") || stem.contains(['/', '\\', '\0']) {
        return Err("Enter a filename without a folder path");
    }
    Ok(directory.join(format!("{stem}.{extension}")))
}

fn loading_result_matches(
    current_request: u64,
    result_request: u64,
    current_source: &Path,
    result_source: &Path,
) -> bool {
    current_request == result_request && current_source == result_source
}

#[derive(Clone, Copy, PartialEq)]
enum NumberField {
    X,
    Y,
    Width,
    Height,
    OutputWidth,
    OutputHeight,
}

impl NumberField {
    const ALL: [Self; 6] = [
        Self::X,
        Self::Y,
        Self::Width,
        Self::Height,
        Self::OutputWidth,
        Self::OutputHeight,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Width | Self::OutputWidth => "Width",
            Self::Height | Self::OutputHeight => "Height",
        }
    }

    fn is_crop(self) -> bool {
        !matches!(self, Self::OutputWidth | Self::OutputHeight)
    }
}

/// Matches App.tsx's numeric crop changes: translating never resizes, and a
/// locked dimension fits both bounds before rounding to source pixels.
fn crop_number(
    mut crop: CropRect,
    field: NumberField,
    value: f64,
    bounds: (u32, u32),
    locked: bool,
) -> CropRect {
    let max_w = bounds.0.saturating_sub(crop.x).max(2);
    let max_h = bounds.1.saturating_sub(crop.y).max(2);
    let ratio = crop.width as f64 / crop.height.max(1) as f64;
    match field {
        NumberField::X => {
            crop.x = value
                .round()
                .clamp(0., bounds.0.saturating_sub(crop.width) as f64) as u32
        }
        NumberField::Y => {
            crop.y = value
                .round()
                .clamp(0., bounds.1.saturating_sub(crop.height) as f64) as u32
        }
        NumberField::Width => {
            crop.width = value.round().clamp(2., max_w as f64) as u32;
            if locked {
                crop.height = (crop.width as f64 / ratio).round().max(2.) as u32;
                if crop.height > max_h {
                    crop.height = max_h;
                    crop.width = (max_h as f64 * ratio).round().max(2.) as u32;
                }
            }
        }
        NumberField::Height => {
            crop.height = value.round().clamp(2., max_h as f64) as u32;
            if locked {
                crop.width = (crop.height as f64 * ratio).round().max(2.) as u32;
                if crop.width > max_w {
                    crop.width = max_w;
                    crop.height = (max_w as f64 / ratio).round().max(2.) as u32;
                }
            }
        }
        _ => unreachable!("output sizes do not edit the source crop"),
    }
    crop
}

/// Port of editorCropAfterDrag's aspect-locked path. The handle order is the
/// shared Rect::handles order (NW, N, NE, E, SE, S, SW, W, move).
fn crop_drag_rect(
    initial: Rect,
    handle: usize,
    delta: (f32, f32),
    bounds: (f32, f32),
    locked: bool,
) -> Rect {
    if !locked || handle == 8 {
        return initial.adjusted(handle, delta, bounds);
    }
    let west = matches!(handle, 0 | 6 | 7);
    let east = matches!(handle, 2..=4);
    let north = matches!(handle, 0..=2);
    let south = matches!(handle, 4..=6);
    let ratio = initial.width / initial.height.max(1.);
    let mut width = (initial.width
        + if west {
            -delta.0
        } else if east {
            delta.0
        } else {
            0.
        })
    .max(2.);
    let mut height = (initial.height
        + if north {
            -delta.1
        } else if south {
            delta.1
        } else {
            0.
        })
    .max(2.);
    if (west || east) && (north || south) {
        if (width - initial.width).abs() / initial.width
            >= (height - initial.height).abs() / initial.height
        {
            height = width / ratio;
        } else {
            width = height * ratio;
        }
    } else if west || east {
        height = width / ratio;
    } else {
        width = height * ratio;
    }
    let anchor_x = initial.x
        + if west {
            initial.width
        } else if east {
            0.
        } else {
            initial.width / 2.
        };
    let anchor_y = initial.y
        + if north {
            initial.height
        } else if south {
            0.
        } else {
            initial.height / 2.
        };
    let max_w = if west {
        anchor_x
    } else if east {
        bounds.0 - anchor_x
    } else {
        2. * anchor_x.min(bounds.0 - anchor_x)
    };
    let max_h = if north {
        anchor_y
    } else if south {
        bounds.1 - anchor_y
    } else {
        2. * anchor_y.min(bounds.1 - anchor_y)
    };
    let fit = 1_f32.min(max_w / width).min(max_h / height);
    width = (width * fit).max(2.);
    height = (height * fit).max(2.);
    Rect {
        x: (anchor_x
            - if west {
                width
            } else if east {
                0.
            } else {
                width / 2.
            })
        .clamp(0., (bounds.0 - width).max(0.))
        .round(),
        y: (anchor_y
            - if north {
                height
            } else if south {
                0.
            } else {
                height / 2.
            })
        .clamp(0., (bounds.1 - height).max(0.))
        .round(),
        width: width.round(),
        height: height.round(),
    }
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
    timeline_bounds: Rc<Cell<Bounds<Pixels>>>,
    crop_drag: Option<CropDrag>,
    aspect_locked: bool,
    crop_value: Option<CropRect>,
    number_inputs: [Option<Entity<TextInput>>; 6],
    filename_input: Option<Entity<TextInput>>,
    format_open: bool,
    choice_open: Option<&'static str>,
    choice_values: Vec<&'static str>,
    choice_index: usize,
    resolution: &'static str,
    make_copy: bool,
    destination: PathBuf,
    saved_path: Option<PathBuf>,
    close_hook_installed: bool,
    preview_loop: bool,
    preview_actual_size: bool,
    playing: bool,
    playhead_ms: u64,
    clock_started: Option<(Instant, u64)>,
    tick_generation: u64,
    pending_frame: bool,
    drag: Option<TimelineDrag>,
    audio_slider_drag: Option<AudioSlider>,
    trim_start_focus: Option<FocusHandle>,
    trim_end_focus: Option<FocusHandle>,
    maximum_size_input: Option<Entity<TextInput>>,
    maximum_size_unit: u64,
    quality_fingerprint: Vec<u8>,
    quality_request: u64,
    quality_cancel: Option<CancelToken>,
    quality_pending: bool,
    quality_error: Option<String>,
    estimated_size: Option<(u64, bool)>,
    comparison: Option<(Arc<RenderImage>, Arc<RenderImage>)>,
    comparison_dismissed: bool,
    comparison_split: f32,
    comparison_drag: bool,
    comparison_bounds: Rc<Cell<Bounds<Pixels>>>,
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
            timeline_bounds: Rc::new(Cell::new(Bounds::default())),
            crop_drag: None,
            aspect_locked: true,
            crop_value: None,
            number_inputs: Default::default(),
            filename_input: None,
            format_open: false,
            choice_open: None,
            choice_values: Vec::new(),
            choice_index: 0,
            resolution: "original",
            make_copy: true,
            destination,
            saved_path,
            close_hook_installed: false,
            preview_loop: false,
            preview_actual_size: false,
            playing: false,
            playhead_ms: 0,
            clock_started: None,
            tick_generation: 0,
            pending_frame: false,
            drag: None,
            audio_slider_drag: None,
            trim_start_focus: None,
            trim_end_focus: None,
            maximum_size_input: None,
            maximum_size_unit: 1_000_000,
            quality_fingerprint: Vec::new(),
            quality_request: 0,
            quality_cancel: None,
            quality_pending: false,
            quality_error: None,
            estimated_size: None,
            comparison: None,
            comparison_dismissed: false,
            comparison_split: 0.5,
            comparison_drag: false,
            comparison_bounds: Rc::new(Cell::new(Bounds::default())),
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
        crate::preferences::input::bind_keys(cx);
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
                        s.resolution = if s.state.output_width.is_some() {
                            "custom"
                        } else {
                            "original"
                        };
                        s.has_audio = prepared.has_audio;
                        s.audio_tracks = prepared.audio_tracks;
                        s.status.clear();
                        s.crop_value = s.state.crop;
                        s.number_inputs = Default::default();
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
                            if s.preview_loop {
                                s.seek(s.state.trim_start_ms, cx);
                                return false;
                            } else {
                                s.playing = false;
                                s.clock_started = None;
                                s.decoder.stop();
                                s.audio.stop();
                                cx.notify();
                                return false;
                            }
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
        if self.state.max_size_bytes.is_some() {
            let value = self.maximum_size_input.as_ref().and_then(|input| {
                parse_size_limit(&input.read(cx).value(), self.maximum_size_unit)
            });
            let Some(bytes) = value else {
                self.status = "Enter a positive maximum file size".into();
                cx.notify();
                return;
            };
            self.state.max_size_bytes = Some(bytes);
        }
        let ext = if self.state.format == ExportFormat::Gif {
            "gif"
        } else if self.state.format == ExportFormat::WebM {
            "webm"
        } else {
            "mp4"
        };
        let dir = self.destination.clone();
        let name = self
            .filename_input
            .as_ref()
            .map(|v| v.read(cx).value())
            .unwrap_or_default();
        let destination = match video_export_path(&dir, &name, ext) {
            Ok(path) => path,
            Err(error) => {
                self.status = error.into();
                cx.notify();
                return;
            }
        };
        let make_copy = self.make_copy;
        let source = self.source.clone();
        let profile = self.launch.profile.clone();
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
            // Encode away from both source and final destination. Only a successful,
            // uncancelled export may replace an explicitly chosen existing file.
            let staging = tempfile::Builder::new()
                .prefix(".captures-export-")
                .tempdir_in(&dir)?;
            // The media toolchain refuses existing destinations, even empty
            // placeholders. Reserve a directory, not an output file.
            let staged =
                tempfile::TempPath::try_from_path(staging.path().join(format!("output.{ext}")))?;
            MediaToolchain::from_command_names()
                .export(&source, &staged, &edit, &export, &cancel, |p| {
                    progress.store(p.completed_per_mille as u64, Ordering::Release);
                })
                .map_err(anyhow::Error::from)?;
            anyhow::ensure!(!cancel.is_cancelled(), "Save cancelled");
            if make_copy {
                staged.persist_noclobber(&destination)?;
            } else {
                staged.persist(&destination)?;
            }
            let history_error = crate::preferences::history::record_export(
                &profile,
                Some(&source),
                &destination,
                make_copy,
            )
            .err()
            .map(|e| e.to_string());
            Ok::<_, anyhow::Error>((destination, history_error))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.exporting = false;
                s.export_cancel = None;
                s.status = match result {
                    Ok((path, history_error)) => {
                        s.saved_path = Some(path.clone());
                        match history_error {
                            Some(error) => {
                                format!("Saved {}; history copy failed: {error}", path.display())
                            }
                            None => format!("Saved {}", path.display()),
                        }
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

    fn can_compare(&self) -> bool {
        self.state.format != ExportFormat::WebM
            && (self.state.quality != QualityPreset::Preserve
                || self.state.max_size_bytes.is_some())
    }

    fn request_quality(&mut self, cx: &mut Context<Self>) {
        let mut edit = self.state.edit(self.has_audio);
        edit.audio.source_has_system_audio = self.audio_tracks.system;
        edit.audio.source_has_microphone_audio = self.audio_tracks.microphone;
        let export = self.state.export();
        let at = (self.can_compare() && !self.playing && !self.comparison_dismissed)
            .then_some(self.playhead_ms);
        let fingerprint =
            serde_json::to_vec(&(&edit, &export, at)).expect("serializable recording specs");
        if fingerprint == self.quality_fingerprint {
            return;
        }
        self.quality_fingerprint = fingerprint;
        self.quality_request += 1;
        let request = self.quality_request;
        if let Some(cancel) = self.quality_cancel.take() {
            cancel.cancel();
        }
        self.quality_pending = self.state.format != ExportFormat::WebM;
        self.quality_error = None;
        self.estimated_size = None;
        if !self.quality_pending {
            self.comparison = None;
            return;
        }
        let source = self.source.clone();
        let cancel = CancelToken::default();
        self.quality_cancel = Some(cancel.clone());
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(350)).await;
            if cancel.is_cancelled() {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move { quality::render(&source, &edit, &export, at, &cancel) })
                .await;
            let _ = this.update(cx, |s, cx| {
                if s.quality_request != request {
                    return;
                }
                s.quality_pending = false;
                s.quality_cancel = None;
                match result {
                    Ok(result) => {
                        s.estimated_size = result.estimate;
                        s.comparison = result
                            .frames
                            .map(|(before, after)| (quality_image(before), quality_image(after)));
                    }
                    Err(error) => {
                        s.comparison = None;
                        s.quality_error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn compression_overlay(&self, cx: &Context<Self>) -> Stateful<Div> {
        let bounds = self.comparison_bounds.clone();
        let frames = self.comparison.clone();
        let split = self.comparison_split;
        let pending = self.quality_pending;
        let before_bytes = std::fs::metadata(&self.source)
            .ok()
            .map(|metadata| metadata.len());
        let t = Theme::for_media(cx);
        let label = |text: String| {
            div()
                .px(px(8.))
                .py(px(4.))
                .rounded_full()
                .border_1()
                .border_color(t.glass_border)
                .bg(t.glass)
                .text_color(t.glass_text)
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .child(text)
        };
        div()
            .id("recording-comparison")
            .absolute()
            .inset_0()
            .occlude()
            .child(
                canvas(
                    move |area, _, _| bounds.set(area),
                    move |area, _, window, _| {
                        if let Some((before, after)) = &frames {
                            let _ = window.paint_image(
                                area,
                                Corners::default(),
                                before.clone(),
                                0,
                                false,
                            );
                            let right = Bounds::new(
                                point(area.left() + area.size.width * split, area.top()),
                                size(area.size.width * (1. - split), area.size.height),
                            );
                            window.with_content_mask(
                                Some(ContentMask { bounds: right }),
                                |window| {
                                    let _ = window.paint_image(
                                        area,
                                        Corners::default(),
                                        after.clone(),
                                        0,
                                        false,
                                    );
                                },
                            );
                        }
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .bottom(px(12.))
                    .child(label(format!(
                        "Before{}",
                        before_bytes
                            .map(|bytes| format!(" · {}", file_size(bytes)))
                            .unwrap_or_default()
                    ))),
            )
            .child(
                div()
                    .absolute()
                    .right(px(12.))
                    .bottom(px(12.))
                    .child(label(format!(
                        "After{}",
                        if pending {
                            " · Processing…".into()
                        } else {
                            self.estimated_size
                                .map(|(bytes, _)| format!(" · {}", file_size(bytes)))
                                .unwrap_or_default()
                        }
                    ))),
            )
            .when(!pending && self.comparison.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(split))
                        .ml(px(-1.))
                        .w(px(2.))
                        .bg(t.glass_text),
                )
                .child(
                    div()
                        .absolute()
                        .left(relative(split))
                        .top(relative(0.5))
                        .ml(px(-18.))
                        .mt(px(-18.))
                        .size(px(36.))
                        .rounded_full()
                        .bg(t.glass)
                        .border_1()
                        .border_color(t.glass_border)
                        .text_color(t.glass_text)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("↔"),
                )
            })
            .when(pending || self.quality_error.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(label(
                            self.quality_error
                                .clone()
                                .unwrap_or_else(|| "Preparing preview…".into()),
                        )),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|s, e: &MouseDownEvent, _, cx| {
                    if !s.quality_pending {
                        s.comparison_drag = true;
                        s.set_comparison_split(e.position.x);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|s, e: &MouseMoveEvent, _, cx| {
                if s.comparison_drag {
                    s.set_comparison_split(e.position.x);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.comparison_drag = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.comparison_drag = false),
            )
            .child(
                div()
                    .id("dismiss-comparison")
                    .absolute()
                    .right(px(12.))
                    .top(px(12.))
                    .cursor_pointer()
                    .child(label("Hide".into()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.comparison_dismissed = true;
                        cx.notify();
                    })),
            )
    }

    fn set_comparison_split(&mut self, x: Pixels) {
        let bounds = self.comparison_bounds.get();
        self.comparison_split =
            ((x - bounds.left()) / bounds.size.width.max(px(1.))).clamp(0.06, 0.94);
    }

    fn save_draft(&self) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.state) {
            let temporary = self.draft_path.with_extension("json.tmp");
            if std::fs::write(&temporary, bytes).is_ok() {
                let _ = std::fs::rename(temporary, &self.draft_path);
            }
        }
    }

    fn timeline_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let bounds = self.timeline_bounds.get();
        let width = f32::from(bounds.size.width);
        let x = f32::from(e.position.x - bounds.origin.x);
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
    fn timeline_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            let bounds = self.timeline_bounds.get();
            self.timeline_update(
                f32::from(e.position.x - bounds.origin.x),
                f32::from(bounds.size.width),
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
        let adjusted = crop_drag_rect(
            drag.initial,
            drag.handle,
            (point.0 - drag.start.0, point.1 - drag.start.1),
            (self.source_width as f32, self.source_height as f32),
            self.aspect_locked,
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
        self.crop_value = self.state.crop;
        self.update_resolution();
        self.sync_numbers(None, cx);
        cx.notify();
    }

    fn current_crop(&self) -> CropRect {
        self.crop_value.unwrap_or(CropRect {
            x: 0,
            y: 0,
            width: self.source_width,
            height: self.source_height,
        })
    }

    fn number_value(&self, field: NumberField) -> u32 {
        let crop = self.current_crop();
        match field {
            NumberField::X => crop.x,
            NumberField::Y => crop.y,
            NumberField::Width => crop.width,
            NumberField::Height => crop.height,
            NumberField::OutputWidth => self
                .state
                .output_width
                .unwrap_or(self.state.crop.map_or(self.source_width, |c| c.width)),
            NumberField::OutputHeight => self
                .state
                .output_height
                .unwrap_or(self.state.crop.map_or(self.source_height, |c| c.height)),
        }
    }

    fn sync_numbers(&self, preserve: Option<NumberField>, cx: &mut Context<Self>) {
        for field in NumberField::ALL {
            if preserve == Some(field) {
                continue;
            }
            if let Some(input) = &self.number_inputs[field as usize] {
                let value = self.number_value(field).to_string();
                if input.read(cx).value() != value {
                    input.update(cx, |input, cx| input.set_value(value, cx));
                }
            }
        }
    }

    fn update_resolution(&mut self) {
        if self.resolution == "custom" {
            return;
        }
        if self.resolution == "original" {
            self.state.output_width = None;
            self.state.output_height = None;
        } else {
            let width = self.state.crop.map_or(self.source_width, |c| c.width);
            let height = self.state.crop.map_or(self.source_height, |c| c.height);
            let height_out = height
                .min(if self.resolution == "720" { 720 } else { 1080 })
                .max(2);
            self.state.output_width = Some(
                (width as f64 * height_out as f64 / height as f64)
                    .round()
                    .max(2.) as u32,
            );
            self.state.output_height = Some(height_out);
        }
    }

    fn change_number(
        &mut self,
        field: NumberField,
        value: f64,
        typing: bool,
        cx: &mut Context<Self>,
    ) {
        if !value.is_finite() || (field.is_crop() && self.state.crop.is_none()) {
            return;
        }
        if field.is_crop() {
            let crop = crop_number(
                self.current_crop(),
                field,
                value,
                (self.source_width, self.source_height),
                self.aspect_locked,
            );
            self.crop_value = Some(crop);
            self.state
                .set_crop(Some(crop), self.source_width, self.source_height);
            self.update_resolution();
        } else {
            // Output width and height are independent in the source interface;
            // its aspect checkbox belongs to crop, not custom output size.
            let value = value.round().clamp(2., 16_384.) as u32;
            if field == NumberField::OutputWidth {
                self.state.output_width = Some(value);
            } else {
                self.state.output_height = Some(value);
            }
        }
        self.sync_numbers(typing.then_some(field), cx);
        self.save_draft();
        self.status.clear();
        cx.notify();
    }

    fn number_field(&self, field: NumberField, t: Theme, cx: &Context<Self>) -> Div {
        let disabled = field.is_crop() && self.state.crop.is_none();
        let input = self.number_inputs[field as usize].as_ref().unwrap().clone();
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(px(11.))
            .text_color(t.subtle)
            .opacity(if disabled { 0.5 } else { 1. })
            .child(field.label())
            .child(
                div()
                    .id(("recording-number", field as usize))
                    .h(px(34.))
                    .flex()
                    .border_1()
                    .border_color(t.border)
                    .rounded(px(7.))
                    .bg(t.field)
                    .overflow_hidden()
                    .child(div().min_w_0().flex_1().child(input))
                    .child(
                        div()
                            .w(px(26.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(t.border)
                            .children([(1., "⌃"), (-1., "⌄")].into_iter().map(|(step, icon)| {
                                div()
                                    .id((
                                        SharedString::from(format!(
                                            "recording-step-{}",
                                            field as usize
                                        )),
                                        (step > 0.) as usize,
                                    ))
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(11.))
                                    .when(step < 0., |d| d.border_t_1().border_color(t.border))
                                    .when(!disabled, |d| {
                                        d.cursor_pointer().hover(|d| d.bg(t.hover))
                                    })
                                    .child(icon)
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        s.change_number(
                                            field,
                                            s.number_value(field) as f64 + step,
                                            false,
                                            cx,
                                        );
                                    }))
                            })),
                    )
                    .on_key_down(cx.listener(move |s, e: &KeyDownEvent, _, cx| {
                        let step = match e.keystroke.key.as_str() {
                            "up" => 1.,
                            "down" => -1.,
                            _ => return,
                        };
                        s.change_number(field, s.number_value(field) as f64 + step, false, cx);
                        cx.stop_propagation();
                    }))
                    .on_key_up(cx.listener(move |s, e: &KeyUpEvent, _, cx| {
                        let value = s.number_inputs[field as usize]
                            .as_ref()
                            .unwrap()
                            .read(cx)
                            .value();
                        if let Ok(value) = value.parse::<f64>() {
                            s.change_number(field, value, e.keystroke.key != "enter", cx);
                        }
                    })),
            )
    }

    fn select(
        &self,
        id: &'static str,
        current: &str,
        options: &[(&'static str, &'static str)],
        t: Theme,
        cx: &Context<Self>,
    ) -> Div {
        let values: Vec<_> = options.iter().map(|(value, _)| *value).collect();
        let selected_index = values
            .iter()
            .position(|value| *value == current)
            .unwrap_or(0);
        let label = options
            .iter()
            .find(|(value, _)| *value == current)
            .map_or(current, |(_, label)| *label)
            .to_owned();
        div()
            .relative()
            .w_full()
            .child(
                div()
                    .id(id)
                    .h(px(34.))
                    .px_3()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.field)
                    .text_color(t.text)
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .child(div().flex_1().child(label))
                    .child("⌄")
                    .on_click(cx.listener(move |s, _, window, cx| {
                        if let Some(focus) = &s.focus {
                            focus.focus(window);
                        }
                        s.choice_values = values.clone();
                        s.choice_index = selected_index;
                        s.choice_open = if s.choice_open == Some(id) {
                            None
                        } else {
                            Some(id)
                        };
                        cx.notify();
                    })),
            )
            .when(self.choice_open == Some(id), |d| {
                d.child(
                    deferred(
                        div()
                            .id(SharedString::from(format!("{id}-menu")))
                            .occlude()
                            .absolute()
                            .top(px(38.))
                            .left_0()
                            .w_full()
                            .p_1()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.raised)
                            .shadow_lg()
                            .on_mouse_down_out(cx.listener(move |s, _, _, cx| {
                                if s.choice_open == Some(id) {
                                    s.choice_open = None;
                                    cx.notify();
                                }
                            }))
                            .children(options.iter().enumerate().map(|(index, (value, label))| {
                                let value = *value;
                                div()
                                    .id(SharedString::from(format!("{id}-{value}")))
                                    .min_h(px(34.))
                                    .px_2()
                                    .py_2()
                                    .rounded(px(5.))
                                    .bg(if index == self.choice_index {
                                        t.hover
                                    } else {
                                        t.raised
                                    })
                                    .text_color(t.text)
                                    .cursor_pointer()
                                    .hover(|d| d.bg(t.hover))
                                    .child(*label)
                                    .on_click(cx.listener(move |s, _, _, cx| {
                                        s.set_choice(id, value, cx);
                                        s.choice_open = None;
                                        cx.notify();
                                    }))
                            })),
                    )
                    .with_priority(2),
                )
            })
    }

    fn set_choice(&mut self, id: &str, value: &'static str, cx: &mut Context<Self>) {
        match id {
            "quality-mode" => {
                self.comparison_dismissed = false;
                self.state.max_size_bytes = None;
                match value {
                    "preserve" => self.state.quality = QualityPreset::Preserve,
                    "compress" => self.state.quality = QualityPreset::High,
                    _ => {
                        self.state.max_size_bytes = self
                            .maximum_size_input
                            .as_ref()
                            .and_then(|input| {
                                parse_size_limit(&input.read(cx).value(), self.maximum_size_unit)
                            })
                            .or(Some(10_000_000))
                    }
                }
            }
            "compression-quality" => {
                self.state.quality = match value {
                    "highest" => QualityPreset::Highest,
                    "high" => QualityPreset::High,
                    "standard" => QualityPreset::Standard,
                    "small" => QualityPreset::Small,
                    _ => QualityPreset::Tiny,
                }
            }
            "size-unit" => {
                let unit = match value {
                    "kb" => 1_000,
                    "gb" => 1_000_000_000,
                    _ => 1_000_000,
                };
                if let Some(input) = &self.maximum_size_input {
                    let bytes = parse_size_limit(&input.read(cx).value(), self.maximum_size_unit);
                    if let Some(bytes) = bytes {
                        input.update(cx, |i, cx| {
                            i.set_value(format!("{}", bytes as f64 / unit as f64), cx)
                        });
                    }
                }
                self.maximum_size_unit = unit;
            }
            "output-resolution" => {
                self.resolution = value;
                if value == "custom" {
                    self.state.output_width = Some(self.number_value(NumberField::OutputWidth));
                    self.state.output_height = Some(self.number_value(NumberField::OutputHeight));
                }
                self.update_resolution();
                self.sync_numbers(None, cx);
            }
            "gif-fps" => self.state.gif_fps = value.parse().expect("fixed frame rate option"),
            "gif-palette" => self.state.gif_colors = value.parse().expect("fixed palette option"),
            _ => unreachable!("unknown recording editor select"),
        }
        self.save_draft();
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.choice_open {
            match e.keystroke.key.as_str() {
                "up" => self.choice_index = self.choice_index.saturating_sub(1),
                "down" => {
                    self.choice_index =
                        (self.choice_index + 1).min(self.choice_values.len().saturating_sub(1))
                }
                "home" => self.choice_index = 0,
                "end" => self.choice_index = self.choice_values.len().saturating_sub(1),
                "enter" | "space" => {
                    if let Some(&value) = self.choice_values.get(self.choice_index) {
                        self.set_choice(id, value, cx);
                    }
                    self.choice_open = None;
                }
                "escape" => self.choice_open = None,
                "tab" => {
                    self.choice_open = None;
                    cx.notify();
                    return;
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self
            .focus
            .as_ref()
            .is_none_or(|focus| !focus.is_focused(window))
        {
            return;
        }
        match e.keystroke.key.as_str() {
            "escape" => {
                self.format_open = false;
                self.choice_open = None;
                cx.notify();
            }
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

    fn audio_slider(
        &self,
        id: &'static str,
        control: AudioSlider,
        disabled: bool,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let recorded = bounds.clone();
        let pressed = bounds.clone();
        let value = match control {
            AudioSlider::System => self.state.system_volume,
            AudioSlider::Microphone => self.state.microphone_volume,
        };
        let ratio = (value / 2.).clamp(0., 1.);
        div()
            .id(id)
            .w_full()
            .h(px(28.))
            .when(!disabled, |d| d.cursor_pointer())
            .opacity(if disabled { 0.45 } else { 1. })
            .child(
                canvas(
                    move |area, _, _| recorded.set(area),
                    move |area, _, window, _| {
                        let left = area.left() + px(6.);
                        let width = area.size.width - px(12.);
                        let y = area.center().y;
                        window.paint_quad(fill(
                            Bounds::new(point(left, y - px(2.)), size(width, px(4.))),
                            t.border,
                        ));
                        window.paint_quad(fill(
                            Bounds::new(point(left, y - px(2.)), size(width * ratio, px(4.))),
                            t.accent,
                        ));
                        window.paint_quad(quad(
                            Bounds::new(
                                point(left + width * ratio - px(6.), y - px(6.)),
                                size(px(12.), px(12.)),
                            ),
                            Corners::all(px(6.)),
                            t.field,
                            Edges::all(px(1.)),
                            t.border,
                            BorderStyle::Solid,
                        ));
                    },
                )
                .size_full(),
            )
            .when(!disabled, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |s, event: &MouseDownEvent, _, cx| {
                        s.audio_slider_drag = Some(control);
                        s.change_audio_slider(control, event.position.x, pressed.get(), cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |s, event: &MouseMoveEvent, _, cx| {
                    if s.audio_slider_drag == Some(control) {
                        s.change_audio_slider(control, event.position.x, bounds.get(), cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|s, _, _, _| s.audio_slider_drag = None),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|s, _, _, _| s.audio_slider_drag = None),
                )
            })
    }

    fn change_audio_slider(
        &mut self,
        control: AudioSlider,
        x: Pixels,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let percent = bounded_percent(
            ((x - bounds.left() - px(6.)) / (bounds.size.width - px(12.)).max(px(1.))) * 200.,
        );
        match control {
            AudioSlider::System => self.state.system_volume = percent / 100.,
            AudioSlider::Microphone => self.state.microphone_volume = percent / 100.,
        }
        if self.playing {
            self.seek(self.playhead_ms, cx);
        }
        self.save_draft();
        cx.notify();
    }

    fn save_footer(&mut self, t: Theme, cx: &mut Context<Self>) -> Div {
        let filename = self
            .filename_input
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    TextInput::new(
                        format!(
                            "{}-edited",
                            self.source
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                        ),
                        "Saved filename",
                        cx,
                    )
                    .chrome()
                })
            })
            .clone();
        let formats = [
            ("mp4", ExportFormat::Mp4),
            ("gif", ExportFormat::Gif),
            ("webm", ExportFormat::WebM),
        ];
        let extension = formats
            .iter()
            .find(|(_, f)| *f == self.state.format)
            .unwrap()
            .0;
        let format_changed = self.source.extension().and_then(|e| e.to_str()) != Some(extension);
        let progress = self.export_progress.load(Ordering::Acquire) as f32 / 1000.;
        div()
            .absolute()
            .occlude()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(76.))
            .px_5()
            .py_2()
            .border_t_1()
            .border_color(t.border)
            .bg(t.raised)
            .flex()
            .items_center()
            .justify_center()
            .gap_4()
            .when(self.exporting, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h(px(2.))
                        .w(relative(progress))
                        .bg(t.accent),
                )
            })
            .child(
                div()
                    .w(px(420.))
                    .min_w(px(280.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(t.subtle)
                            .child("Filename")
                            .child(div().flex_1())
                            .child("Saving to")
                            .child(
                                div()
                                    .max_w(px(175.))
                                    .truncate()
                                    .child(self.destination.display().to_string()),
                            )
                            .child(
                                self.button("choose-destination", "Change…", t)
                                    .px_0()
                                    .py_0()
                                    .bg(transparent_black())
                                    .on_click(cx.listener(|s, _, window, cx| {
                                        if s.exporting {
                                            return;
                                        }
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
                                                        cx.notify();
                                                    }
                                                });
                                            }
                                        })
                                        .detach();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .h(px(32.))
                            .border_1()
                            .border_color(t.border)
                            .rounded(px(8.))
                            .bg(t.field)
                            .flex()
                            .items_center()
                            .child(div().flex_1().min_w_0().child(filename))
                            .child(
                                self.button("format", format!(".{extension} ⌄"), t)
                                    .h_full()
                                    .py_1()
                                    .on_click(cx.listener(|s, _, _, cx| {
                                        if !s.exporting {
                                            s.format_open = !s.format_open;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .when(self.format_open, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .occlude()
                                        .right_0()
                                        .bottom(px(36.))
                                        .w(px(120.))
                                        .p_1()
                                        .bg(t.raised)
                                        .border_1()
                                        .border_color(t.border)
                                        .rounded(px(8.))
                                        .shadow_lg()
                                        .flex()
                                        .flex_col()
                                        .children(formats.into_iter().map(|(label, format)| {
                                            self.button(label, label.to_uppercase(), t)
                                                .bg(if self.state.format == format {
                                                    t.hover
                                                } else {
                                                    t.raised
                                                })
                                                .on_click(cx.listener(move |s, _, _, cx| {
                                                    if s.exporting {
                                                        return;
                                                    }
                                                    s.state.format = format;
                                                    if format != ExportFormat::Mp4
                                                        && s.state.quality
                                                            == QualityPreset::Preserve
                                                    {
                                                        s.state.quality = QualityPreset::High;
                                                    }
                                                    s.format_open = false;
                                                    s.make_copy = true;
                                                    s.save_draft();
                                                    cx.notify();
                                                }))
                                        })),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .id("make-copy")
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(
                        div()
                            .w(px(30.))
                            .h(px(18.))
                            .rounded_full()
                            .bg(if self.make_copy || format_changed {
                                t.accent
                            } else {
                                t.hover
                            })
                            .p(px(2.))
                            .child(div().size(px(14.)).rounded_full().bg(t.raised).ml(px(
                                if self.make_copy || format_changed {
                                    12.
                                } else {
                                    0.
                                },
                            ))),
                    )
                    .child("Save as new file")
                    .on_click(cx.listener(move |s, _, _, cx| {
                        if s.exporting || format_changed {
                            return;
                        }
                        s.make_copy = !s.make_copy;
                        if let Some(input) = &s.filename_input {
                            input.update(cx, |input, cx| {
                                input.set_value(
                                    format!(
                                        "{}{}",
                                        s.source.file_stem().unwrap_or_default().to_string_lossy(),
                                        if s.make_copy { "-edited" } else { "" }
                                    ),
                                    cx,
                                )
                            });
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .min_w(px(220.))
                    .max_w(px(420.))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.subtle)
                            .truncate()
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(self.exporting, |d| {
                                d.child(self.button("cancel", "Cancel", t).on_click(cx.listener(
                                    |s, _, _, cx| {
                                        if let Some(cancel) = &s.export_cancel {
                                            cancel.cancel();
                                        }
                                        s.status = "Cancelling save…".into();
                                        cx.notify();
                                    },
                                )))
                            })
                            .when_some(
                                self.saved_path.clone().filter(|_| !self.exporting),
                                |d, path| {
                                    d.child(self.button("reveal", "Show in Folder", t).on_click(
                                        cx.listener(move |s, _, _, cx| {
                                            if let Err(error) =
                                                crate::previews::media::reveal(&path)
                                            {
                                                s.status = error.to_string();
                                                cx.notify();
                                            }
                                        }),
                                    ))
                                },
                            )
                            .child(
                                self.button("export", "▣  Save", t)
                                    .bg(t.accent)
                                    .text_color(black())
                                    .opacity(if self.exporting { 0.4 } else { 1. })
                                    .on_click(cx.listener(Self::export)),
                            ),
                    ),
            )
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
        let trim_start_focus = self
            .trim_start_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let trim_end_focus = self
            .trim_end_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        for field in NumberField::ALL {
            let value = self.number_value(field).to_string();
            let disabled = field.is_crop() && self.state.crop.is_none();
            self.number_inputs[field as usize]
                .get_or_insert_with(|| {
                    cx.new(|cx| TextInput::new(value, field.label(), cx).chrome())
                })
                .update(cx, |input, cx| input.set_disabled(disabled, cx));
        }
        let maximum_size_input = self
            .maximum_size_input
            .get_or_insert_with(|| {
                let value = self
                    .state
                    .max_size_bytes
                    .map(|bytes| format!("{:.1}", bytes as f64 / 1_000_000.))
                    .unwrap_or_else(|| "10".into());
                cx.new(|cx| TextInput::new(value, "Maximum file size", cx))
            })
            .clone();
        let poster = self.poster.clone();
        let frame = self.frame.clone();
        let duration = self.state.duration_ms as f32;
        let start = self.state.trim_start_ms as f32 / duration;
        let end = self.state.trim_end_ms as f32 / duration;
        let playhead = self.playhead_ms as f32 / duration;
        let frames = self.timeline_frames.clone();
        let waveform = self.waveform.clone();
        let crop = self.state.crop;
        let has_system_track =
            self.has_audio && (self.audio_tracks.system || !self.audio_tracks.microphone);
        let source_size = (self.source_width, self.source_height);
        let recorded_bounds = self.preview_bounds.clone();
        let paint_bounds = recorded_bounds.clone();
        let timeline_bounds = self.timeline_bounds.clone();
        let preview_height = f32::from(window.viewport_size().height) * 0.52;
        let preview_width = (preview_height * self.source_width as f32 / self.source_height as f32)
            .min(f32::from(window.viewport_size().width).min(1268.) - 74.);
        let media_height = preview_width * self.source_height as f32 / self.source_width as f32;
        self.request_quality(cx);
        let footer = self.save_footer(t, cx);
        div()
            .track_focus(&focus)
            .on_mouse_down(MouseButton::Left, move |_, window, _| focus.focus(window))
            .on_key_down(cx.listener(Self::key_down))
            .font_family(theme::font())
            .text_size(px(13.))
            .size_full()
            .bg(t.canvas)
            .text_color(t.text)
            .relative()
            .child(
                div()
                    .id("recording-editor-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .px_6()
                    .pt_6()
                    .pb(px(112.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .max_w(px(1220.))
                            .mx_auto()
                            .mb_4()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .line_height(px(26.4))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if self.state.format == ExportFormat::Gif {
                                        "Edit GIF"
                                    } else {
                                        "Edit recording"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(1220.))
                            .mx_auto()
                            .border_1()
                            .border_color(t.border)
                            .rounded_xl()
                            .bg(t.raised)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h(px(46.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .border_b_1()
                                    .border_color(t.border)
                                    .child(
                                        div()
                                            .text_color(t.muted)
                                            .text_size(px(13.))
                                            .child("Preview"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                self.button("preview-loop", "↻  Loop preview", t)
                                                    .when(self.preview_loop, |b| {
                                                        b.bg(t.hover)
                                                            .border_1()
                                                            .border_color(t.border)
                                                    })
                                                    .on_click(cx.listener(|s, _, _, cx| {
                                                        s.preview_loop = !s.preview_loop;
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .w(px(132.))
                                                    .h(px(34.))
                                                    .p(px(2.))
                                                    .rounded_md()
                                                    .bg(t.sunken)
                                                    .child(
                                                        self.button("preview-fit", "Fit", t)
                                                            .flex_1()
                                                            .py(px(6.))
                                                            .when(!self.preview_actual_size, |b| {
                                                                b.bg(t.raised)
                                                            })
                                                            .on_click(cx.listener(
                                                                |s, _, _, cx| {
                                                                    s.preview_actual_size = false;
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        self.button("preview-actual", "100%", t)
                                                            .flex_1()
                                                            .py(px(6.))
                                                            .when(self.preview_actual_size, |b| {
                                                                b.bg(t.raised)
                                                            })
                                                            .on_click(cx.listener(
                                                                |s, _, _, cx| {
                                                                    s.preview_actual_size = true;
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("recording-preview-viewport")
                                    .h(px(if self.preview_actual_size {
                                        preview_height + 24.
                                    } else {
                                        media_height + 24.
                                    }))
                                    .p_3()
                                    .bg(t.sunken)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .overflow_scroll()
                                    .child(
                                        div()
                                            .relative()
                                            .flex_shrink_0()
                                            .rounded(px(8.)).overflow_hidden()
                                            .when(self.preview_actual_size, |d| {
                                                d.w(px(self.source_width as f32))
                                                    .h(px(self.source_height as f32))
                                            })
                                            .when(!self.preview_actual_size, |d| {
                                                d.w(px(preview_width)).h(px(media_height))
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when_some(frame, |d, image| {
                                                d.child(
                                                    img(image)
                                                        .size_full()
                                                        .object_fit(ObjectFit::Contain),
                                                )
                                            })
                                            .when(self.frame.is_none(), |d| {
                                                d.when_some(poster, |d, p| {
                                                    d.child(
                                                        img(p)
                                                            .size_full()
                                                            .object_fit(ObjectFit::Contain),
                                                    )
                                                })
                                            })
                                            .when(self.can_compare() && !self.playing && !self.comparison_dismissed, |d| d.child(self.compression_overlay(cx)))
                                            .when_some(crop, |d, crop| {
                                                d.child(
                                                    div()
                                                        .absolute()
                                                        .inset_0()
                                                        .child(
                                                            canvas(
                                                                move |bounds, _, _| {
                                                                    recorded_bounds.set(
                                                                        fitted_video_bounds(
                                                                            bounds,
                                                                            source_size,
                                                                        ),
                                                                    )
                                                                },
                                                                move |_, _, window, _| {
                                                                    let fitted = paint_bounds.get();
                                                                    let sx = f32::from(
                                                                        fitted.size.width,
                                                                    ) / source_size.0
                                                                        as f32;
                                                                    let sy = f32::from(
                                                                        fitted.size.height,
                                                                    ) / source_size.1
                                                                        as f32;
                                                                    let rect = Rect {
                                                                        x: crop.x as f32,
                                                                        y: crop.y as f32,
                                                                        width: crop.width as f32,
                                                                        height: crop.height as f32,
                                                                    };
                                                                    let crop_bounds = Bounds {
                                                                        origin: point(
                                                                            fitted.origin.x
                                                                                + px(rect.x * sx),
                                                                            fitted.origin.y
                                                                                + px(rect.y * sy),
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
                                                                            fitted.origin.x
                                                                                + px(handle.0 * sx),
                                                                            fitted.origin.y
                                                                                + px(handle.1 * sy),
                                                                        );
                                                                        window.paint_quad(quad(
                                                                            Bounds {
                                                                                origin: point(
                                                                                    center.x
                                                                                        - px(5.),
                                                                                    center.y
                                                                                        - px(5.),
                                                                                ),
                                                                                size: size(
                                                                                    px(10.),
                                                                                    px(10.),
                                                                                ),
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
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(Self::crop_down),
                                                        )
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
                                            })
                                            .child(
                                                div()
                                                    .id("preview-play")
                                                    .absolute()
                                                    .top(relative(0.5))
                                                    .left(relative(0.5))
                                                    .ml(px(-28.))
                                                    .mt(px(-28.))
                                                    .size(px(56.))
                                                    .rounded_full()
                                                    .bg(t.accent)
                                                    .text_color(gpui::black())
                                                    .text_size(px(18.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .cursor_pointer()
                                                    .child(if self.playing { "Ⅱ" } else { "▶" })
                                                    .on_click(
                                                        cx.listener(|s, _, _, cx| {
                                                            s.toggle_play(cx)
                                                        }),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(1220.))
                            .mx_auto()
                            .my(px(12.))
                            .px(px(16.))
                            .pt_3()
                            .pb_4()
                            .border_1()
                            .border_color(t.border)
                            .rounded(px(14.))
                            .bg(t.raised)
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_size(px(12.))
                                    .line_height(px(16.2))
                                    .mb_2()
                                    .child(
                                        editor_time(self.state.trim_start_ms)
                                            + " – "
                                            + &editor_time(self.state.trim_end_ms),
                                    )
                                    .child(div().text_color(t.subtle).child(format!(
                                        "{} selected",
                                        editor_time(
                                            self.state.trim_end_ms - self.state.trim_start_ms
                                        )
                                    ))),
                            )
                            .child(
                                div()
                                    .id("timeline")
                                    .relative()
                                    .h(px(76.))
                                    .w_full()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(t.border)
                                    .bg(t.field)
                                    .cursor_pointer()
                                    .child(
                                        canvas(
                                            move |bounds, _, _| timeline_bounds.set(bounds),
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .inset_1()
                                            .flex()
                                            .overflow_hidden()
                                            .children(frames.into_iter().map(|frame| {
                                                img(frame)
                                                    .flex_1()
                                                    .h_full()
                                                    .object_fit(ObjectFit::Cover)
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
                                            .left_0()
                                            .w(relative(start))
                                            .bg(gpui::black())
                                            .opacity(0.66),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .left(relative(end))
                                            .right_0()
                                            .bg(gpui::black())
                                            .opacity(0.66),
                                    )
                                    .child(
                                        div()
                                            .id("trim-start-handle")
                                            .track_focus(&trim_start_focus)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                move |_, window, cx| {
                                                    trim_start_focus.focus(window);
                                                    cx.stop_propagation();
                                                },
                                            )
                                            .on_key_down(cx.listener(
                                                |s, event: &KeyDownEvent, _, cx| {
                                                    let Some(delta) = trim_keyboard_delta(
                                                        &event.keystroke.key,
                                                        s.state.duration_ms,
                                                    ) else {
                                                        return;
                                                    };
                                                    let next = (s.state.trim_start_ms as i64
                                                        + delta)
                                                        .clamp(
                                                            0,
                                                            s.state.trim_end_ms.saturating_sub(1)
                                                                as i64,
                                                        )
                                                        as u64;
                                                    s.state.trim_start_ms = next;
                                                    s.seek(next, cx);
                                                    s.save_draft();
                                                    cx.stop_propagation();
                                                },
                                            ))
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .left(relative(start))
                                            .ml(px(-8. * start))
                                            .top(px(-3.))
                                            .bottom(px(-3.))
                                            .w(px(8.))
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(t.border)
                                            .bg(t.accent)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(9.))
                                            .text_color(black())
                                            .child("‖"),
                                    )
                                    .child(
                                        div()
                                            .id("trim-end-handle")
                                            .track_focus(&trim_end_focus)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                move |_, window, cx| {
                                                    trim_end_focus.focus(window);
                                                    cx.stop_propagation();
                                                },
                                            )
                                            .on_key_down(cx.listener(
                                                |s, event: &KeyDownEvent, _, cx| {
                                                    let Some(delta) = trim_keyboard_delta(
                                                        &event.keystroke.key,
                                                        s.state.duration_ms,
                                                    ) else {
                                                        return;
                                                    };
                                                    let next = (s.state.trim_end_ms as i64 + delta)
                                                        .clamp(
                                                            s.state.trim_start_ms.saturating_add(1)
                                                                as i64,
                                                            s.state.duration_ms as i64,
                                                        )
                                                        as u64;
                                                    s.state.trim_end_ms = next;
                                                    s.seek(next.saturating_sub(1), cx);
                                                    s.save_draft();
                                                    cx.stop_propagation();
                                                },
                                            ))
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .left(relative(end))
                                            .ml(px(-8. * end))
                                            .top(px(-3.))
                                            .bottom(px(-3.))
                                            .w(px(8.))
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(t.border)
                                            .bg(t.accent)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(9.))
                                            .text_color(black())
                                            .child("‖"),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .left(relative(playhead))
                                            .ml(px(4. - 8. * playhead))
                                            .w(px(2.))
                                            .bg(t.text),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(Self::timeline_down),
                                    )
                                    .on_mouse_move(cx.listener(Self::timeline_move))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|s, _, _, _| s.drag = None),
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        cx.listener(|s, _, _, _| s.drag = None),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .grid().grid_cols(2).when(compact, |d| d.grid_cols(1))
                            .w_full().max_w(px(1220.)).mx_auto().gap(px(12.))
                            .children([
                                div()
                                    .min_w_0().p(px(16.)).rounded(px(14.))
                                    .border_1().border_color(t.border).bg(t.raised)
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child(div().mb_1().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Crop & size"))
                                    .child(
                                        check_row("crop-enabled", "Crop recording", crop.is_some(), t)
                                        .on_click(
                                            cx.listener(|s, _, _, cx| {
                                                if s.state.crop.is_some() {
                                                    s.state.crop = None;
                                                } else {
                                                    s.state.set_crop(
                                                        Some(s.current_crop()),
                                                        s.source_width,
                                                        s.source_height,
                                                    );
                                                }
                                                s.update_resolution();
                                                s.sync_numbers(None, cx);
                                                s.save_draft();
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        div().grid().grid_cols(4).gap(px(6.)).mt(px(8.))
                                            .children([NumberField::X, NumberField::Y, NumberField::Width, NumberField::Height].into_iter().map(|f| self.number_field(f, t, cx)))
                                    )
                                    .child(check_row("aspect-lock", "Lock aspect ratio", self.aspect_locked, t).mt(px(6.)).mb(px(8.))
                                        .on_click(cx.listener(|s, _, _, cx| { s.aspect_locked = !s.aspect_locked; cx.notify(); })))
                                    .child(div().text_color(t.subtle).child("Output resolution"))
                                    .child(self.select("output-resolution", self.resolution,
                                        &[("original", "Original"), ("1080", "1080p maximum"), ("720", "720p maximum"), ("custom", "Custom")], t, cx))
                                    .when(self.resolution == "custom", |d| d.child(
                                        div().grid().grid_cols(2).gap(px(6.))
                                            .child(self.number_field(NumberField::OutputWidth, t, cx))
                                            .child(self.number_field(NumberField::OutputHeight, t, cx)),
                                    ))
                                    .into_any_element(),
                                div()
                                    .min_w_0().p(px(16.)).rounded(px(14.))
                                    .border_1().border_color(t.border).bg(t.raised)
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child(div().mb_1().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Save quality"))
                                    .child(div().text_color(t.subtle).child("Quality mode"))
                                    .child(self.select("quality-mode", if self.state.max_size_bytes.is_some() { "maximum" } else if self.state.quality == QualityPreset::Preserve { "preserve" } else { "compress" },
                                        &[("preserve", "Preserve quality"), ("compress", "Compress"), ("maximum", "Maximum file size")], t, cx))
                                    .child(div().text_size(px(12.)).text_color(t.subtle).child(if self.state.max_size_bytes.is_some() {
                                        "Set a hard size limit for the saved file."
                                    } else if self.state.quality == QualityPreset::Preserve {
                                        "Original quality with no extra compression unless an edit requires it."
                                    } else { "Choose a smaller file with Tiny through Highest quality presets." }))
                                    .when(self.state.max_size_bytes.is_none() && self.state.quality != QualityPreset::Preserve, |d| d
                                        .child(div().text_color(t.subtle).child("Quality"))
                                        .child(self.select("compression-quality", match self.state.quality {
                                            QualityPreset::Highest => "highest", QualityPreset::High => "high", QualityPreset::Standard => "standard", QualityPreset::Small => "small", _ => "tiny"
                                        }, &[("tiny", "Tiny"), ("small", "Small"), ("standard", "Standard"), ("high", "High"), ("highest", "Highest")], t, cx)))
                                    .when(self.state.max_size_bytes.is_some(), |d| d
                                        .child(div().text_color(t.subtle).child("Maximum file size"))
                                        .child(div().flex().gap_2().items_center()
                                            .child(div().flex_1().child(maximum_size_input).on_key_up(cx.listener(|s, _, _, cx| {
                                                if let Some(bytes) = s.maximum_size_input.as_ref().and_then(|input| parse_size_limit(&input.read(cx).value(), s.maximum_size_unit)) {
                                                    s.state.max_size_bytes = Some(bytes); s.save_draft(); cx.notify();
                                                }
                                            })))
                                            .child(div().w(px(82.)).child(self.select("size-unit", match self.maximum_size_unit { 1_000 => "kb", 1_000_000_000 => "gb", _ => "mb" },
                                                &[("kb", "KB"), ("mb", "MB"), ("gb", "GB")], t, cx)))))
                                    .child(div().mt(px(8.)).text_color(t.subtle).child("Est. size"))
                                    .child(div().font_family("monospace").child(
                                        if let Some(bytes) = self.state.max_size_bytes { format!("≤ {}", file_size(bytes)) }
                                        else if self.quality_pending { "Estimating…".into() }
                                        else { self.estimated_size.map_or_else(|| "—".into(), |(bytes, exact)| format!("{}{}", if exact { "" } else { "≈ " }, file_size(bytes))) }
                                    ))
                                    .when(self.can_compare() && self.comparison_dismissed, |d| d.child(
                                        self.button("show-comparison", "Show before / after", t)
                                            .on_click(cx.listener(|s, _, _, cx| { s.comparison_dismissed = false; cx.notify(); }))
                                    ))
                                    .when_some(self.quality_error.clone(), |d, error| d.child(div().text_size(px(11.)).text_color(t.signal).child(error)))
                                    .when(self.state.format == ExportFormat::Gif, |d| {
                                        d.child(div().text_color(t.subtle).child("Frame rate"))
                                            .child(self.select("gif-fps", &self.state.gif_fps.to_string(), &[("8", "8 FPS"), ("10", "10 FPS"), ("12", "12 FPS"), ("15", "15 FPS"), ("20", "20 FPS"), ("24", "24 FPS"), ("30", "30 FPS")], t, cx))
                                            .child(div().text_color(t.subtle).child("Colors"))
                                            .child(self.select("gif-palette", &self.state.gif_colors.to_string(), &[("32", "32"), ("64", "64"), ("128", "128"), ("256", "256")], t, cx))
                                    })
                                    .into_any_element(),
                                div()
                                    .col_span(2).when(compact, |d| d.col_span(1))
                                    .min_w_0().p(px(16.)).rounded(px(14.))
                                    .border_1().border_color(t.border).bg(t.raised)
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .when(!self.has_audio || self.state.format == ExportFormat::WebM, |d| d.hidden())
                                    .child(div().mb_1().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Audio"))
                                    .when(self.state.format == ExportFormat::Gif, |d| d.child("GIFs do not include recorded audio."))
                                    .when(has_system_track && self.state.format == ExportFormat::Mp4, |d| {
                                        d.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    self.button(
                                                        "system-mute",
                                                        if self.state.mute_system_audio {
                                                            "☐ System audio"
                                                        } else {
                                                            "☑ System audio"
                                                        },
                                                        t,
                                                    )
                                                    .on_click(cx.listener(|s, _, _, cx| {
                                                        s.state.mute_system_audio =
                                                            !s.state.mute_system_audio;
                                                        if s.playing {
                                                            s.seek(s.playhead_ms, cx);
                                                        }
                                                        s.save_draft();
                                                        cx.notify();
                                                    })),
                                                )
                                                .child(self.audio_slider(
                                                    "system-volume",
                                                    AudioSlider::System,
                                                    self.state.mute_system_audio,
                                                    t,
                                                    cx,
                                                ))
                                                .child(format!(
                                                    "{:.0}%",
                                                    self.state.system_volume * 100.
                                                )),
                                        )
                                    })
                                    .when(self.has_audio && self.audio_tracks.microphone && self.state.format == ExportFormat::Mp4, |d| {
                                        d.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    self.button(
                                                        "microphone-mute",
                                                        if self.state.mute_microphone {
                                                            "☐ Microphone"
                                                        } else {
                                                            "☑ Microphone"
                                                        },
                                                        t,
                                                    )
                                                    .on_click(cx.listener(|s, _, _, cx| {
                                                        s.state.mute_microphone =
                                                            !s.state.mute_microphone;
                                                        if s.playing {
                                                            s.seek(s.playhead_ms, cx);
                                                        }
                                                        s.save_draft();
                                                        cx.notify();
                                                    })),
                                                )
                                                .child(self.audio_slider(
                                                    "microphone-volume",
                                                    AudioSlider::Microphone,
                                                    self.state.mute_microphone,
                                                    t,
                                                    cx,
                                                ))
                                                .child(format!(
                                                    "{:.0}%",
                                                    self.state.microphone_volume * 100.
                                                )),
                                        )
                                    })
                                    .when(self.state.format == ExportFormat::Mp4, |d| d.child(
                                        div().flex().gap_2().child(
                                            self.button(
                                                "mono",
                                                if self.state.mono_audio {
                                                    "☑ Convert to mono"
                                                } else {
                                                    "☐ Convert to mono"
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
                                    ))
                                    .into_any_element(),
                            ]),
                    ),
            )
            .child(footer)
    }
}

fn quality_image(mut image: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(image)]))
}

fn file_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.)
    } else {
        format!("{:.1} KB", bytes as f64 / 1_000.)
    }
}

fn check_row(id: &'static str, label: &'static str, checked: bool, t: Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .cursor_pointer()
        .child(
            div()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .border_color(if checked { t.accent } else { t.border })
                .bg(if checked { t.accent } else { t.field })
                .text_color(black())
                .text_size(px(11.))
                .child(if checked { "✓" } else { "" }),
        )
        .child(label)
}

impl Drop for RecordingEditor {
    fn drop(&mut self) {
        if let Some(cancel) = &self.quality_cancel {
            cancel.cancel();
        }
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
    fn numeric_crop_keeps_ratio_fits_both_bounds_and_translation_keeps_size() {
        let crop = CropRect {
            x: 80,
            y: 50,
            width: 400,
            height: 200,
        };
        let expected = CropRect {
            width: 560,
            height: 280,
            ..crop
        };
        assert_eq!(
            crop_number(crop, NumberField::Width, 1000., (640, 360), true),
            expected
        );
        assert_eq!(
            crop_number(crop, NumberField::Height, 900., (640, 360), true),
            expected
        );
        assert_eq!(
            crop_number(crop, NumberField::Height, 900., (640, 360), false),
            CropRect {
                height: 310,
                ..crop
            }
        );
        assert_eq!(
            crop_number(crop, NumberField::X, 999., (640, 360), true),
            CropRect { x: 240, ..crop }
        );
        assert_eq!(
            crop_number(crop, NumberField::Y, -99., (640, 360), true),
            CropRect { y: 0, ..crop }
        );
        assert_eq!(
            crop_number(crop, NumberField::Width, 0., (640, 360), false),
            CropRect { width: 2, ..crop }
        );
    }

    #[test]
    fn locked_crop_drag_anchors_opposite_corner_or_side_center() {
        let rect = Rect {
            x: 80.,
            y: 50.,
            width: 400.,
            height: 200.,
        };
        assert_eq!(
            crop_drag_rect(rect, 0, (-100., -10.), (640., 360.), true),
            Rect {
                x: 0.,
                y: 10.,
                width: 480.,
                height: 240.
            }
        );
        assert_eq!(
            crop_drag_rect(rect, 3, (200., 0.), (640., 360.), true),
            Rect {
                x: 80.,
                y: 10.,
                width: 560.,
                height: 280.
            }
        );
        assert_eq!(
            crop_drag_rect(rect, 3, (200., 0.), (640., 360.), false),
            Rect {
                width: 560.,
                ..rect
            }
        );
        assert_eq!(
            crop_drag_rect(rect, 8, (1000., -1000.), (640., 360.), true),
            Rect {
                x: 240.,
                y: 0.,
                ..rect
            }
        );
    }

    #[test]
    fn timeline_boundaries_are_asymmetric_and_clamped() {
        assert_eq!(timeline_value(4., 1008., 10_000), 0);
        assert_eq!(timeline_value(1004., 1008., 10_000), 10_000);
        assert_eq!(timeline_value(260., 1008., 10_000), 2_560);
        assert_eq!(timeline_value(-50., 1008., 10_000), 0);
    }

    #[test]
    fn maximum_size_parsing_supports_all_displayed_units() {
        assert_eq!(parse_size_limit("100", 1_000), Some(100_000));
        assert_eq!(parse_size_limit("0.5", 1_000_000), Some(500_000));
        assert_eq!(parse_size_limit("1.25", 1_000_000_000), Some(1_250_000_000));
        for invalid in ["", "0", "-1", "wat", "NaN"] {
            assert_eq!(parse_size_limit(invalid, 1_000_000), None);
        }
    }

    #[test]
    fn volume_slider_clamps_to_zero_through_two_hundred_percent() {
        assert_eq!(bounded_percent(-0.1), 0.);
        assert_eq!(bounded_percent(127.5), 127.5);
        assert_eq!(bounded_percent(250.), 200.);
    }

    #[test]
    fn trim_keyboard_matches_web_editor_steps() {
        assert_eq!(trim_keyboard_delta("left", 59_999), Some(-1));
        assert_eq!(trim_keyboard_delta("right", 59_999), Some(1));
        assert_eq!(trim_keyboard_delta("down", 60_000), Some(-10));
        assert_eq!(trim_keyboard_delta("up", 60_000), Some(10));
        assert_eq!(trim_keyboard_delta("pagedown", 60_000), Some(-1_000));
        assert_eq!(trim_keyboard_delta("pageup", 60_000), Some(1_000));
        assert_eq!(trim_keyboard_delta("space", 60_000), None);
    }

    #[test]
    fn video_filename_is_a_stem_not_a_path() {
        assert_eq!(
            video_export_path(Path::new("/out"), "my.edit", "webm").unwrap(),
            Path::new("/out/my.edit.webm")
        );
        for invalid in [
            "",
            ".",
            "..",
            "../source",
            "dir/file",
            "dir\\file",
            "bad\0file",
        ] {
            assert!(video_export_path(Path::new("/out"), invalid, "mp4").is_err());
        }
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
