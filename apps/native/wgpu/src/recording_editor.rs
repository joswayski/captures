//! Frame-based recording editor. Media work stays on one serialized worker;
//! the UI never substitutes a poster or unaccepted edit for the decoded frame.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequestV2 as RecordingEditorRequest,
    RecordingEditorSession, RecordingExportComparison, RecordingSaveRequest,
    RecordingTimelineThumbnails, ReplaceOriginalError, ReplacedRecording, SavedRecording,
};
use captures_app::recording_editor_ui::{self, QualityMode, ResolutionChoice};
use captures_app::recording_timeline::{
    TimelineTrimDrag, TimelineTrimEdge, timeline_ratio, timeline_time_at_client_x,
};
use captures_media::{
    AudioEdit, CancelToken, CropDragHandle, CropRect, CropResizeAxis, EditSpec, ExportEstimate,
    ExportFormat, ExportProgress, ExportSpec, MediaMetadata, MediaToolchain, QualityPreset,
};
use captures_recording::MaxResolution;
use eframe::egui;
use image::RgbaImage;

use crate::tokens::Tokens;

struct Presented {
    revision: u64,
    preview_export: ExportSpec,
    source: MediaMetadata,
    edit: EditSpec,
    export: ExportSpec,
    position_ms: u64,
    frame: Arc<RgbaImage>,
    /// Frames the source capture dropped (shipping's header warning).
    dropped_frames: u64,
}

impl Presented {
    fn from_session(session: &RecordingEditorSession) -> Self {
        let snapshot = session.snapshot_v2();
        Self {
            revision: snapshot.editor.revision,
            preview_export: snapshot.editor.preview_export.clone(),
            source: snapshot.editor.source.clone(),
            edit: snapshot.editor.edit.clone(),
            export: snapshot.save_export.clone(),
            position_ms: snapshot.editor.position_ms,
            frame: session.frame(),
            dropped_frames: snapshot.editor.dropped_frames,
        }
    }
}

enum Job {
    Apply(RecordingEditorRequest),
    Save(RecordingSaveRequest, CancelToken),
    Replace(CancelToken),
    Estimate(CancelToken),
    Compare(u64, CancelToken),
    Thumbnails(CancelToken),
    SourceFrame(CancelToken),
    Play(u64, bool, CancelToken),
    Shutdown,
}

struct Comparison {
    revision: u64,
    position_ms: u64,
    after_seek_position_ms: u64,
    export: ExportSpec,
    frames: [Arc<RgbaImage>; 2],
}

impl From<RecordingExportComparison> for Comparison {
    fn from(result: RecordingExportComparison) -> Self {
        Self {
            revision: result.revision,
            position_ms: result.position_ms,
            after_seek_position_ms: result.after_seek_position_ms,
            export: result.export.clone(),
            frames: [result.before_frame(), result.after_frame()],
        }
    }
}

struct ComparisonPreview {
    result: Comparison,
    textures: [egui::TextureHandle; 2],
}

struct PlaybackFrame {
    position_ms: u64,
    pixels: Arc<RgbaImage>,
}

enum PlaybackEnd {
    Paused,
    Ended,
}

enum Event {
    Opened(Presented, Option<PathBuf>),
    Presented(Result<Presented, String>),
    Progress(ExportProgress),
    Saved(Result<SavedRecording, String>),
    Replaced(Result<(PathBuf, Presented), ReplaceOriginalError>),
    Estimated(Result<ExportEstimate, String>),
    Compared(u64, Result<Comparison, String>),
    Thumbnails(Result<RecordingTimelineThumbnails, String>),
    SourceFrame(Result<Arc<RgbaImage>, String>),
    PlaybackStarted { audio_enabled: bool },
    PlaybackFinished(Result<PlaybackEnd, String>),
    Destination(Option<PathBuf>),
}

struct TrimGesture {
    edge: TimelineTrimEdge,
    drag: TimelineTrimDrag,
    track: egui::Rect,
}

struct CropGesture {
    initial: CropRect,
    handle: CropDragHandle,
    origin: egui::Pos2,
    image: egui::Rect,
    locked: bool,
}

struct ReplacementConfirmation {
    path: PathBuf,
    revision: u64,
    export: ExportSpec,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum FileSizeUnit {
    Kb,
    #[default]
    Mb,
    Gb,
}

impl FileSizeUnit {
    fn label(self) -> &'static str {
        match self {
            Self::Kb => "KB",
            Self::Mb => "MB",
            Self::Gb => "GB",
        }
    }

    fn digits(self) -> usize {
        match self {
            Self::Kb => 3,
            Self::Mb => 6,
            Self::Gb => 9,
        }
    }

    fn bytes(self, value: &str) -> Option<u64> {
        // Decimal units, floored to whole bytes without floating-point rounding
        // at the 100 KB boundary or when switching units.
        let value = value.trim();
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty() && fraction.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let whole = if whole.is_empty() {
            0
        } else {
            whole.parse::<u64>().ok()?
        };
        let fraction = &fraction[..fraction.len().min(self.digits())];
        let part = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u64>().ok()?
        };
        whole
            .checked_mul(10_u64.pow(self.digits() as u32))?
            .checked_add(part * 10_u64.pow((self.digits() - fraction.len()) as u32))
    }

    fn value(self, bytes: u64) -> String {
        let factor = 10_u64.pow(self.digits() as u32);
        let fraction = format!("{:0width$}", bytes % factor, width = self.digits());
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            (bytes / factor).to_string()
        } else {
            format!("{}.{}", bytes / factor, fraction)
        }
    }
}

#[derive(Default)]
struct View {
    presented: Option<Presented>,
    texture: Option<egui::TextureHandle>,
    source_texture: Option<egui::TextureHandle>,
    comparison: Option<ComparisonPreview>,
    comparison_split: f32,
    comparison_generation: u64,
    comparing: Option<u64>,
    preview_actual_size: bool,
    adjusting_crop: bool,
    crop_gesture: Option<CropGesture>,
    loading_source: bool,
    thumbnails: Option<egui::TextureHandle>,
    loading_thumbnails: bool,
    thumbnail_error: Option<String>,
    playing: bool,
    preview_loop: Arc<AtomicBool>,
    preview_sound: bool,
    playback_audio_enabled: bool,
    playback_position_ms: Option<u64>,
    playback_ended: bool,
    close_after_playback: bool,
    busy: bool,
    cancel: Option<CancelToken>,
    estimating: bool,
    estimate: Option<ExportEstimate>,
    picker: bool,
    closed: bool,
    confirm_close: bool,
    original_path: Option<PathBuf>,
    confirm_replace: Option<ReplacementConfirmation>,
    requires_reopen: bool,
    history_changed: bool,
    original_replaced: bool,
    saved_edit: EditSpec,
    saved_export: Option<ExportSpec>,
    start_ms: u64,
    end_ms: u64,
    trim_gesture: Option<TrimGesture>,
    crop: Option<CropRect>,
    crop_aspect_unlocked: bool,
    output_size: Option<(u32, u32)>,
    max_resolution: MaxResolution,
    audio: AudioEdit,
    position_ms: u64,
    /// Timeline scrub in progress over this track rectangle.
    scrub: Option<egui::Rect>,
    artifact_id: String,
    directory: PathBuf,
    stem: String,
    gif: bool,
    gif_frames_per_second: Option<u16>,
    gif_maximum_width: Option<u32>,
    quality: QualityPreset,
    /// Last Compress preset, restored when leaving Preserve or Maximum.
    compress_quality: Option<QualityPreset>,
    maximum_size: bool,
    maximum_value: String,
    maximum_unit: FileSizeUnit,
    progress: Option<ExportProgress>,
    status: Option<String>,
    error: Option<String>,
    /// The last new copy this editor saved, for shipping's Show in Folder.
    saved_path: Option<PathBuf>,
    probe_emitted: Option<ProbeRects>,
}

impl View {
    fn destination(&self) -> PathBuf {
        self.directory.join(format!(
            "{}.{}",
            self.stem,
            if self.gif { "gif" } else { "mp4" }
        ))
    }

    fn export_spec(&self) -> ExportSpec {
        ExportSpec {
            format: if self.gif {
                ExportFormat::Gif
            } else {
                ExportFormat::Mp4
            },
            quality: if self.maximum_size {
                QualityPreset::Preserve
            } else {
                self.quality
            },
            // Invalid text must remain an unapplied, invalid cap, never silently
            // turn maximum mode into an unbounded export. Apply is gated below.
            max_size_bytes: self.maximum_size.then(|| self.maximum_bytes().unwrap_or(0)),
            frames_per_second: if self.gif {
                self.gif_frames_per_second
            } else {
                None
            },
            // Match the shipping editor's quality-to-palette mapping, including
            // the remembered quality while Maximum forces Preserve encoding.
            // A rebased GIF's null palette already means 256. Retain that
            // accepted identity until the user selects another quality.
            gif_max_colors: (self.gif
                && !(self.quality == QualityPreset::Preserve
                    && self.presented.as_ref().is_some_and(|p| {
                        p.export.format == ExportFormat::Gif && p.export.gif_max_colors.is_none()
                    })))
            .then_some(match self.quality {
                QualityPreset::Tiny => 64,
                QualityPreset::Small => 96,
                QualityPreset::Standard => 128,
                QualityPreset::Preserve | QualityPreset::High | QualityPreset::Highest => 256,
            }),
        }
    }

    fn maximum_bytes(&self) -> Option<u64> {
        self.maximum_unit
            .bytes(&self.maximum_value)
            .filter(|bytes| *bytes >= 100_000)
    }

    fn set_maximum_unit(&mut self, unit: FileSizeUnit) {
        if let Some(bytes) = self.maximum_unit.bytes(&self.maximum_value) {
            self.maximum_value = unit.value(bytes);
        }
        self.maximum_unit = unit;
    }

    fn output_dimensions(&self, source_size: (u32, u32)) -> Option<(u32, u32)> {
        self.output_size.or_else(|| match self.max_resolution {
            MaxResolution::Original => None,
            preset => {
                let (width, height) = self
                    .crop
                    .map_or(source_size, |crop| (crop.width, crop.height));
                Some(preset.constrain(width, height))
            }
        })
    }

    fn staged_edit(&self, p: &Presented) -> EditSpec {
        let mut output_size = self.output_dimensions((p.source.width, p.source.height));
        if self.gif {
            let base = output_size.unwrap_or_else(|| {
                self.crop.map_or((p.source.width, p.source.height), |crop| {
                    (crop.width, crop.height)
                })
            });
            let (width, height) = MaxResolution::Original.constrain(base.0, base.1);
            let maximum = self.gif_maximum_width.unwrap_or(800);
            output_size = Some(if width > maximum {
                let scale = f64::from(maximum) / f64::from(width);
                (
                    maximum,
                    ((f64::from(height) * scale).round().max(2.0) as u32) & !1,
                )
            } else {
                (width, height)
            });
            // A rebased source has no resize. An inert width cap must not
            // turn that accepted state into a phantom unapplied edit.
            if p.export.format == ExportFormat::Gif
                && p.edit.output_width.is_none()
                && p.edit.output_height.is_none()
                && self.crop.is_none()
                && self.output_size.is_none()
                && self.max_resolution == MaxResolution::Original
                && output_size == Some((p.source.width, p.source.height))
            {
                output_size = None;
            }
        }
        EditSpec {
            trim_start_ms: self.start_ms,
            trim_end_ms: (self.end_ms != p.source.duration_ms.unwrap_or(0)).then_some(self.end_ms),
            crop: self.crop,
            output_width: output_size.map(|size| size.0),
            output_height: output_size.map(|size| size.1),
            audio: self.audio.clone(),
        }
    }

    fn unapplied(&self) -> bool {
        self.presented
            .as_ref()
            .is_some_and(|p| self.staged_edit(p) != p.edit || self.export_spec() != p.export)
    }

    fn dirty(&self) -> bool {
        self.unapplied()
            || self.presented.as_ref().is_some_and(|p| {
                p.edit != self.saved_edit || Some(&p.export) != self.saved_export.as_ref()
            })
    }

    fn can_replace(&self) -> bool {
        !self.busy
            && !self.picker
            && !self.closed
            && !self.confirm_close
            && !self.requires_reopen
            && !self.unapplied()
            && !self.adjusting_crop
            && self.presented.as_ref().is_some_and(|p| {
                let extension = match (p.source.mime_type.as_str(), p.export.format) {
                    ("video/mp4", ExportFormat::Mp4) => "mp4",
                    ("image/gif", ExportFormat::Gif) => "gif",
                    _ => return false,
                };
                self.original_path.as_ref().is_some_and(|path| {
                    path.extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
                })
            })
    }

    fn begin_replace(&mut self) {
        if !self.can_replace() || self.confirm_replace.is_some() {
            return;
        }
        let p = self.presented.as_ref().unwrap();
        self.confirm_replace = Some(ReplacementConfirmation {
            path: self.original_path.clone().unwrap(),
            revision: p.revision,
            export: p.export.clone(),
        });
        self.trim_gesture = None;
        self.crop_gesture = None;
    }

    fn confirm_replacement(&mut self, tx: &Sender<Job>) {
        let Some(confirmed) = self.confirm_replace.take() else {
            return;
        };
        if !self.can_replace()
            || self.original_path.as_ref() != Some(&confirmed.path)
            || self
                .presented
                .as_ref()
                .is_none_or(|p| p.revision != confirmed.revision || p.export != confirmed.export)
        {
            self.error = Some("Recording changed. Review the original and confirm again.".into());
            return;
        }
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Replace(cancel));
    }

    fn estimate_presentation(&self) -> recording_editor_ui::EstimatePresentation {
        let estimate = self.estimate.as_ref();
        recording_editor_ui::estimate(&recording_editor_ui::EstimateInput {
            estimating: self.estimating,
            unapplied: self.unapplied(),
            invalid_maximum: self.maximum_size && self.maximum_bytes().is_none(),
            maximum_bytes: self
                .presented
                .as_ref()
                .and_then(|p| p.export.max_size_bytes),
            estimate_bytes: estimate.map(|estimate| estimate.size_bytes),
            estimate_exact: estimate.is_some_and(|estimate| estimate.exact),
            // A staged Maximum never advertises a reduction.
            original_bytes: if self.maximum_size {
                0
            } else {
                self.presented.as_ref().map_or(0, |p| p.source.size_bytes)
            },
        })
    }

    #[cfg(test)]
    fn estimate_label(&self) -> String {
        let shown = self.estimate_presentation();
        match shown.delta {
            Some(delta) => format!("{} · {}", shown.label, delta.label),
            None => shown.label,
        }
    }

    fn request_estimate(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.confirm_replace.is_some()
            || self.presented.is_none()
            || self.unapplied()
            || self
                .presented
                .as_ref()
                .is_some_and(|p| p.export.max_size_bytes.is_some())
        {
            return;
        }
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Estimate(cancel));
    }

    fn can_compare(&self) -> bool {
        !self.busy
            && !self.picker
            && !self.confirm_close
            && self.confirm_replace.is_none()
            && !self.closed
            && !self.adjusting_crop
            && !self.unapplied()
            && self.presented.as_ref().is_some_and(|p| {
                p.position_ms >= p.edit.trim_start_ms
                    && p.position_ms
                        < p.edit
                            .trim_end_ms
                            .unwrap_or(p.source.duration_ms.unwrap_or(0))
            })
    }

    fn request_comparison(&mut self, ctx: &egui::Context, tx: &Sender<Job>) {
        if !self.can_compare() {
            return;
        }
        // Playback owns transient pixels/time. Comparison explicitly returns to
        // the accepted still rather than silently relabelling a paused frame.
        let p = self.presented.as_ref().unwrap();
        self.position_ms = p.position_ms;
        let frame = p.frame.clone();
        self.set_frame(ctx, &frame);
        self.playback_position_ms = None;
        self.playback_ended = false;
        self.comparison = None;
        self.comparison_generation += 1;
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Compare(self.comparison_generation, cancel));
    }

    fn request_thumbnails(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.confirm_replace.is_some()
            || self.closed
            || self.presented.is_none()
            || self.thumbnails.is_some()
        {
            return;
        }
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Thumbnails(cancel));
    }

    fn request_crop_view(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.confirm_replace.is_some()
            || self.closed
            || self.presented.is_none()
            || self.crop.is_none()
        {
            return;
        }
        if self.source_texture.is_some() {
            self.comparison = None;
            self.adjusting_crop = true;
        } else {
            let cancel = CancelToken::default();
            self.cancel = Some(cancel.clone());
            self.send(tx, Job::SourceFrame(cancel));
        }
    }

    fn request_playback(&mut self, tx: &Sender<Job>) {
        if self.busy
            || self.picker
            || self.confirm_close
            || self.confirm_replace.is_some()
            || self.requires_reopen
            || self.closed
            || self.unapplied()
            || self.adjusting_crop
        {
            return;
        }
        let Some(p) = &self.presented else { return };
        let position = if self.playback_ended {
            p.edit.trim_start_ms
        } else {
            self.playback_position_ms.unwrap_or(p.position_ms)
        };
        self.position_ms = self.playback_position_ms.unwrap_or(p.position_ms);
        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        self.send(tx, Job::Play(position, self.preview_sound, cancel));
    }

    fn pause_playback(&self) {
        if self.playing
            && let Some(cancel) = &self.cancel
        {
            cancel.cancel();
        }
    }

    fn receive_playback_frame(&mut self, ctx: &egui::Context, frame: PlaybackFrame) {
        // Pause freezes the last frame actually presented, not a pending worker
        // frame that happened to race the click. There is only one active job.
        if !self.playing || self.cancel.as_ref().is_none_or(CancelToken::is_cancelled) {
            return;
        }
        self.playback_position_ms = Some(frame.position_ms);
        self.position_ms = frame.position_ms;
        self.set_frame(ctx, &frame.pixels);
    }

    fn set_frame(&mut self, ctx: &egui::Context, pixels: &RgbaImage) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [pixels.width() as usize, pixels.height() as usize],
            pixels.as_raw(),
        );
        if let Some(texture) = &mut self.texture {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.texture = Some(ctx.load_texture(
                "recording-editor-frame",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }

    fn send(&mut self, tx: &Sender<Job>, job: Job) {
        if self.busy || self.picker || self.confirm_replace.is_some() || self.requires_reopen {
            return;
        }
        self.trim_gesture = None;
        self.crop_gesture = None;
        let estimating = matches!(job, Job::Estimate(_));
        let comparing = if let Job::Compare(generation, _) = &job {
            Some(*generation)
        } else {
            None
        };
        if matches!(
            job,
            Job::Apply(_) | Job::Play(..) | Job::SourceFrame(_) | Job::Replace(_)
        ) {
            self.comparison = None;
        }
        let loading_thumbnails = matches!(job, Job::Thumbnails(_));
        let loading_source = matches!(job, Job::SourceFrame(_));
        let playing = matches!(job, Job::Play(..));
        match tx.send(job) {
            Ok(()) => {
                self.busy = true;
                self.estimating = estimating;
                self.comparing = comparing;
                self.loading_thumbnails = loading_thumbnails;
                self.loading_source = loading_source;
                self.playing = playing;
                if playing {
                    self.playback_ended = false;
                    self.playback_audio_enabled = false;
                }
                if loading_thumbnails {
                    self.thumbnail_error = None;
                }
                if estimating {
                    self.estimate = None;
                }
                self.error = None;
                // Automatic source thumbnails must not erase the preceding
                // replacement result while refreshing the rebased timeline.
                if !loading_thumbnails {
                    self.status = if comparing.is_some() {
                        Some("Encoding accepted frame comparison…".into())
                    } else {
                        loading_source.then(|| "Loading uncropped source frame…".into())
                    };
                }
            }
            Err(_) => {
                self.error = Some("Recording editor worker stopped.".into());
                self.cancel = None;
            }
        }
    }

    fn receive(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::Opened(presented, original_path) => {
                // Confirm the session's accepted path, never an older History
                // list hint loaded before the worker opened this recording.
                self.original_path = original_path;
                self.receive(ctx, Event::Presented(Ok(presented)));
            }
            Event::Presented(result) => {
                self.busy = false;
                self.comparison = None;
                match result {
                    Ok(p) => {
                        self.adjusting_crop = false;
                        if self
                            .presented
                            .as_ref()
                            .is_none_or(|old| old.position_ms != p.position_ms)
                        {
                            self.source_texture = None;
                        }
                        if self
                            .presented
                            .as_ref()
                            .is_none_or(|old| old.edit != p.edit || old.export != p.export)
                        {
                            self.estimate = None;
                        }
                        self.set_frame(ctx, &p.frame);
                        self.playback_position_ms = None;
                        self.playback_ended = false;
                        self.start_ms = p.edit.trim_start_ms;
                        self.end_ms = p
                            .edit
                            .trim_end_ms
                            .unwrap_or(p.source.duration_ms.unwrap_or(0));
                        self.crop = p.edit.crop;
                        // A preset's resolved pixels are not a custom size:
                        // retain the preset so later crop changes recompute it.
                        // GIF dimensions are also derived: do not overwrite the
                        // uncapped custom base or compound later width changes.
                        if self.presented.is_none()
                            || (self.output_size.is_some() && p.export.format != ExportFormat::Gif)
                        {
                            self.output_size = p.edit.output_width.zip(p.edit.output_height);
                        }
                        self.audio = p.edit.audio.clone();
                        self.position_ms = p.position_ms;
                        self.gif = p.export.format == ExportFormat::Gif;
                        if self.gif {
                            self.gif_frames_per_second = p.export.frames_per_second;
                        }
                        self.maximum_size = p.export.max_size_bytes.is_some();
                        if let Some(cap) = p.export.max_size_bytes {
                            if self.maximum_bytes() != Some(cap) {
                                self.maximum_value = self.maximum_unit.value(cap);
                            }
                        } else {
                            self.quality = p.export.quality;
                        }
                        // The initial edit includes trusted audio flags.
                        if self.presented.is_none() {
                            if !self.maximum_size {
                                self.maximum_value = "10".into();
                            }
                            self.saved_edit = p.edit.clone();
                            self.saved_export = Some(p.export.clone());
                        }
                        self.presented = Some(p);
                    }
                    Err(error) => {
                        // A failed command restores the accepted still, rather
                        // than labelling transient playback pixels as accepted.
                        if let Some(p) = &self.presented {
                            self.position_ms = p.position_ms;
                            let frame = p.frame.clone();
                            self.set_frame(ctx, &frame);
                        }
                        self.playback_position_ms = None;
                        self.playback_ended = false;
                        self.error = Some(error);
                    }
                }
            }
            Event::Progress(progress) => self.progress = Some(progress),
            Event::Replaced(result) => {
                self.busy = false;
                self.cancel = None;
                self.progress = None;
                match result {
                    Ok((path, presented)) => {
                        // Replacement changes the source, not just accepted edits.
                        // Drop every source-dependent cache and saved baseline.
                        let directory = std::mem::take(&mut self.directory);
                        let stem = std::mem::take(&mut self.stem);
                        let artifact_id = std::mem::take(&mut self.artifact_id);
                        let preview_loop = self.preview_loop.clone();
                        preview_loop.store(false, Ordering::Relaxed);
                        *self = Self {
                            directory,
                            stem,
                            artifact_id,
                            original_path: Some(path.clone()),
                            preview_actual_size: self.preview_actual_size,
                            preview_loop,
                            // Preserve the new source rather than immediately
                            // staging the default 800px cap against a 1200px GIF.
                            gif_maximum_width: (presented.export.format == ExportFormat::Gif
                                && presented.source.width > 800)
                                .then_some(presented.source.width.max(1200)),
                            history_changed: true,
                            original_replaced: true,
                            ..Self::default()
                        };
                        self.receive(ctx, Event::Presented(Ok(presented)));
                        self.status = Some(format!("Replaced original: {}", path.display()));
                    }
                    Err(error) if error.requires_reopen => {
                        self.requires_reopen = true;
                        self.presented = None;
                        self.texture = None;
                        self.source_texture = None;
                        self.thumbnails = None;
                        self.comparison = None;
                        self.estimate = None;
                        self.adjusting_crop = false;
                        self.history_changed = true;
                        self.error = Some(format!(
                            "{} Close and reopen this recording before continuing.",
                            error.message
                        ));
                    }
                    Err(error) => self.error = Some(error.message),
                }
            }
            Event::Saved(result) => {
                self.busy = false;
                self.cancel = None;
                self.progress = None;
                match result {
                    Ok(saved) => {
                        let gif = self
                            .presented
                            .as_ref()
                            .is_some_and(|p| p.export.format == ExportFormat::Gif);
                        let (status, warning) = match saved {
                            SavedRecording::Saved { path, artifact } => {
                                self.history_changed = true;
                                self.saved_path = Some(path);
                                (
                                    recording_editor_ui::saved_message(
                                        gif,
                                        artifact.entry.size_bytes,
                                    ),
                                    None,
                                )
                            }
                            SavedRecording::SavedWithoutHistory { path, warning } => {
                                let status = format!("Saved new copy: {}", path.display());
                                self.saved_path = Some(path);
                                (status, Some(warning))
                            }
                        };
                        if let Some(p) = &self.presented {
                            self.saved_edit = p.edit.clone();
                            self.saved_export = Some(p.export.clone());
                        }
                        self.status = Some(status);
                        self.error = warning;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Estimated(result) => {
                self.busy = false;
                self.estimating = false;
                self.cancel = None;
                match result {
                    Ok(estimate) => {
                        self.estimate = Some(estimate);
                        self.error = None;
                    }
                    Err(error) => {
                        self.estimate = None;
                        self.error = Some(error);
                    }
                }
            }
            Event::Compared(generation, result) => {
                // Each editor owns a private channel/session. A request ID also
                // rejects late replies from an earlier cancelled comparison.
                if self.comparing != Some(generation) {
                    return;
                }
                self.comparing = None;
                self.busy = false;
                self.status = None;
                let cancelled = self
                    .cancel
                    .take()
                    .is_none_or(|cancel| cancel.is_cancelled());
                self.comparison = None;
                if cancelled {
                    self.error = Some("Encoded comparison cancelled.".into());
                } else {
                    match result {
                        Ok(result) if self.can_compare() && self.presented.as_ref().is_some_and(|p|
                            result.revision == p.revision && result.position_ms == p.position_ms
                                && result.export == p.preview_export) => {
                            let textures = std::array::from_fn(|index| {
                                let pixels = &result.frames[index];
                                ctx.load_texture(
                                    format!("recording-comparison-{index}"),
                                    egui::ColorImage::from_rgba_unmultiplied(
                                        [pixels.width() as usize, pixels.height() as usize], pixels.as_raw()),
                                    egui::TextureOptions::LINEAR,
                                )
                            });
                            self.comparison = Some(ComparisonPreview { result, textures });
                            self.comparison_split = 0.5;
                            self.error = None;
                        }
                        Ok(_) => self.error = Some("Encoded comparison no longer matches the accepted frame. Retry after applying edits.".into()),
                        Err(error) => self.error = Some(format!("Encoded comparison failed: {error}")),
                    }
                }
            }
            Event::Thumbnails(result) => {
                self.busy = false;
                self.loading_thumbnails = false;
                self.cancel = None;
                self.error = None;
                match result {
                    Ok(thumbnails) => {
                        let pixels = thumbnails.pixels();
                        self.thumbnails = Some(ctx.load_texture(
                            "recording-source-thumbnails",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [pixels.width() as usize, pixels.height() as usize],
                                pixels.as_raw(),
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                        self.thumbnail_error = None;
                    }
                    Err(error) => self.thumbnail_error = Some(error),
                }
            }
            Event::SourceFrame(result) => {
                self.busy = false;
                self.loading_source = false;
                self.status = None;
                let result = if self.cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
                    Err("Source preview cancelled.".into())
                } else {
                    result
                };
                self.cancel = None;
                match result {
                    Ok(pixels) => {
                        self.source_texture = Some(ctx.load_texture(
                            "recording-crop-source",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [pixels.width() as usize, pixels.height() as usize],
                                pixels.as_raw(),
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                        self.adjusting_crop = true;
                        self.error = None;
                    }
                    Err(error) => {
                        self.adjusting_crop = false;
                        self.error = Some(format!("Source preview failed: {error}"));
                    }
                }
            }
            Event::PlaybackStarted { audio_enabled } => {
                if self.playing
                    && self
                        .cancel
                        .as_ref()
                        .is_some_and(|cancel| !cancel.is_cancelled())
                {
                    self.playback_audio_enabled = audio_enabled;
                    self.status = Some(
                        if audio_enabled {
                            "Playing accepted audio on the default output device."
                        } else if self.preview_sound {
                            "Playing silently: the accepted format or audio mix has no sound."
                        } else {
                            "Playing silently."
                        }
                        .into(),
                    );
                }
            }
            Event::PlaybackFinished(result) => {
                self.busy = false;
                self.playing = false;
                self.cancel = None;
                match result {
                    Ok(end) => {
                        self.error = None;
                        self.playback_ended = matches!(end, PlaybackEnd::Ended);
                        self.status = Some(
                            if self.playback_ended {
                                "Playback ended. Play restarts the accepted trim."
                            } else {
                                "Playback paused."
                            }
                            .into(),
                        );
                    }
                    Err(error) => {
                        self.playback_position_ms = None;
                        self.playback_ended = false;
                        if let Some(p) = &self.presented {
                            self.position_ms = p.position_ms;
                            let frame = p.frame.clone();
                            self.set_frame(ctx, &frame);
                        }
                        self.status = None;
                        self.error = Some(format!("Playback failed: {error}"));
                    }
                }
                self.playback_audio_enabled = false;
                if std::mem::take(&mut self.close_after_playback) {
                    self.request_close();
                }
            }
            Event::Destination(path) => {
                self.picker = false;
                if let Some(path) = path {
                    self.directory = path;
                }
            }
        }
    }

    fn request_close(&mut self) {
        self.trim_gesture = None;
        self.crop_gesture = None;
        self.comparison = None;
        self.confirm_replace = None;
        if self.playing {
            self.close_after_playback = true;
            self.pause_playback();
        } else if self.busy || self.picker {
            self.error =
                Some("Wait for the current operation, or cancel it, before closing.".into());
        } else if self.dirty() {
            self.confirm_close = true;
        } else {
            self.closed = true;
        }
    }

    fn title(&self) -> &'static str {
        if self.busy {
            "Recording editor — Working…"
        } else {
            "Recording editor"
        }
    }
}

pub struct Editor {
    viewport: egui::ViewportId,
    view: Arc<Mutex<View>>,
    // One latest frame, not an unbounded event queue or one closure per frame.
    playback_frame: Arc<Mutex<Option<PlaybackFrame>>>,
    tx: Sender<Job>,
    events: Sender<Event>,
    rx: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
}

fn wake(ctx: &egui::Context, viewport: egui::ViewportId) {
    // Live drains worker results in the root, including while it is hidden.
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.request_repaint_of(viewport);
}

impl Editor {
    pub fn open(
        ctx: &egui::Context,
        history_root: PathBuf,
        artifact_id: String,
        directory: PathBuf,
    ) -> Self {
        let viewport = egui::ViewportId::from_hash_of(("recording-editor", &artifact_id));
        let artifact_id_for_view = artifact_id.clone();
        let (tx, jobs) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let out = events.clone();
        let wake_ctx = ctx.clone();
        let playback_frame = Arc::new(Mutex::new(None));
        let latest_frame = playback_frame.clone();
        let preview_loop = Arc::<AtomicBool>::default();
        let loop_enabled = preview_loop.clone();
        let worker = thread::spawn(move || {
            let mut session = match RecordingEditorSession::open(
                RecordingEditorOpenRequest {
                    history_root,
                    artifact_id,
                },
                MediaToolchain::from_command_names(),
            ) {
                Ok(session) => {
                    let _ = out.send(Event::Opened(
                        Presented::from_session(&session),
                        session.original_save_path().map(std::path::Path::to_owned),
                    ));
                    Some(session)
                }
                Err(error) => {
                    let _ = out.send(Event::Presented(Err(error)));
                    None
                }
            };
            wake(&wake_ctx, viewport);
            while let Ok(job) = jobs.recv() {
                let event = match job {
                    Job::Shutdown => break,
                    Job::Apply(request) => Event::Presented(
                        session
                            .as_mut()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| {
                                s.execute_v2(request)?;
                                Ok(Presented::from_session(s))
                            }),
                    ),
                    Job::Save(request, cancel) => Event::Saved(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| {
                                s.save_new(request, &cancel, |progress| {
                                    let _ = out.send(Event::Progress(progress));
                                    wake(&wake_ctx, viewport);
                                })
                            }),
                    ),
                    Job::Replace(cancel) => {
                        let result = if let Some(s) = session.as_mut() {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                s.replace_original(&cancel, |progress| {
                                    let _ = out.send(Event::Progress(progress));
                                    wake(&wake_ctx, viewport);
                                })
                                .map(
                                    |ReplacedRecording::Replaced { path, .. }| {
                                        (path, Presented::from_session(s))
                                    },
                                )
                            }))
                            .unwrap_or_else(|_| {
                                Err(ReplaceOriginalError {
                                    message: "Recording replacement was interrupted.".into(),
                                    requires_reopen: s.requires_reopen(),
                                })
                            })
                        } else {
                            Err(ReplaceOriginalError {
                                message: "Recording editor is unavailable.".into(),
                                requires_reopen: true,
                            })
                        };
                        Event::Replaced(result)
                    }
                    Job::Estimate(cancel) => Event::Estimated(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| s.estimate_save_export(&cancel)),
                    ),
                    Job::Compare(generation, cancel) => Event::Compared(
                        generation,
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| s.export_comparison(&cancel).map(Comparison::from)),
                    ),
                    Job::Thumbnails(cancel) => Event::Thumbnails(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| s.timeline_thumbnails(&cancel)),
                    ),
                    Job::SourceFrame(cancel) => Event::SourceFrame(
                        session
                            .as_ref()
                            .ok_or_else(|| "Recording editor is unavailable.".to_owned())
                            .and_then(|s| s.source_frame(&cancel)),
                    ),
                    Job::Play(position, sound, cancel) => {
                        let result = (|| {
                            let session =
                                session.as_ref().ok_or("Recording editor is unavailable.")?;
                            let mut position = position;
                            let mut started = false;
                            loop {
                                let mut playback = if sound {
                                    session.playback_with_audio(position, &cancel)?
                                } else {
                                    session.playback(position, &cancel)?
                                };
                                if !started {
                                    let _ = out.send(Event::PlaybackStarted {
                                        audio_enabled: playback.audio_enabled(),
                                    });
                                    wake(&wake_ctx, viewport);
                                    started = true;
                                }
                                let mut decoded_frame = false;
                                while let Some(frame) = playback.next_frame()? {
                                    if cancel.is_cancelled() {
                                        break;
                                    }
                                    decoded_frame = true;
                                    let needs_wake = {
                                        let mut latest = latest_frame.lock().unwrap();
                                        let empty = latest.is_none();
                                        *latest = Some(PlaybackFrame {
                                            position_ms: frame.position_ms,
                                            pixels: frame.pixels(),
                                        });
                                        empty
                                    };
                                    if needs_wake {
                                        wake(&wake_ctx, viewport);
                                    }
                                }
                                // Never restart an empty stream or a cancelled/failed
                                // decoder. Drop finishes teardown before the next lap.
                                if cancel.is_cancelled()
                                    || !decoded_frame
                                    || !loop_enabled.load(Ordering::Relaxed)
                                {
                                    break;
                                }
                                position = session.snapshot().edit.trim_start_ms;
                            }
                            Ok(PlaybackEnd::Ended)
                        })();
                        // Cancellation is expected on Pause/close/focus loss.
                        Event::PlaybackFinished(if cancel.is_cancelled() {
                            Ok(PlaybackEnd::Paused)
                        } else {
                            result
                        })
                    }
                };
                if out.send(event).is_err() {
                    break;
                }
                wake(&wake_ctx, viewport);
            }
        });
        Self {
            viewport,
            view: Arc::new(Mutex::new(View {
                busy: true,
                preview_loop,
                artifact_id: artifact_id_for_view,
                stem: format!(
                    "Captures_{}_edited",
                    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
                ),
                directory,
                ..View::default()
            })),
            playback_frame,
            tx,
            events,
            rx,
            worker: Some(worker),
        }
    }

    pub fn focus(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(self.viewport, egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(self.viewport, egui::ViewportCommand::Focus);
        wake(ctx, self.viewport);
    }

    pub fn closed(&self) -> bool {
        self.view.lock().unwrap().closed
    }
    pub fn take_history_changed(&self) -> bool {
        std::mem::take(&mut self.view.lock().unwrap().history_changed)
    }

    pub fn take_original_replaced(&self) -> bool {
        std::mem::take(&mut self.view.lock().unwrap().original_replaced)
    }

    pub fn receive(&self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            let mut view = self.view.lock().unwrap();
            // Drain the final frame before EOF; completion never overtakes it.
            if let Some(frame) = self.playback_frame.lock().unwrap().take() {
                view.receive_playback_frame(ctx, frame);
            }
            let opening = view.presented.is_none();
            let replaced = matches!(&event, Event::Replaced(Ok(_)));
            view.receive(ctx, event);
            if (opening || replaced) && view.presented.is_some() {
                view.request_thumbnails(&self.tx);
            }
            wake(ctx, self.viewport);
        }
        if let Some(frame) = self.playback_frame.lock().unwrap().take() {
            self.view.lock().unwrap().receive_playback_frame(ctx, frame);
            wake(ctx, self.viewport);
        }
    }

    pub fn flush(&self, ctx: &egui::Context) -> Result<(), String> {
        self.receive(ctx);
        let mut view = self.view.lock().unwrap();
        view.pause_playback();
        if !view.closed && (view.busy || view.picker || view.dirty()) {
            let error = "Recording edits are not saved as drafts. Save a new copy or close the recording editor before quitting.".to_owned();
            view.error = Some(error.clone());
            drop(view);
            self.focus(ctx);
            return Err(error);
        }
        Ok(())
    }

    pub fn show(&self, ctx: &egui::Context, tokens: &Tokens) {
        if self.closed() {
            return;
        }
        let state = self.view.clone();
        let tx = self.tx.clone();
        let events = self.events.clone();
        let tokens = tokens.clone();
        let viewport = self.viewport;
        let title = self.view.lock().unwrap().title();
        ctx.show_viewport_deferred(
            viewport,
            egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([960., 760.])
                .with_min_inner_size([760., 580.]),
            move |ui, _| {
                let mut view = state.lock().unwrap();
                if ui.input(|i| !i.focused || i.viewport().minimized == Some(true)) {
                    view.pause_playback();
                }
                if ui.input(|i| i.viewport().close_requested()) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    view.request_close();
                }
                ui.push_id(viewport, |ui| {
                    show(ui, &tokens, &mut view, &tx, &events, viewport)
                });
                if ui.input(|i| i.viewport().title.as_deref() != Some(view.title())) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title(view.title().into()));
                }
                if view.closed {
                    wake(ui.ctx(), viewport);
                }
            },
        );
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        if let Some(cancel) = &self.view.lock().unwrap().cancel {
            cancel.cancel();
        }
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn show_crop_overlay(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, image: egui::Rect) {
    let Some(initial) = view.crop else {
        view.crop_gesture = None;
        return;
    };
    let Some(presented) = &view.presented else {
        return;
    };
    let (width, height) = (presented.source.width, presented.source.height);
    if initial
        .after_drag(width, height, CropDragHandle::Move, 0., 0., false)
        .is_none()
    {
        view.crop_gesture = None;
        return;
    }
    let to_view = |crop: CropRect| {
        egui::Rect::from_min_max(
            image.min
                + egui::vec2(
                    crop.x as f32 / width as f32 * image.width(),
                    crop.y as f32 / height as f32 * image.height(),
                ),
            image.min
                + egui::vec2(
                    (f64::from(crop.x) + f64::from(crop.width)) as f32 / width as f32
                        * image.width(),
                    (f64::from(crop.y) + f64::from(crop.height)) as f32 / height as f32
                        * image.height(),
                ),
        )
    };
    let handle_positions = |crop: CropRect| {
        let rect = to_view(crop);
        [
            (CropDragHandle::NorthWest, rect.left_top(), "Crop top left"),
            (
                CropDragHandle::NorthEast,
                rect.right_top(),
                "Crop top right",
            ),
            (
                CropDragHandle::SouthEast,
                rect.right_bottom(),
                "Crop bottom right",
            ),
            (
                CropDragHandle::SouthWest,
                rect.left_bottom(),
                "Crop bottom left",
            ),
            (CropDragHandle::North, rect.center_top(), "Crop top"),
            (CropDragHandle::East, rect.right_center(), "Crop right"),
            (CropDragHandle::South, rect.center_bottom(), "Crop bottom"),
            (CropDragHandle::West, rect.left_center(), "Crop left"),
        ]
    };
    let enabled = ui.is_enabled()
        && !view.busy
        && !view.picker
        && !view.confirm_close
        && ui.input(|input| input.focused)
        && !egui::Popup::is_any_open(ui.ctx());
    if !enabled
        || view
            .crop_gesture
            .as_ref()
            .is_some_and(|drag| drag.image != image)
    {
        view.crop_gesture = None;
    }
    let sense = if enabled {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let pending_text = ui
        .ctx()
        .memory(|memory| memory.focused())
        .filter(|id| egui::TextEdit::load_state(ui.ctx(), *id).is_some());
    let mut responses = vec![(
        CropDragHandle::Move,
        ui.interact(to_view(initial), ui.scope_id().with("Move crop"), sense)
            .on_hover_cursor(egui::CursorIcon::Move),
    )];
    responses[0]
        .1
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "Move crop"));
    let hit_size = tokens.number("s-5");
    for (handle, center, label) in handle_positions(initial) {
        let response = ui
            .interact(
                egui::Rect::from_center_size(center, egui::Vec2::splat(hit_size)),
                ui.scope_id().with(label),
                sense,
            )
            .on_hover_text(format!(
                "{label}: drag or use arrows; Shift moves 10 source pixels."
            ));
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
        responses.push((handle, response));
    }
    if enabled && ui.ctx().current_pass_index() == 0 {
        for event in ui.input(|input| input.events.clone()) {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => {
                    view.crop_gesture = None;
                    if !ui.clip_rect().contains(pos) {
                        continue;
                    }
                    let handle = handle_positions(view.crop.unwrap())
                        .into_iter()
                        .find(|(_, center, _)| {
                            egui::Rect::from_center_size(*center, egui::Vec2::splat(hit_size))
                                .contains(pos)
                        })
                        .map(|(handle, _, _)| handle)
                        .or_else(|| {
                            to_view(view.crop.unwrap())
                                .contains(pos)
                                .then_some(CropDragHandle::Move)
                        });
                    if let Some(handle) = handle {
                        if let Some(id) = pending_text {
                            // Numeric fields commit later in this UI pass. Let
                            // that commit finish before capturing drag geometry.
                            ui.memory_mut(|memory| memory.surrender_focus(id));
                            continue;
                        }
                        if let Some((_, response)) =
                            responses.iter().find(|(candidate, _)| *candidate == handle)
                        {
                            response.request_focus();
                        }
                        view.crop_gesture = Some(CropGesture {
                            initial: view.crop.unwrap(),
                            handle,
                            origin: pos,
                            image,
                            locked: !view.crop_aspect_unlocked,
                        });
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    if let Some(drag) = &view.crop_gesture {
                        let delta = pos - drag.origin;
                        if let Some(crop) = drag.initial.after_drag(
                            width,
                            height,
                            drag.handle,
                            f64::from(delta.x / image.width()) * f64::from(width),
                            f64::from(delta.y / image.height()) * f64::from(height),
                            drag.locked,
                        ) {
                            view.crop = Some(crop);
                        }
                    }
                }
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                }
                | egui::Event::PointerGone
                | egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } => view.crop_gesture = None,
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.ctrl && !modifiers.alt && !modifiers.command => {
                    let step = if modifiers.shift { 10. } else { 1. };
                    let delta = match key {
                        egui::Key::ArrowLeft => (-step, 0.),
                        egui::Key::ArrowRight => (step, 0.),
                        egui::Key::ArrowUp => (0., -step),
                        egui::Key::ArrowDown => (0., step),
                        _ => continue,
                    };
                    if let Some((handle, _)) =
                        responses.iter().find(|(_, response)| response.has_focus())
                    {
                        ui.input_mut(|input| input.consume_key(modifiers, key));
                        view.crop_gesture = None;
                        if let Some(crop) = view.crop.unwrap().after_drag(
                            width,
                            height,
                            *handle,
                            delta.0,
                            delta.1,
                            !view.crop_aspect_unlocked,
                        ) {
                            view.crop = Some(crop);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let selected = to_view(view.crop.unwrap()).intersect(image);
    for dim in [
        egui::Rect::from_min_max(image.min, egui::pos2(image.right(), selected.top())),
        egui::Rect::from_min_max(egui::pos2(image.left(), selected.bottom()), image.max),
        egui::Rect::from_min_max(
            egui::pos2(image.left(), selected.top()),
            selected.left_bottom(),
        ),
        egui::Rect::from_min_max(
            selected.right_top(),
            egui::pos2(image.right(), selected.bottom()),
        ),
    ] {
        ui.painter()
            .rect_filled(dim, 0., tokens.color("surface-sunken").gamma_multiply(0.75));
    }
    ui.painter().rect_stroke(
        selected,
        0.,
        egui::Stroke::new(tokens.number("s-1"), tokens.color("theme-accent")),
        egui::StrokeKind::Inside,
    );
    // `.editor-crop-box > span`: the source-pixel size in an accent pill.
    let crop = view.crop.unwrap();
    let galley = ui.painter().layout_no_wrap(
        format!("{} × {}", crop.width, crop.height),
        egui::FontId::proportional(tokens.number("text-2xs")),
        tokens.color("theme-accent-ink"),
    );
    let pill = egui::Align2::CENTER_TOP.anchor_size(
        selected.center_top() + egui::vec2(0., tokens.number("s-3")),
        galley.size() + egui::vec2(tokens.number("s-3") * 2., 6.),
    );
    if selected.contains_rect(pill) {
        ui.painter()
            .rect_filled(pill, tokens.number("r-xs"), tokens.color("theme-accent"));
        ui.painter().galley(
            pill.min + egui::vec2(tokens.number("s-3"), 3.),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        probe(ui, "Crop size", pill);
    }
    for (_, center, _) in handle_positions(crop) {
        let handle = egui::Rect::from_center_size(center, egui::Vec2::splat(tokens.number("s-4")));
        ui.painter()
            .rect_filled(handle, tokens.number("r-xs"), tokens.color("theme-accent"));
        ui.painter().rect_stroke(
            handle,
            tokens.number("r-xs"),
            egui::Stroke::new(tokens.number("s-1"), tokens.color("surface-raised")),
            egui::StrokeKind::Inside,
        );
    }
}

// ----------------------------------------------------------------- layout ---
//
// The window follows the shipping recording editor (`App.tsx` RecordingEditor,
// `styles/editor-video.css`): a heading, a Preview card with its toolbar and
// overlay play button, a Timeline card, option cards and a fixed save footer.

const LAYOUT_PROBE_ENV: &str = "CAPTURES_NATIVE_LAYOUT_PROBE";

type ProbeRects = BTreeMap<String, [i32; 8]>;

thread_local! {
    /// Named control geometry for smoke and unit tests: `[rect, clip]` in
    /// window points. `None` unless a caller opts in; each pass starts afresh.
    static PROBE: RefCell<Option<ProbeRects>> = const { RefCell::new(None) };
}

fn probe_env() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os(LAYOUT_PROBE_ENV).is_some())
}

fn probe(ui: &egui::Ui, name: &str, rect: egui::Rect) {
    PROBE.with_borrow_mut(|controls| {
        if let Some(controls) = controls {
            let clip = ui.clip_rect();
            controls.insert(
                name.to_owned(),
                [
                    rect.min.x, rect.min.y, rect.max.x, rect.max.y, clip.min.x, clip.min.y,
                    clip.max.x, clip.max.y,
                ]
                .map(|value| value.clamp(-1e6, 1e6).round() as i32),
            );
        }
    });
}

fn text(tokens: &Tokens, value: impl Into<String>, size: &str, color: &str) -> egui::RichText {
    egui::RichText::new(value)
        .size(tokens.number(size))
        .color(tokens.color(color))
}

/// `.editor-card`: raised surface, subtle border, large radius.
fn card<R>(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    fill: &str,
    border: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(tokens.color(fill))
        .stroke(egui::Stroke::new(1., tokens.color(border)))
        .corner_radius(tokens.number("r-xl") as u8)
        .inner_margin(tokens.number("s-6") as i8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn card_title(ui: &mut egui::Ui, tokens: &Tokens, title: &str) {
    ui.label(text(tokens, title, "text-lg", "text").strong());
}

/// `.editor-field > span`: a small subtle label above its control.
fn field_label(ui: &mut egui::Ui, tokens: &Tokens, label: &str) {
    ui.label(text(tokens, label, "text-xs", "text-subtle"));
}

/// A shipping `CustomSelect` whose listbox shows each option's description.
/// The trigger is probed as `name` and each row as `name/label`.
fn select<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    name: &str,
    width: f32,
    value: &mut T,
    options: &[(T, String, &str)],
) -> bool {
    select_styled(
        ui,
        tokens,
        name,
        width,
        value,
        options,
        crate::primitives::SelectStyle::Field,
    )
}

fn select_styled<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    name: &str,
    width: f32,
    value: &mut T,
    options: &[(T, String, &str)],
    style: crate::primitives::SelectStyle,
) -> bool {
    let choices: Vec<_> = options
        .iter()
        .map(|(option, label, description)| {
            crate::primitives::SelectOption::new(*option, label.as_str()).description(description)
        })
        .collect();
    let output = crate::primitives::Select::new(("recording-select", name), name, width)
        .style(style)
        .show(ui, tokens, &choices, value);
    for ((_, label, _), row) in options.iter().zip(&output.rows) {
        probe(ui, &format!("{name}/{label}"), *row);
    }
    probe(ui, name, output.response.rect);
    match output.chosen {
        Some(chosen) => {
            *value = chosen;
            true
        }
        None => false,
    }
}

/// `.recording-preview-loop`-style quiet toggle for the preview toolbar.
fn toolbar_toggle(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    label: &str,
    on: bool,
    enabled: bool,
) -> egui::Response {
    let button = egui::Button::new(text(
        tokens,
        label,
        "text-sm",
        if on { "text" } else { "text-subtle" },
    ))
    .fill(if on {
        tokens.color("surface-active")
    } else {
        egui::Color32::TRANSPARENT
    })
    .stroke(if on {
        egui::Stroke::new(1., tokens.color("border"))
    } else {
        egui::Stroke::NONE
    })
    .corner_radius(tokens.number("r-md"))
    .min_size(egui::vec2(0., tokens.number("h-sm")))
    .selected(on);
    let response = ui.add_enabled(enabled, button);
    probe(ui, label, response.rect);
    response
}

/// `.recording-preview-loop`: the toggle with its circular-arrow icon, drawn
/// because the bundled fonts have no loop glyph.
fn loop_toggle(ui: &mut egui::Ui, tokens: &Tokens, on: bool, enabled: bool) -> egui::Response {
    let label = "Loop preview";
    let font = egui::FontId::proportional(tokens.number("text-sm"));
    let color = tokens.color(if !enabled {
        "text-faint"
    } else if on {
        "text"
    } else {
        "text-subtle"
    });
    let galley = ui.painter().layout_no_wrap(label.into(), font, color);
    let icon = 12.;
    let padding = tokens.number("s-4");
    let gap = tokens.number("s-3");
    let size = egui::vec2(
        padding * 2. + icon + gap + galley.size().x,
        tokens.number("h-sm"),
    );
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, label));
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, tokens.number("r-md"), tokens.color("surface-active"));
        painter.rect_stroke(
            rect,
            tokens.number("r-md"),
            egui::Stroke::new(1., tokens.color("border")),
            egui::StrokeKind::Inside,
        );
    } else if enabled && response.hovered() {
        painter.rect_filled(rect, tokens.number("r-md"), tokens.color("surface-hover"));
    }
    if response.has_focus() {
        crate::primitives::focus_indicated(ui.ctx());
        painter.rect_stroke(
            rect,
            tokens.number("r-md"),
            egui::Stroke::new(1., tokens.color("theme-accent")),
            egui::StrokeKind::Outside,
        );
    }
    let center = egui::pos2(rect.left() + padding + icon / 2., rect.center().y);
    let radius = icon / 2. - 1.;
    let stroke = egui::Stroke::new(1.4, color);
    let points: Vec<_> = (0..=20)
        .map(|step| {
            let angle = -0.35 + step as f32 / 20. * 1.6 * std::f32::consts::PI;
            center + radius * egui::vec2(angle.cos(), angle.sin())
        })
        .collect();
    let tip = *points.last().unwrap();
    painter.add(egui::Shape::line(points, stroke));
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip + egui::vec2(-3.2, 0.2),
            tip + egui::vec2(1.6, -2.6),
            tip + egui::vec2(1.2, 2.8),
        ],
        color,
        egui::Stroke::NONE,
    ));
    painter.galley(
        egui::pos2(
            rect.left() + padding + icon + gap,
            rect.center().y - galley.size().y / 2.,
        ),
        galley,
        color,
    );
    probe(ui, label, rect);
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// `.editor-segmented` Fit / 100%: sunken track with a raised active segment.
fn preview_size_segmented(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View) {
    let height = tokens.number("h-sm") + 6.;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(132., height), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, tokens.number("r-lg"), tokens.color("surface-sunken"));
    ui.painter().rect_stroke(
        rect,
        tokens.number("r-lg"),
        egui::Stroke::new(1., tokens.color("border-subtle")),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(3.);
    let enabled = view.texture.is_some() || view.source_texture.is_some();
    for (index, (label, actual)) in [("Fit", false), ("100%", true)].into_iter().enumerate() {
        let segment = egui::Rect::from_min_size(
            inner.min + egui::vec2(inner.width() / 2. * index as f32, 0.),
            egui::vec2(inner.width() / 2., inner.height()),
        );
        let active = view.preview_actual_size == actual;
        let response = ui
            .interact(
                segment,
                ui.scope_id().with(("recording-preview-size", label)),
                if enabled {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            )
            .on_hover_text(if actual {
                "One decoded image pixel per screen point. Scroll to see overflow; playback may use a reduced-size frame."
            } else {
                "Fit the decoded frame within the preview."
            });
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, active, label)
        });
        if active {
            ui.painter().rect_filled(
                segment,
                tokens.number("r-sm"),
                tokens.color("surface-raised"),
            );
            ui.painter().rect_stroke(
                segment,
                tokens.number("r-sm"),
                egui::Stroke::new(1., tokens.color("border-subtle")),
                egui::StrokeKind::Inside,
            );
        }
        let color = if !enabled {
            tokens.color("text-faint")
        } else if active || response.hovered() {
            tokens.color("text")
        } else {
            tokens.color("text-subtle")
        };
        ui.painter().text(
            segment.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(tokens.number("text-sm")),
            color,
        );
        if response.has_focus() {
            crate::primitives::focus_indicated(ui.ctx());
            ui.painter().rect_stroke(
                segment,
                tokens.number("r-sm"),
                egui::Stroke::new(1., tokens.color("theme-accent")),
                egui::StrokeKind::Outside,
            );
        }
        probe(ui, label, segment);
        if response.clicked() {
            view.preview_actual_size = actual;
        }
    }
}

/// `.recording-preview-overlay-play`: accent circle over the media.
fn show_overlay_play(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    image: egui::Rect,
    viewport: egui::Rect,
) {
    let size = tokens.number("s-12") - tokens.number("s-4");
    let center = if view.comparison.is_some() {
        // Keep play clear of the centered comparison divider, as shipping does.
        egui::pos2(
            image.center().x,
            image.bottom() - tokens.number("s-5") - tokens.number("s-10") - size / 2.,
        )
    } else {
        image.center()
    };
    let center = center.clamp(
        viewport.min + egui::Vec2::splat(size / 2.),
        viewport.max - egui::Vec2::splat(size / 2.),
    );
    let rect = egui::Rect::from_center_size(center, egui::Vec2::splat(size));
    let pausing = view.playing && view.cancel.as_ref().is_some_and(CancelToken::is_cancelled);
    let enabled = if view.playing {
        !pausing
    } else {
        !view.busy
            && !view.picker
            && !view.confirm_close
            && !view.adjusting_crop
            && view.presented.is_some()
            && !view.unapplied()
    };
    let label = if view.playing {
        if pausing {
            "Pausing…"
        } else {
            "Pause preview"
        }
    } else {
        "Play preview"
    };
    let response = ui
        .interact(
            rect,
            ui.scope_id().with("recording-overlay-play"),
            if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        )
        .on_hover_text(if view.playing {
            "Pause the preview."
        } else if view.unapplied() {
            "Apply staged edits before playing."
        } else {
            "Play the accepted trim and mix; Sound is off by default. Motion preview fits within 1280 × 720."
        });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    probe(ui, "Play preview", rect);
    probe(ui, label, rect);
    // Shipping hides the pause affordance while playing until the media is hovered.
    let hovered = ui.rect_contains_pointer(viewport) || response.has_focus();
    let opacity = if view.playing && !hovered {
        0.
    } else if enabled || view.playing {
        1.
    } else {
        0.45
    };
    if opacity > 0. {
        let painter = ui.painter();
        let fill = if response.hovered() && enabled {
            tokens.color("theme-accent-hover")
        } else {
            tokens.color("theme-accent")
        };
        painter.circle_filled(center, size / 2., fill.gamma_multiply(opacity));
        painter.circle_stroke(
            center,
            size / 2.,
            egui::Stroke::new(
                1.,
                tokens.color("glass-border-strong").gamma_multiply(opacity),
            ),
        );
        let ink = tokens.color("theme-accent-ink").gamma_multiply(opacity);
        let glyph = tokens.number("s-4") + 1.;
        if view.playing {
            for offset in [-glyph / 2., glyph / 2.] {
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        center + egui::vec2(offset * 0.7, 0.),
                        egui::vec2(glyph * 0.45, glyph * 1.8),
                    ),
                    1.,
                    ink,
                );
            }
        } else {
            let tip = center + egui::vec2(glyph + 1., 0.);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + egui::vec2(-glyph * 0.7 + 1., -glyph),
                    tip,
                    center + egui::vec2(-glyph * 0.7 + 1., glyph),
                ],
                ink,
                egui::Stroke::NONE,
            ));
        }
        if response.has_focus() {
            crate::primitives::focus_indicated(ui.ctx());
            painter.circle_stroke(
                center,
                size / 2. + 3.,
                egui::Stroke::new(2., tokens.color("text")),
            );
        }
    }
    if response.clicked() {
        if view.playing {
            view.pause_playback();
        } else {
            view.request_playback(tx);
        }
        ui.ctx().request_repaint();
    }
}

/// The shipping timeline track: filmstrip, dimmed exclusions, accent trim
/// handles and a playhead. Returns a source position when a scrub completes.
///
/// `rect` is the whole row. Trim grips sit outside the selected interval so
/// even a 1 ms selection leaves distinct start/end hit regions inside it.
fn show_trim_timeline(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    duration: u64,
    rect: egui::Rect,
) -> Option<u64> {
    let grip_width = tokens.number("s-6");
    let track = rect.shrink2(egui::vec2(grip_width, 0.));
    if track.width() <= 0. || duration == 0 {
        view.trim_gesture = None;
        view.scrub = None;
        return None;
    }
    let handles = |start: u64, end: u64| {
        let start = track.left()
            + timeline_ratio(start as f64, duration as f64).unwrap_or(0.) as f32 * track.width();
        let end = track.left()
            + timeline_ratio(end as f64, duration as f64).unwrap_or(1.) as f32 * track.width();
        [
            egui::Rect::from_min_max(
                egui::pos2(start - grip_width, rect.top()),
                egui::pos2(start, rect.bottom()),
            ),
            egui::Rect::from_min_max(
                egui::pos2(end, rect.top()),
                egui::pos2(end + grip_width, rect.bottom()),
            ),
        ]
    };
    let enabled = ui.is_enabled()
        && !view.busy
        && !view.picker
        && !view.confirm_close
        && view.start_ms < view.end_ms
        && view.end_ms <= duration
        && ui.input(|input| input.focused)
        && !egui::Popup::is_any_open(ui.ctx());
    // Clicking the track scrubs the accepted still, as the shipping track
    // seeks its video. Staged edits must be applied first, as for Seek.
    let scrub_enabled = enabled && !view.unapplied() && view.presented.is_some();
    let handle_rects = handles(view.start_ms, view.end_ms);
    let responses = ["Trim start", "Trim end"].map(|label| {
        let index = usize::from(label == "Trim end");
        ui.interact(handle_rects[index], ui.scope_id().with(label),
                    if enabled { egui::Sense::click_and_drag() } else { egui::Sense::hover() })
            .on_hover_text(format!("{label}: {}. Drag or use arrow keys/Page Up/Page Down. Apply edits to update the preview.",
                recording_editor_ui::format_editor_time(if index == 0 { view.start_ms } else { view.end_ms }, duration)))
    });
    if !enabled
        || view
            .trim_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.track != track)
    {
        // Match shipping pointer cancellation: keep the last staged values.
        view.trim_gesture = None;
    }
    if !scrub_enabled || view.scrub.is_some_and(|scrub| scrub != track) {
        view.scrub = None;
    }
    let time_at = |x: f32| {
        timeline_time_at_client_x(
            f64::from(x),
            f64::from(track.left()),
            f64::from(track.width()),
            duration as f64,
        )
        .map(|time| (time.round() as u64).min(duration.saturating_sub(1)))
    };
    let mut seek = None;
    if enabled && ui.ctx().current_pass_index() == 0 {
        for event in ui.input(|input| input.events.clone()) {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => {
                    view.trim_gesture = None;
                    view.scrub = None;
                    if !ui.clip_rect().contains(pos) {
                        continue;
                    }
                    if let Some(index) = handles(view.start_ms, view.end_ms)
                        .iter()
                        .position(|rect| rect.contains(pos))
                    {
                        let edge = if index == 0 {
                            TimelineTrimEdge::Start
                        } else {
                            TimelineTrimEdge::End
                        };
                        if let Some(drag) = TimelineTrimDrag::begin(
                            edge,
                            f64::from(pos.x),
                            view.start_ms as f64,
                            view.end_ms as f64,
                            duration as f64,
                        ) {
                            responses[index].request_focus();
                            view.trim_gesture = Some(TrimGesture { edge, drag, track });
                        }
                    } else if scrub_enabled
                        && track.contains(pos)
                        && let Some(time) = time_at(pos.x)
                    {
                        view.position_ms = time;
                        view.scrub = Some(track);
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    if let Some(gesture) = &mut view.trim_gesture
                        && let Some(update) = gesture.drag.update(
                            f64::from(pos.x),
                            f64::from(track.left()),
                            f64::from(track.width()),
                        )
                    {
                        gesture.drag = update.drag;
                        let value = update.time_ms.round() as u64;
                        match gesture.edge {
                            TimelineTrimEdge::Start => view.start_ms = value,
                            TimelineTrimEdge::End => view.end_ms = value,
                        }
                    } else if view.scrub.is_some()
                        && let Some(time) = time_at(pos.x)
                    {
                        view.position_ms = time;
                    }
                }
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } => {
                    view.trim_gesture = None;
                    if view.scrub.take().is_some() {
                        seek = Some(view.position_ms);
                    }
                }
                egui::Event::PointerGone => {
                    view.trim_gesture = None;
                    if view.scrub.take().is_some() {
                        seek = Some(view.position_ms);
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } => {
                    view.trim_gesture = None;
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers == egui::Modifiers::NONE => {
                    let step = if duration < 60_000 { 1. } else { 10. };
                    let delta = match key {
                        egui::Key::ArrowLeft | egui::Key::ArrowDown => -step,
                        egui::Key::ArrowRight | egui::Key::ArrowUp => step,
                        egui::Key::PageDown => -1000.,
                        egui::Key::PageUp => 1000.,
                        _ => continue,
                    };
                    if let Some(index) = responses.iter().position(|response| response.has_focus())
                    {
                        ui.input_mut(|input| input.consume_key(modifiers, key));
                        view.trim_gesture = None;
                        if index == 0 {
                            view.start_ms = (view.start_ms as f64 + delta)
                                .clamp(0., (view.end_ms - 1) as f64)
                                as u64;
                        } else {
                            view.end_ms = (view.end_ms as f64 + delta)
                                .clamp((view.start_ms + 1) as f64, duration as f64)
                                as u64;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let [start, end] = handles(view.start_ms, view.end_ms);
    let overhang = (rect.height() - track.height()).max(0.) / 2. + 3.;
    let frame = rect.shrink2(egui::vec2(0., overhang.min(rect.height() / 4.)));
    let painter = ui.painter();
    painter.rect_filled(frame, tokens.number("r-md"), tokens.color("surface-sunken"));
    painter.rect_stroke(
        frame,
        tokens.number("r-md"),
        egui::Stroke::new(1., tokens.color("border")),
        egui::StrokeKind::Inside,
    );
    let strip = egui::Rect::from_x_y_ranges(
        track.x_range(),
        frame.shrink(tokens.number("s-2")).y_range(),
    );
    if let Some(texture) = &view.thumbnails {
        // Center-crop the strip vertically to the track, preserving thumbnail
        // aspect and the full horizontal source-time mapping.
        let size = texture.size_vec2();
        let uv_height = (strip.height() * size.x / (strip.width() * size.y)).min(1.);
        painter.add(
            egui::epaint::RectShape::filled(strip, tokens.number("r-xs"), egui::Color32::WHITE)
                .with_texture(
                    texture.id(),
                    egui::Rect::from_min_max(
                        egui::pos2(0., (1. - uv_height) / 2.),
                        egui::pos2(1., (1. + uv_height) / 2.),
                    ),
                ),
        );
    } else {
        painter.rect_filled(strip, tokens.number("r-xs"), tokens.color("n-5"));
    }
    // `.timeline-excluded`: trimmed-away time dims toward the canvas.
    let excluded = tokens.color("surface-canvas").gamma_multiply(0.72);
    for dim in [
        egui::Rect::from_min_max(frame.min, egui::pos2(start.right(), frame.bottom())),
        egui::Rect::from_min_max(egui::pos2(end.left(), frame.top()), frame.max),
    ] {
        if dim.width() > 0. {
            painter.rect_filled(dim, tokens.number("r-md"), excluded);
        }
    }
    if view.loading_thumbnails {
        // This status remains readable while the enclosing editing controls
        // are disabled for the serialized thumbnail operation.
        ui.ctx()
            .layer_painter(ui.layer_id())
            .with_clip_rect(ui.clip_rect())
            .text(
                track.center(),
                egui::Align2::CENTER_CENTER,
                "Loading source thumbnails…",
                egui::FontId::proportional(tokens.number("text-sm")),
                tokens.color("text-muted"),
            );
    }
    // `.timeline-playhead`: a 2px line with a cap, over the filmstrip.
    let position = view.position_ms.min(duration);
    let x = track.left()
        + timeline_ratio(position as f64, duration as f64).unwrap_or(0.) as f32 * track.width();
    let line = egui::Rect::from_min_max(
        egui::pos2(x - 1., rect.top()),
        egui::pos2(x + 1., rect.bottom()),
    );
    painter.rect_filled(line, 0., tokens.color("text"));
    painter.rect_filled(
        egui::Rect::from_min_size(egui::pos2(x - 5., rect.top() - 3.), egui::vec2(10., 8.)),
        3.,
        tokens.color("text"),
    );
    probe(ui, "Timeline track", track);
    for (index, grip) in [start, end].into_iter().enumerate() {
        let response = &responses[index];
        if enabled && response.has_focus() {
            // Arrow keys adjust this slider instead of moving egui focus to a
            // neighboring widget before the following key event arrives.
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
        }
        let label = if index == 0 { "Trim start" } else { "Trim end" };
        let value = if index == 0 {
            view.start_ms
        } else {
            view.end_ms
        };
        let time = recording_editor_ui::format_editor_time(value, duration);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Slider,
                enabled,
                format!("{label}: {time}"),
            )
        });
        probe(ui, label, grip);
        // `.timeline-trim-handle`: an 8px accent bar beside the boundary.
        let bar_width = tokens.number("s-4");
        let bar = if index == 0 {
            egui::Rect::from_min_max(egui::pos2(grip.right() - bar_width, rect.top()), grip.max)
        } else {
            egui::Rect::from_min_max(grip.min, egui::pos2(grip.left() + bar_width, rect.bottom()))
        };
        let accent = tokens.color(if enabled {
            "theme-accent"
        } else {
            "text-faint"
        });
        painter.rect_filled(bar, tokens.number("r-xs"), accent);
        let grips = tokens.color("theme-accent-ink").gamma_multiply(0.45);
        for offset in [-tokens.number("s-1"), tokens.number("s-1")] {
            painter.vline(
                bar.center().x + offset,
                egui::Rangef::new(bar.center().y - 7., bar.center().y + 7.),
                egui::Stroke::new(1., grips),
            );
        }
        let active = enabled
            && (response.hovered()
                || response.has_focus()
                || view.trim_gesture.as_ref().is_some_and(|gesture| {
                    (gesture.edge == TimelineTrimEdge::Start) == (index == 0)
                }));
        if response.has_focus() {
            crate::primitives::focus_indicated(ui.ctx());
            painter.rect_stroke(
                bar.expand(2.),
                tokens.number("r-xs"),
                egui::Stroke::new(2., tokens.color("text")),
                egui::StrokeKind::Outside,
            );
        }
        if active {
            // Time bubble above the handle, like the shipping hover label.
            let font = egui::FontId::proportional(tokens.number("text-2xs"));
            let galley = ui
                .painter()
                .layout_no_wrap(time, font, tokens.color("theme-accent-ink"));
            let size = galley.size() + egui::vec2(tokens.number("s-3") * 2., 6.);
            let top = rect.top() - 5. - size.y;
            let bubble = if index == 0 {
                egui::Rect::from_min_size(egui::pos2(bar.left(), top), size)
            } else {
                egui::Rect::from_min_size(egui::pos2(bar.right() - size.x, top), size)
            };
            let layer = ui
                .ctx()
                .layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    ui.scope_id().with(label),
                ))
                .with_clip_rect(ui.clip_rect());
            layer.rect_filled(bubble, tokens.number("r-xs"), tokens.color("theme-accent"));
            layer.galley(
                bubble.min + egui::vec2(tokens.number("s-3"), 3.),
                galley,
                egui::Color32::PLACEHOLDER,
            );
        }
        if enabled && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }
    seek
}

fn show(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    events: &Sender<Event>,
    viewport: egui::ViewportId,
) {
    if probe_env() || PROBE.with_borrow(Option::is_some) {
        PROBE.with_borrow_mut(|controls| *controls = Some(BTreeMap::new()));
    }
    show_footer(ui, tokens, view, tx, events, viewport);
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(tokens.color("surface-canvas")))
        .show(ui, |ui| {
            if view.confirm_replace.is_some() || view.requires_reopen {
                ui.disable();
            }
            crate::primitives::scroll_area(
                ui,
                tokens,
                egui::ScrollArea::vertical()
                    .id_salt("recording-editor-page")
                    .auto_shrink([false, false]),
                |ui| {
                    probe(ui, "Page", ui.clip_rect());
                    let pad = tokens.number("s-8");
                    let side = ((ui.available_width() - 1220.) / 2.).max(pad);
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: side as i8,
                            right: side as i8,
                            top: pad as i8,
                            bottom: pad as i8,
                        })
                        .show(ui, |ui| show_page(ui, tokens, view, tx));
                },
            );
        });
    if view.unapplied() {
        view.comparison = None;
    }
    if probe_env() {
        let controls = PROBE.with_borrow(Clone::clone);
        if controls.is_some() && controls != view.probe_emitted {
            crate::emit(
                "recording-editor-layout",
                serde_json::json!({"artifact_id": view.artifact_id, "controls": controls}),
            );
            view.probe_emitted = controls;
        }
    }
}

fn show_footer(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    events: &Sender<Event>,
    viewport: egui::ViewportId,
) {
    let margin = egui::Margin::symmetric(tokens.number("s-7") as i8, tokens.number("s-4") as i8);
    egui::Panel::bottom("recording-save")
        .frame(
            egui::Frame::new()
                .fill(tokens.color("surface-raised"))
                .inner_margin(margin),
        )
        .show(ui, |ui| {
            let full = ui.max_rect() + margin;
            if let Some(progress) = &view.progress {
                // `.recording-export-progress`: a 3px accent bar on the top edge.
                let bar = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 3.));
                ui.painter().rect_filled(bar, 0., tokens.color("surface-sunken"));
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        bar.min,
                        egui::vec2(
                            bar.width() * f32::from(progress.completed_per_mille.min(1000)) / 1000.,
                            bar.height(),
                        ),
                    ),
                    0.,
                    tokens.color("theme-accent"),
                );
            }
            ui.spacing_mut().item_spacing.y = tokens.number("s-4");
            if view.confirm_close {
                card(ui, tokens, "caution-surface", "border-subtle", |ui| {
                    ui.label(
                        text(
                            tokens,
                            "Discard unsaved recording edits? The original recording is unchanged.",
                            "text-sm",
                            "text",
                        )
                        .strong(),
                    );
                    ui.horizontal(|ui| {
                        let keep = ui.button("Keep editing");
                        probe(ui, "Keep editing", keep.rect);
                        if keep.clicked() {
                            view.confirm_close = false;
                        }
                        let discard = ui.button("Discard edits and close");
                        probe(ui, "Discard edits and close", discard.rect);
                        if discard.clicked() {
                            view.closed = true;
                        }
                    });
                });
            }
            if let Some(confirmation) = &view.confirm_replace {
                let path = confirmation.path.display().to_string();
                card(ui, tokens, "caution-surface", "border-subtle", |ui| {
                    ui.spacing_mut().item_spacing.y = tokens.number("s-3");
                    ui.label(text(tokens, "Replace the original recording?", "text-md", "text").strong());
                    ui.label(text(tokens, path, "text-sm", "text-muted").monospace());
                    ui.label(text(tokens, "Replaces this file and its History recovery copy with the accepted edits. This cannot be undone.", "text-sm", "text"));
                    ui.label(text(tokens, "A matching recovery copy is required. Cancellation stops preparation, not an update already being committed.", "text-xs", "text-subtle"));
                    ui.horizontal(|ui| {
                        let cancel = ui.button("Cancel replacement");
                        probe(ui, "Cancel replacement", cancel.rect);
                        if cancel.clicked() {
                            view.confirm_replace = None;
                        }
                        let confirm = ui.add(primary_button(tokens, "Replace original"));
                        probe(ui, "Replace original", confirm.rect);
                        if confirm.clicked() {
                            view.confirm_replacement(tx);
                        }
                    });
                });
            }
            if view.confirm_replace.is_some() || view.requires_reopen {
                ui.disable();
            }
            let available = ui.available_width();
            let wide = available >= 820.;
            let editable = !view.busy && !view.picker && !view.confirm_close;
            let filename_width = if wide {
                (available - 500. - tokens.number("s-6")).clamp(280., 420.)
            } else {
                available
            };
            let filename = |ui: &mut egui::Ui, view: &mut View| {
                ui.add_enabled_ui(editable, |ui| {
                    show_filename(ui, tokens, view, events, viewport, filename_width)
                });
            };
            let actions = |ui: &mut egui::Ui, view: &mut View| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    show_save_actions(ui, tokens, view, tx, editable)
                });
            };
            if wide {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(filename_width);
                        filename(ui, view);
                    });
                    ui.add_space(tokens.number("s-6"));
                    actions(ui, view);
                });
            } else {
                filename(ui, view);
                actions(ui, view);
            }
        });
}

fn primary_button(tokens: &Tokens, label: &str) -> egui::Button<'static> {
    egui::Button::new(text(tokens, label, "text-md", "theme-accent-ink").strong())
        .fill(tokens.color("theme-accent"))
        .stroke(egui::Stroke::NONE)
        .corner_radius(tokens.number("r-md"))
        .min_size(egui::vec2(0., tokens.number("h-md")))
}

/// `.recording-filename`: label, destination folder and the filename field
/// with its attached format select.
fn show_filename(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    events: &Sender<Event>,
    viewport: egui::ViewportId,
    width: f32,
) {
    ui.spacing_mut().item_spacing.y = tokens.number("s-2");
    ui.set_width(width);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tokens.number("s-2");
        ui.label(text(tokens, "Filename", "text-xs", "text-subtle"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let change = ui
                .add(egui::Button::new(text(tokens, "Change…", "text-2xs", "text")).small())
                .on_hover_text("Choose the folder for the new copy.");
            probe(ui, "Change…", change.rect);
            if change.clicked() {
                view.picker = true;
                let events = events.clone();
                let ctx = ui.ctx().clone();
                let directory = view.directory.clone();
                thread::spawn(move || {
                    let folder = rfd::FileDialog::new()
                        .set_title("Choose save location")
                        .set_directory(&directory)
                        .pick_folder();
                    let _ = events.send(Event::Destination(folder));
                    wake(&ctx, viewport);
                });
            }
            let directory = view.directory.display().to_string();
            let location = ui
                .add(
                    egui::Label::new(
                        text(tokens, directory.clone(), "text-2xs", "text-subtle").monospace(),
                    )
                    .truncate(),
                )
                .on_hover_text(directory);
            probe(ui, "Save location", location.rect);
            ui.label(text(tokens, "Saving to", "text-2xs", "text-faint"));
        });
    });
    let height = tokens.number("h-md");
    let (field, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let format_width = 76.;
    let stem_rect = egui::Rect::from_min_max(
        field.min,
        egui::pos2(field.right() - format_width, field.bottom()),
    );
    let format_rect =
        egui::Rect::from_min_max(egui::pos2(stem_rect.right(), field.top()), field.max);
    // `.recording-filename-input`: one field with the format select attached.
    ui.painter()
        .rect_filled(field, tokens.number("r-md"), tokens.color("surface-field"));
    let mut stem_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(stem_rect.shrink2(egui::vec2(tokens.number("s-4"), 0.)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let response = stem_ui.add(
        egui::TextEdit::singleline(&mut view.stem)
            .frame(egui::Frame::NONE)
            .font(egui::FontId::proportional(tokens.number("text-sm")))
            .desired_width(stem_rect.width() - tokens.number("s-4") * 2.)
            .align(egui::Align2::LEFT_CENTER),
    );
    probe(ui, "Filename", response.rect);
    if response.changed() {
        view.error = None;
    }
    ui.painter().rect_stroke(
        field,
        tokens.number("r-md"),
        egui::Stroke::new(
            1.,
            tokens.color(if response.has_focus() {
                "theme-accent"
            } else {
                "control-border"
            }),
        ),
        egui::StrokeKind::Inside,
    );
    // Shipping `.recording-filename-input:focus-within` rings the whole field.
    if response.has_focus() {
        crate::primitives::focus_ring(ui, tokens, field, tokens.number("r-md"));
    }
    ui.painter().vline(
        format_rect.left(),
        field.y_range().shrink(1.),
        egui::Stroke::new(1., tokens.color("control-border")),
    );
    let mut format_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(format_rect.shrink2(egui::vec2(1., 1.)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let old = view.gif;
    let mut gif = view.gif;
    select_styled(
        &mut format_ui,
        tokens,
        "Format",
        format_rect.width() - 2.,
        &mut gif,
        &[(false, ".mp4".into(), "MP4"), (true, ".gif".into(), "GIF")],
        crate::primitives::SelectStyle::Inline,
    );
    if gif != old {
        view.gif = gif;
        // Shipping offers Preserve quality only for MP4 and switches a GIF to
        // Compress; Maximum keeps its remembered preset.
        if gif && !view.maximum_size && view.quality == QualityPreset::Preserve {
            view.quality = view
                .compress_quality
                .unwrap_or(recording_editor_ui::DEFAULT_COMPRESS_PRESET);
        }
    }
}

fn show_save_actions(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    editable: bool,
) {
    ui.spacing_mut().item_spacing.y = tokens.number("s-2");
    // `.recording-save-toast`: error, status, or the running stage.
    let (message, color) = if let Some(error) = &view.error {
        (error.clone(), "danger-text")
    } else if let Some(status) = &view.status {
        (status.clone(), "text-subtle")
    } else if let Some(progress) = &view.progress {
        (
            progress.message.clone().unwrap_or_else(|| {
                recording_editor_ui::export_stage_label(progress.stage).to_owned()
            }),
            "text-subtle",
        )
    } else {
        (String::new(), "text-subtle")
    };
    let status = ui.add(egui::Label::new(text(tokens, message, "text-xs", color)).wrap());
    probe(ui, "Status", status.rect);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(editable, |ui| {
                let save = ui
                    .add_enabled(
                        view.presented.is_some() && !view.unapplied(),
                        primary_button(tokens, "Save new copy"),
                    )
                    .on_hover_text(
                        "Creates a separate copy. The original and existing files are never replaced.",
                    );
                probe(ui, "Save new copy", save.rect);
                if save.clicked() {
                    if let Some(error) = recording_editor_ui::filename_error(&view.stem) {
                        view.error = Some(error.into());
                    } else {
                        let cancel = CancelToken::default();
                        view.cancel = Some(cancel.clone());
                        view.saved_path = None;
                        let request = RecordingSaveRequest {
                            destination: view.destination(),
                            export: view.presented.as_ref().unwrap().export.clone(),
                        };
                        view.send(tx, Job::Save(request, cancel));
                    }
                }
                let apply = ui
                    .add_enabled(
                        view.unapplied() && (!view.maximum_size || view.maximum_bytes().is_some()),
                        egui::Button::new("Apply edits").min_size(egui::vec2(0., tokens.number("h-md"))),
                    )
                    .on_hover_text("Update the preview before scrubbing or saving");
                probe(ui, "Apply edits", apply.rect);
                if apply.clicked() {
                    let edit = view.staged_edit(view.presented.as_ref().unwrap());
                    let export = view.export_spec();
                    view.send(
                        tx,
                        Job::Apply(RecordingEditorRequest::UpdatePreview { edit, export }),
                    );
                }
                if view.cancel.is_none() {
                    let replace = ui
                        .add_enabled(
                            view.can_replace(),
                            egui::Button::new("Replace original…").min_size(egui::vec2(0., tokens.number("h-md"))),
                        )
                        .on_hover_text("Replace the saved original and matching History recovery copy after confirmation. Only same-format MP4/GIF; apply edits first. A saved path is only a hint: the backend checks both files before writing.");
                    probe(ui, "Replace original…", replace.rect);
                    if replace.clicked() {
                        view.begin_replace();
                    }
                    // `.recording-show-in-folder`: after a save, until the next one.
                    if let Some(path) = view.saved_path.clone() {
                        let reveal = ui
                            .add(
                                egui::Button::new("Show in Folder")
                                    .min_size(egui::vec2(0., tokens.number("h-md"))),
                            )
                            .on_hover_text(path.display().to_string());
                        probe(ui, "Show in Folder", reveal.rect);
                        if reveal.clicked()
                            && let Err(error) = crate::reveal::reveal(&path)
                        {
                            view.error = Some(format!("Could not show the saved copy: {error}"));
                        }
                    }
                }
            });
            if let Some(cancel) = &view.cancel {
                let label = if view.playing {
                    "Pause playback"
                } else if view.loading_source {
                    "Cancel source preview"
                } else if view.loading_thumbnails {
                    "Cancel thumbnails"
                } else if view.estimating {
                    "Cancel estimate"
                } else if view.comparing.is_some() {
                    "Cancel comparison"
                } else {
                    "Cancel export"
                };
                let response = ui.add_enabled(
                    !cancel.is_cancelled(),
                    egui::Button::new(label).min_size(egui::vec2(0., tokens.number("h-md"))),
                );
                probe(ui, "Cancel", response.rect);
                probe(ui, label, response.rect);
                if response.clicked() {
                    cancel.cancel();
                }
            }
        });
    });
}

fn show_page(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    if view.unapplied() {
        view.comparison = None;
    }
    let gap = tokens.number("s-5");
    ui.spacing_mut().item_spacing.y = gap;
    let title = view
        .presented
        .as_ref()
        .map_or(recording_editor_ui::TITLE_RECORDING, |p| {
            recording_editor_ui::title(&p.source.mime_type)
        });
    ui.label(text(tokens, title, "text-2xl", "text").strong());
    // `.recording-editor-warning`: a caution band for sources that dropped frames.
    if let Some(warning) = view
        .presented
        .as_ref()
        .and_then(|p| recording_editor_ui::dropped_frames_warning(p.dropped_frames))
    {
        let band = egui::Frame::new()
            .fill(tokens.color("caution-surface"))
            .corner_radius(tokens.number("r-md") as u8)
            .inner_margin(egui::Margin::symmetric(
                tokens.number("s-5") as i8,
                tokens.number("s-4") as i8,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(text(tokens, warning, "text-sm", "caution-text"));
            })
            .response;
        probe(ui, "Dropped frames warning", band.rect);
    }
    let window_height = ui.ctx().content_rect().height();
    card_frame(ui, tokens, |ui| {
        show_preview_card(ui, tokens, view, tx, window_height)
    });
    let Some(p) = &view.presented else {
        return;
    };
    let duration = p.source.duration_ms.unwrap_or(0);
    let source_size = (p.source.width, p.source.height);
    let system_audio = p.edit.audio.source_has_system_audio;
    let microphone_audio = p.edit.audio.source_has_microphone_audio;
    ui.add_enabled_ui(!view.busy && !view.picker && !view.confirm_close, |ui| {
        show_timeline_card(ui, tokens, view, tx, duration);
        let wide = ui.available_width() >= 700.;
        if view.gif {
            show_gif_card(ui, tokens, view);
        }
        if wide {
            ui.columns(2, |columns| {
                columns[0].spacing_mut().item_spacing.y = gap;
                columns[1].spacing_mut().item_spacing.y = gap;
                show_crop_card(&mut columns[0], tokens, view, tx, source_size);
                show_quality_card(&mut columns[1], tokens, view, tx);
            });
        } else {
            show_crop_card(ui, tokens, view, tx, source_size);
            show_quality_card(ui, tokens, view, tx);
        }
        if system_audio || microphone_audio {
            show_audio_card(ui, tokens, view, system_audio, microphone_audio);
        }
    });
}

/// `.recording-editor-preview`: raised card whose viewport is sunken.
fn card_frame<R>(ui: &mut egui::Ui, tokens: &Tokens, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(tokens.color("surface-raised"))
        .stroke(egui::Stroke::new(1., tokens.color("border-subtle")))
        .corner_radius(tokens.number("r-xl") as u8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn show_preview_card(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    window_height: f32,
) {
    ui.spacing_mut().item_spacing = egui::vec2(tokens.number("s-4"), 0.);
    let previous_actual_size = view.preview_actual_size;
    let width = ui.available_width();
    let (toolbar, _) = ui.allocate_exact_size(egui::vec2(width, 46.), egui::Sense::hover());
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(toolbar.shrink2(egui::vec2(tokens.number("s-5"), 0.)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.label(text(tokens, "Preview", "text-md", "text-muted"));
    let mode = if view.adjusting_crop {
        "Source crop"
    } else if view.comparison.is_some() {
        "Encoded comparison"
    } else if view.playback_audio_enabled {
        "Audio playback"
    } else if view.preview_sound && !view.playing {
        "Sound selected"
    } else {
        "Silent playback"
    };
    bar.label(text(tokens, mode, "text-sm", "text-subtle"));
    bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        preview_size_segmented(ui, tokens, view);
        let looping = view.preview_loop.load(Ordering::Relaxed);
        if loop_toggle(
            ui,
            tokens,
            looping,
            view.presented.is_some()
                && (!view.busy || view.playing)
                && !view.picker
                && !view.confirm_close
                && view.cancel.as_ref().is_none_or(|cancel| !cancel.is_cancelled()),
        )
        .on_hover_text("Repeat the accepted trim until paused. This changes only playback, not the saved recording.")
        .clicked()
        {
            view.preview_loop.store(!looping, Ordering::Relaxed);
        }
        let compare = toolbar_toggle(
            ui,
            tokens,
            if view.comparison.is_some() { "Hide compare" } else { "Compare" },
            view.comparison.is_some(),
            view.can_compare(),
        )
        .on_hover_text("Encode a sample at the accepted still frame, not the paused playback position. Before is spatially edited; Encoded includes compression, GIF palette and cadence. First attempt only: a Maximum-size save may differ. Apply staged edits and seek inside the accepted trim first.");
        if compare.clicked() {
            if view.comparison.is_some() {
                view.comparison = None;
            } else {
                view.request_comparison(ui.ctx(), tx);
            }
        }
        if toolbar_toggle(
            ui,
            tokens,
            "Sound",
            view.preview_sound,
            view.presented.is_some() && !view.busy && !view.picker && !view.confirm_close,
        )
        .on_hover_text("Preview accepted MP4 audio on the default output device. Change only while stopped. If the device fails, turn Sound off and retry. This never changes the export.")
        .clicked()
        {
            view.preview_sound = !view.preview_sound;
        }
        if view.adjusting_crop {
            let done = ui.add_enabled(
                !view.busy && !view.picker && !view.confirm_close,
                egui::Button::new(text(tokens, "Done cropping", "text-sm", "text")),
            );
            probe(ui, "Done cropping (preview)", done.rect);
            if done.clicked() {
                view.adjusting_crop = false;
                view.crop_gesture = None;
            }
        }
    });
    ui.painter().hline(
        toolbar.x_range(),
        toolbar.bottom(),
        egui::Stroke::new(1., tokens.color("border-subtle")),
    );
    let scale_changed = previous_actual_size != view.preview_actual_size;
    if scale_changed {
        view.crop_gesture = None;
    }
    let height = (window_height * 0.46).clamp(180., 480.);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let radius = tokens.number("r-xl") as u8;
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: radius,
            se: radius,
        },
        tokens.color("surface-sunken"),
    );
    probe(ui, "Preview viewport", rect);
    let inner = rect.shrink(tokens.number("s-5"));
    let preview_texture = if view.adjusting_crop {
        view.source_texture.clone()
    } else if let Some(comparison) = &view.comparison {
        Some(comparison.textures[0].clone())
    } else {
        view.texture.clone()
    };
    if let Some(texture) = preview_texture {
        let size = texture.size_vec2();
        let actual_size = view.preview_actual_size;
        let margin = if view.adjusting_crop {
            tokens.number("s-3")
        } else {
            0.
        };
        let source_mode = view.adjusting_crop;
        let mut viewport = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("recording-preview-viewport")
                .max_rect(inner),
        );
        viewport.shrink_clip_rect(inner);
        let mut image_rect_out = None;
        let mut paint = |ui: &mut egui::Ui, image_rect: egui::Rect| {
            ui.painter().add(
                egui::epaint::RectShape::filled(
                    image_rect,
                    tokens.number("r-md"),
                    egui::Color32::WHITE,
                )
                .with_texture(
                    texture.id(),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                ),
            );
            ui.painter().rect_stroke(
                image_rect,
                tokens.number("r-md"),
                egui::Stroke::new(1., tokens.color("border")),
                egui::StrokeKind::Outside,
            );
            probe(ui, "Preview image", image_rect);
            image_rect_out = Some(image_rect.intersect(ui.clip_rect()));
            if let Some(comparison) = &view.comparison {
                let response = ui
                    .interact(
                        image_rect.intersect(ui.clip_rect()),
                        ui.scope_id().with("encoded-comparison-divider"),
                        egui::Sense::click_and_drag(),
                    )
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                if (response.clicked() || response.dragged())
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    view.comparison_split =
                        ((pos.x - image_rect.left()) / image_rect.width()).clamp(0., 1.);
                }
                let split = image_rect.left() + image_rect.width() * view.comparison_split;
                let encoded_rect =
                    egui::Rect::from_min_max(egui::pos2(split, image_rect.top()), image_rect.max);
                ui.painter()
                    .with_clip_rect(ui.clip_rect().intersect(encoded_rect))
                    .image(
                        comparison.textures[1].id(),
                        image_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        egui::Color32::WHITE,
                    );
                ui.painter().line_segment(
                    [
                        egui::pos2(split, image_rect.top()),
                        egui::pos2(split, image_rect.bottom()),
                    ],
                    egui::Stroke::new(tokens.number("s-1"), tokens.color("text")),
                );
                // Before / Encoded badges on the media, like shipping's
                // CompressionPreview labels.
                for (label, align, anchor) in [
                    (
                        "Before",
                        egui::Align2::LEFT_TOP,
                        image_rect.left_top() + egui::vec2(8., 8.),
                    ),
                    (
                        "Encoded",
                        egui::Align2::RIGHT_TOP,
                        image_rect.right_top() + egui::vec2(-8., 8.),
                    ),
                ] {
                    let painter = ui.painter();
                    let galley = painter.layout_no_wrap(
                        label.to_owned(),
                        egui::FontId::proportional(tokens.number("text-2xs")),
                        tokens.color("glass-text"),
                    );
                    let badge = align.anchor_size(anchor, galley.size() + egui::vec2(12., 6.));
                    painter.rect_filled(badge, tokens.number("r-xs"), tokens.color("glass-strong"));
                    painter.galley(
                        badge.min + egui::vec2(6., 3.),
                        galley,
                        egui::Color32::PLACEHOLDER,
                    );
                }
            }
            if source_mode {
                show_crop_overlay(ui, tokens, view, image_rect);
            } else {
                view.crop_gesture = None;
            }
        };
        if actual_size {
            let mut scroll = egui::ScrollArea::both()
                .id_salt(("recording-preview-scroll", source_mode, texture.size()))
                .max_width(inner.width())
                .max_height(inner.height())
                .auto_shrink([false, false])
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: egui::scroll_area::DragScroll::Never,
                    ..Default::default()
                });
            if scale_changed {
                scroll = scroll.scroll_offset(egui::Vec2::ZERO);
            }
            crate::primitives::scroll_area(&mut viewport, tokens, scroll, |ui| {
                let extent = (size + egui::Vec2::splat(margin * 2.)).max(ui.available_size());
                let (content, _) = ui.allocate_exact_size(extent, egui::Sense::hover());
                paint(ui, egui::Rect::from_center_size(content.center(), size));
            });
        } else {
            let bounds = inner.shrink(margin);
            let scale = (bounds.width() / size.x).min(bounds.height() / size.y);
            paint(
                &mut viewport,
                egui::Rect::from_center_size(bounds.center(), size * scale),
            );
        }
        if !view.adjusting_crop
            && let Some(image) = image_rect_out
        {
            show_overlay_play(ui, tokens, view, tx, image, inner);
        }
    } else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if view.busy {
                "Decoding recording…"
            } else {
                "Preview unavailable"
            },
            egui::FontId::proportional(tokens.number("text-md")),
            tokens.color("text-subtle"),
        );
    }
    let Some(p) = &view.presented else {
        return;
    };
    // Native caption: accepted position, dimensions and output identity.
    let duration = p.source.duration_ms.unwrap_or(0);
    let displayed_position = if view.adjusting_crop {
        p.position_ms
    } else {
        view.playback_position_ms.unwrap_or(p.position_ms)
    };
    let time = |ms| recording_editor_ui::format_editor_time(ms, duration);
    let mut caption = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                egui::pos2(rect.left() + tokens.number("s-5"), rect.bottom()),
                egui::vec2(width - tokens.number("s-5") * 2., 200.),
            ))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    caption.spacing_mut().item_spacing = egui::vec2(tokens.number("s-4"), tokens.number("s-2"));
    caption.add_space(tokens.number("s-4"));
    if view.adjusting_crop {
        caption.label(text(
            tokens,
            format!(
                "Uncropped source · {} · {} × {} · Drag to stage the crop, then Apply edits.",
                time(displayed_position),
                p.source.width,
                p.source.height
            ),
            "text-xs",
            "text-subtle",
        ));
    } else if let Some(comparison) = &view.comparison {
        caption.horizontal(|ui| {
            ui.label(text(tokens, "Before", "text-xs", "text-subtle"));
            let mut percent = (f64::from(view.comparison_split) * 100.).round();
            let split = crate::primitives::RangeSlider::new(
                "encoded-split",
                "Encoded split",
                160.,
                0. ..=100.,
                format!("{percent:.0}%"),
            )
            .show(ui, tokens, &mut percent);
            if split.changed() {
                view.comparison_split = (percent / 100.) as f32;
            }
            probe(ui, "Encoded split", split.rect);
            ui.label(text(
                tokens,
                format!(
                    "Encoded · accepted {} · {} × {}",
                    time(comparison.result.position_ms),
                    comparison.result.frames[0].width(),
                    comparison.result.frames[0].height()
                ),
                "text-xs",
                "text-subtle",
            ));
        });
        caption
            .label(text(
                tokens,
                if p.export.max_size_bytes.is_some() {
                    "Encoded first attempt; the final Maximum-size save may differ."
                } else {
                    "Encoded sample; cadence may select neighboring frames."
                },
                "text-xs",
                "text-faint",
            ))
            .on_hover_text(format!(
                "Selected source position: {} ms. Encoded sample seek: {} ms. Seek positions are timeline intent, not exact decoded frame timestamps.",
                comparison.result.position_ms, comparison.result.after_seek_position_ms
            ));
    } else {
        let (preview_width, preview_height) = view
            .texture
            .as_ref()
            .map_or((p.frame.width() as usize, p.frame.height() as usize), |t| {
                (t.size()[0], t.size()[1])
            });
        caption.label(text(
            tokens,
            format!(
                "{} / {} · Source {} × {} · {} {} preview {} × {}",
                time(displayed_position),
                time(duration),
                p.source.width,
                p.source.height,
                if p.export.format == ExportFormat::Gif {
                    "GIF"
                } else {
                    "MP4"
                },
                recording_editor_ui::quality_label(p.export.quality),
                preview_width,
                preview_height
            ),
            "text-xs",
            "text-subtle",
        ));
        if p.export.max_size_bytes.is_some() {
            caption.label(text(
                tokens,
                "First-attempt preview. Size-limited saves may reduce resolution, frame rate or audio quality.",
                "text-xs",
                "text-faint",
            ));
        }
    }
    let used = caption.min_rect().bottom() - rect.bottom() + tokens.number("s-4");
    ui.allocate_exact_size(egui::vec2(width, used.max(0.)), egui::Sense::hover());
}

fn show_timeline_card(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    duration: u64,
) {
    egui::Frame::new()
        .fill(tokens.color("surface-raised"))
        .stroke(egui::Stroke::new(1., tokens.color("border-subtle")))
        .corner_radius(tokens.number("r-xl") as u8)
        .inner_margin(egui::Margin {
            left: tokens.number("s-6") as i8,
            right: tokens.number("s-6") as i8,
            top: tokens.number("s-5") as i8,
            bottom: tokens.number("s-6") as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = tokens.number("s-4");
            let summary = recording_editor_ui::trim_summary(view.start_ms, view.end_ms, duration);
            ui.horizontal(|ui| {
                ui.label(text(tokens, summary.range, "text-sm", "text").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(text(tokens, summary.selected, "text-sm", "text-subtle"));
                    if let Some(error) = view.thumbnail_error.clone() {
                        let retry = ui
                            .add_enabled(
                                !view.busy && !view.picker && !view.confirm_close,
                                egui::Button::new(text(tokens, "Retry thumbnails", "text-xs", "text")).small(),
                            )
                            .on_hover_text(error);
                        probe(ui, "Retry thumbnails", retry.rect);
                        if retry.clicked() {
                            view.request_thumbnails(tx);
                        }
                        ui.label(text(tokens, "Source thumbnails unavailable.", "text-xs", "text-subtle"));
                    }
                });
            });
            ui.add_space(tokens.number("s-2"));
            let height = recording_editor_ui::TIMELINE_TRACK_HEIGHT + 6.;
            let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
            if let Some(position) = show_trim_timeline(ui, tokens, view, duration, row)
                && view.presented.as_ref().is_some_and(|p| p.position_ms != position)
            {
                view.send(tx, Job::Apply(RecordingEditorRequest::Seek { position_ms: position }));
            }
            ui.add_space(tokens.number("s-2"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = tokens.number("s-3");
                ui.label(text(tokens, "Start (ms)", "text-xs", "text-subtle"));
                let start = crate::primitives::NumberInput::new("trim-start", "Start (ms)", 104.)
                    .range(0. ..=duration as f64)
                    .show(ui, tokens, &mut view.start_ms);
                probe(ui, "Start (ms)", start.rect);
                ui.label(text(tokens, "End (ms)", "text-xs", "text-subtle"));
                let end = crate::primitives::NumberInput::new("trim-end", "End (ms)", 104.)
                    .range(0. ..=duration as f64)
                    .show(ui, tokens, &mut view.end_ms);
                probe(ui, "End (ms)", end.rect);
                let reset = ui.button(text(tokens, "Reset trim", "text-sm", "text"));
                probe(ui, "Reset trim", reset.rect);
                if reset.clicked() {
                    view.start_ms = 0;
                    view.end_ms = duration;
                }
                ui.add_space(tokens.number("s-5"));
                let displayed = view
                    .presented
                    .as_ref()
                    .map_or(0, |p| view.playback_position_ms.unwrap_or(p.position_ms));
                ui.add_enabled_ui(!view.unapplied(), |ui| {
                    ui.label(text(tokens, "Position (ms)", "text-xs", "text-subtle"));
                    // Numeric entry must not send its first digit to the
                    // worker and steal focus before the rest can be typed.
                    let position =
                        crate::primitives::NumberInput::new("position", "Position (ms)", 104.)
                            .range(0. ..=duration.saturating_sub(1) as f64)
                            .show(ui, tokens, &mut view.position_ms);
                    probe(ui, "Position (ms)", position.rect);
                    let seek = ui
                        .button(text(tokens, "Seek", "text-sm", "text"))
                        .on_hover_text("Decode the accepted preview at this source position. Clicking the timeline also seeks.");
                    probe(ui, "Seek", seek.rect);
                    if seek.clicked() && view.position_ms != displayed {
                        view.send(
                            tx,
                            Job::Apply(RecordingEditorRequest::Seek {
                                position_ms: view.position_ms,
                            }),
                        );
                    }
                });
            });
        });
}

fn show_gif_card(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View) {
    card(ui, tokens, "surface-raised", "border-subtle", |ui| {
        card_title(ui, tokens, "GIF settings");
        ui.columns(2, |columns| {
            columns[0].spacing_mut().item_spacing.y = tokens.number("s-3");
            columns[1].spacing_mut().item_spacing.y = tokens.number("s-3");
            field_label(&mut columns[0], tokens, "Frame rate");
            let mut fps = view.gif_frames_per_second.unwrap_or(15);
            let width = columns[0].available_width();
            if select(
                &mut columns[0],
                tokens,
                "Frame rate",
                width,
                &mut fps,
                &recording_editor_ui::GIF_FRAME_RATES
                    .map(|value| (value, format!("{value} FPS"), "")),
            ) {
                view.gif_frames_per_second = Some(fps);
            }
            field_label(&mut columns[1], tokens, "Maximum width");
            let mut maximum = view.gif_maximum_width.unwrap_or(800);
            let width = columns[1].available_width();
            if select(
                &mut columns[1],
                tokens,
                "Maximum width",
                width,
                &mut maximum,
                &recording_editor_ui::GIF_MAXIMUM_WIDTHS
                    .map(|value| (value, format!("{value} px"), "")),
            ) {
                view.gif_maximum_width = Some(maximum);
            }
        });
    });
}

fn show_crop_card(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    source_size: (u32, u32),
) {
    card(ui, tokens, "surface-raised", "border-subtle", |ui| {
        card_title(ui, tokens, "Crop & size");
        let mut crop_enabled = view.crop.is_some();
        let toggle = ui.checkbox(&mut crop_enabled, "Crop recording");
        probe(ui, "Crop recording", toggle.rect);
        if toggle.changed() {
            view.crop = crop_enabled.then_some(CropRect {
                x: 0,
                y: 0,
                width: source_size.0,
                height: source_size.1,
            });
            if !crop_enabled {
                view.adjusting_crop = false;
            }
        }
        // `.editor-number-grid`: X, Y, Width, Height, disabled until cropping.
        let mut preview = view.crop.unwrap_or(CropRect {
            x: 0,
            y: 0,
            width: source_size.0,
            height: source_size.1,
        });
        let enabled = view.crop.is_some();
        let mut changed = None;
        ui.add_enabled_ui(enabled, |ui| {
            ui.columns(4, |columns| {
                for (index, column) in columns.iter_mut().enumerate() {
                    column.spacing_mut().item_spacing.y = tokens.number("s-2");
                    let (label, probe_name) = [
                        ("X", "Crop X"),
                        ("Y", "Crop Y"),
                        ("Width", "Crop width"),
                        ("Height", "Crop height"),
                    ][index];
                    field_label(column, tokens, label);
                    let width = column.available_width();
                    let response = match index {
                        0 => crate::primitives::NumberInput::new(probe_name, probe_name, width)
                            .range(0. ..=f64::from(source_size.0.saturating_sub(2)))
                            .show(column, tokens, &mut preview.x),
                        1 => crate::primitives::NumberInput::new(probe_name, probe_name, width)
                            .range(0. ..=f64::from(source_size.1.saturating_sub(2)))
                            .show(column, tokens, &mut preview.y),
                        _ => {
                            let horizontal = index == 2;
                            let mut value = if horizontal {
                                preview.width
                            } else {
                                preview.height
                            };
                            let maximum = if horizontal {
                                source_size.0
                            } else {
                                source_size.1
                            };
                            let response =
                                crate::primitives::NumberInput::new(probe_name, probe_name, width)
                                    .range(2. ..=f64::from(maximum))
                                    .commit_on_enter()
                                    .show(column, tokens, &mut value);
                            if response.changed() {
                                changed = Some((horizontal, value));
                            }
                            response
                        }
                    };
                    probe(column, probe_name, response.rect);
                }
            });
        });
        if let Some(crop) = &mut view.crop {
            crop.x = preview.x;
            crop.y = preview.y;
            if let Some((horizontal, value)) = changed {
                if !view.crop_aspect_unlocked {
                    *crop = crop.resize_aspect_locked(
                        source_size.0,
                        source_size.1,
                        if horizontal {
                            CropResizeAxis::Width
                        } else {
                            CropResizeAxis::Height
                        },
                        value,
                    );
                } else if horizontal {
                    crop.width = value;
                } else {
                    crop.height = value;
                }
            }
        }
        ui.horizontal(|ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let mut locked = !view.crop_aspect_unlocked;
                let lock = ui.checkbox(&mut locked, "Lock aspect ratio");
                probe(ui, "Lock aspect ratio", lock.rect);
                if lock.changed() {
                    view.crop_aspect_unlocked = !locked;
                }
                let label = if view.adjusting_crop { "Done cropping" } else { "Adjust crop" };
                let adjust = ui.button(label).on_hover_text(
                    "Drag the crop on the full source frame. Arrow keys move a focused handle by 1 source pixel; Shift moves 10.",
                );
                probe(ui, "Adjust crop", adjust.rect);
                probe(ui, label, adjust.rect);
                if adjust.clicked() {
                    if view.adjusting_crop {
                        view.adjusting_crop = false;
                    } else {
                        view.request_crop_view(tx);
                    }
                }
            });
        });
        ui.spacing_mut().item_spacing.y = tokens.number("s-3");
        field_label(ui, tokens, "Output resolution");
        let base = view
            .crop
            .map_or(source_size, |crop| (crop.width, crop.height));
        let base = MaxResolution::Original.constrain(base.0, base.1);
        let current = if view.output_size.is_some() {
            ResolutionChoice::Custom
        } else {
            match view.max_resolution {
                MaxResolution::Original => ResolutionChoice::Original,
                MaxResolution::P1080 => ResolutionChoice::P1080,
                MaxResolution::P720 => ResolutionChoice::P720,
            }
        };
        let mut choice = current;
        let width = ui.available_width().min(430.);
        let options = ResolutionChoice::ALL
            .map(|choice| (choice, choice.label(base.0, base.1), choice.description()));
        if select(
            ui,
            tokens,
            "Output resolution",
            width,
            &mut choice,
            &options,
        ) && choice != current
        {
            match choice {
                ResolutionChoice::Custom => {
                    view.output_size =
                        Some(view.output_dimensions(source_size).unwrap_or_else(|| {
                            view.crop
                                .map_or(source_size, |crop| (crop.width, crop.height))
                        }));
                }
                preset => {
                    view.output_size = None;
                    view.max_resolution = match preset {
                        ResolutionChoice::P1080 => MaxResolution::P1080,
                        ResolutionChoice::P720 => MaxResolution::P720,
                        _ => MaxResolution::Original,
                    };
                }
            }
        }
        if let Some((width, height)) = &mut view.output_size {
            ui.columns(2, |columns| {
                for (index, column) in columns.iter_mut().enumerate() {
                    column.spacing_mut().item_spacing.y = tokens.number("s-2");
                    let (label, value) = if index == 0 {
                        ("Width", &mut *width)
                    } else {
                        ("Height", &mut *height)
                    };
                    field_label(column, tokens, label);
                    let available = column.available_width();
                    let name = format!("Output {}", label.to_lowercase());
                    let response = crate::primitives::NumberInput::new(&name, &name, available)
                        .range(2. ..=f64::from(u32::MAX))
                        .show(column, tokens, value);
                    probe(column, &name, response.rect);
                }
            });
        }
        ui.label(text(
            tokens,
            "Apply previews even-pixel sizes for the selected format and quality.",
            "text-xs",
            "text-subtle",
        ));
    });
}

fn show_quality_card(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    card(ui, tokens, "surface-raised", "border-subtle", |ui| {
        card_title(ui, tokens, "Save quality");
        ui.spacing_mut().item_spacing.y = tokens.number("s-3");
        field_label(ui, tokens, "Quality mode");
        let current = QualityMode::of(view.quality, view.maximum_size);
        let mut mode = current;
        let width = ui.available_width().min(430.);
        let mut options: Vec<_> = QualityMode::available(view.gif)
            .iter()
            .map(|mode| (*mode, mode.label().to_owned(), mode.description()))
            .collect();
        if !options.iter().any(|(option, _, _)| *option == current) {
            // An accepted Preserve GIF keeps showing its mode until changed.
            options.insert(
                0,
                (current, current.label().to_owned(), current.description()),
            );
        }
        if select(ui, tokens, "Quality mode", width, &mut mode, &options) && mode != current {
            match mode {
                QualityMode::Preserve => {
                    view.maximum_size = false;
                    view.quality = QualityPreset::Preserve;
                }
                QualityMode::Compress => {
                    view.maximum_size = false;
                    if view.quality == QualityPreset::Preserve {
                        view.quality = view
                            .compress_quality
                            .unwrap_or(recording_editor_ui::DEFAULT_COMPRESS_PRESET);
                    }
                }
                QualityMode::Maximum => view.maximum_size = true,
            }
        }
        ui.label(text(tokens, mode.description(), "text-xs", "text-subtle"));
        if mode == QualityMode::Compress {
            ui.add_space(tokens.number("s-2"));
            field_label(ui, tokens, "Quality");
            let mut quality = view.quality;
            let options = recording_editor_ui::COMPRESS_PRESETS.map(|preset| {
                (
                    preset,
                    recording_editor_ui::quality_label(preset).to_owned(),
                    recording_editor_ui::quality_description(preset),
                )
            });
            if select(ui, tokens, "Quality", width, &mut quality, &options) {
                view.quality = quality;
                view.compress_quality = Some(quality);
            }
        }
        if mode == QualityMode::Maximum {
            ui.add_space(tokens.number("s-2"));
            field_label(ui, tokens, "Maximum file size");
            ui.horizontal(|ui| {
                let value = ui.add(
                    egui::TextEdit::singleline(&mut view.maximum_value)
                        .desired_width(tokens.number("s-6") * 6.),
                );
                probe(ui, "Maximum file size value", value.rect);
                let mut unit = view.maximum_unit;
                select(
                    ui,
                    tokens,
                    "File size unit",
                    64.,
                    &mut unit,
                    &[FileSizeUnit::Kb, FileSizeUnit::Mb, FileSizeUnit::Gb]
                        .map(|choice| (choice, choice.label().to_owned(), "")),
                );
                if unit != view.maximum_unit {
                    view.set_maximum_unit(unit);
                }
            });
            if view.maximum_bytes().is_none() {
                ui.label(text(
                    tokens,
                    "Enter at least 100 KB (decimal units).",
                    "text-xs",
                    "danger-text",
                ));
            }
            ui.label(text(
                tokens,
                "Preserve quality with a hard limit. Save fails if no retry fits; the original stays unchanged.",
                "text-xs",
                "text-subtle",
            ));
        }
        ui.add_space(tokens.number("s-3"));
        field_label(ui, tokens, "Est. size");
        ui.horizontal(|ui| {
            let shown = view.estimate_presentation();
            let value = ui
                .label(
                    text(
                        tokens,
                        shown.label.clone(),
                        "text-sm",
                        if shown.muted { "text-subtle" } else { "text" },
                    )
                    .monospace(),
                )
                .on_hover_text(recording_editor_ui::ESTIMATE_HELP);
            probe(ui, "Est. size", value.rect);
            if let Some(delta) = &shown.delta {
                let (fill, color) = if delta.smaller() {
                    ("positive-surface", "positive-text")
                } else {
                    ("danger-surface", "danger-text")
                };
                let pill = egui::Frame::new()
                    .fill(tokens.color(fill))
                    .corner_radius(tokens.number("r-pill").min(127.) as u8)
                    .inner_margin(egui::Margin::symmetric(tokens.number("s-3") as i8, 2))
                    .show(ui, |ui| {
                        ui.label(text(tokens, delta.label.clone(), "text-xs", color).strong())
                    })
                    .response
                    .on_hover_text(recording_editor_ui::DELTA_HELP);
                probe(ui, "Est. size delta", pill.rect);
            }
            if !view.maximum_size {
                let estimate = ui
                    .add_enabled(
                        view.presented.is_some() && !view.unapplied(),
                        egui::Button::new(text(tokens, "Estimate size", "text-sm", "text")),
                    )
                    .on_hover_text("Percentage change compares the accepted estimate with the original recording file. Longer recordings use approximate encoded samples. No History entry or saved file is created.");
                probe(ui, "Estimate size", estimate.rect);
                if estimate.clicked() {
                    view.request_estimate(tx);
                }
            }
        });
    });
}

fn show_audio_card(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    system_audio: bool,
    microphone_audio: bool,
) {
    if view.gif {
        // `.editor-audio-warning`: settings stay retained for a later MP4.
        card(ui, tokens, "caution-surface", "caution-surface", |ui| {
            card_title(ui, tokens, "Audio");
            ui.label(text(
                tokens,
                recording_editor_ui::GIF_AUDIO_NOTE,
                "text-sm",
                "caution-text",
            ));
        });
        return;
    }
    card(ui, tokens, "surface-raised", "border-subtle", |ui| {
        card_title(ui, tokens, "Audio");
        ui.spacing_mut().item_spacing.y = tokens.number("s-4");
        for (available, label, volume, mute) in [
            (
                system_audio,
                "System audio",
                &mut view.audio.system_volume,
                &mut view.audio.mute_system_audio,
            ),
            (
                microphone_audio,
                "Microphone",
                &mut view.audio.microphone_volume,
                &mut view.audio.mute_microphone,
            ),
        ] {
            if !available {
                continue;
            }
            ui.horizontal(|ui| {
                let mut enabled = !*mute;
                let (toggle_rect, _) = ui.allocate_exact_size(
                    egui::vec2(130., tokens.number("h-sm")),
                    egui::Sense::hover(),
                );
                let toggle = ui.put(toggle_rect, egui::Checkbox::new(&mut enabled, label));
                probe(ui, label, toggle.rect);
                if toggle.changed() {
                    *mute = !enabled;
                }
                // Shipping `.editor-volume` RangeSlider: 0–200% in whole percents.
                let mut percent = (f64::from(*volume) * 100.).round();
                let name = format!("{label} volume");
                let width = ui.available_width().max(100.);
                let slider = ui
                    .add_enabled_ui(!*mute, |ui| {
                        crate::primitives::RangeSlider::new(
                            &name,
                            &name,
                            width,
                            0. ..=200.,
                            format!("{percent:.0}%"),
                        )
                        .show(ui, tokens, &mut percent)
                    })
                    .inner;
                if slider.changed() {
                    *volume = (percent / 100.) as f32;
                }
                probe(ui, &name, slider.rect);
            });
        }
        let mono = ui.checkbox(&mut view.audio.mono_output, "Convert to mono");
        probe(ui, "Convert to mono", mono.rect);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured image, painted either as a mesh or as a rounded rectangle.
    fn textured(
        shape: &egui::epaint::ClippedShape,
        id: egui::TextureId,
    ) -> Option<(egui::Rect, egui::Rect)> {
        match &shape.shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id == id => {
                Some((mesh.calc_bounds(), shape.clip_rect))
            }
            egui::Shape::Rect(rect)
                if rect
                    .brush
                    .as_ref()
                    .is_some_and(|brush| brush.fill_texture_id == id) =>
            {
                Some((rect.rect, shape.clip_rect))
            }
            _ => None,
        }
    }

    /// Renders `show` with the layout probe and returns the named controls.
    fn probe_frame(
        ctx: &egui::Context,
        tokens: &Tokens,
        view: &mut View,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, ProbeRects) {
        let (tx, _) = mpsc::channel();
        let (sender, _) = mpsc::channel();
        probe_frame_with(ctx, tokens, view, &tx, &sender, size, events)
    }

    fn probe_frame_with(
        ctx: &egui::Context,
        tokens: &Tokens,
        view: &mut View,
        tx: &Sender<Job>,
        sender: &Sender<Event>,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, ProbeRects) {
        PROBE.with_borrow_mut(|controls| *controls = Some(BTreeMap::new()));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| show(ui, tokens, view, tx, sender, egui::ViewportId::ROOT),
        );
        output.textures_delta.clear();
        let controls = PROBE.with_borrow(|controls| controls.clone().unwrap_or_default());
        (output, controls)
    }

    /// The visible part of a named control, panicking when it is missing.
    fn probed(controls: &ProbeRects, name: &str) -> egui::Rect {
        let [x0, y0, x1, y1, cx0, cy0, cx1, cy1] = *controls
            .get(name)
            .unwrap_or_else(|| panic!("missing {name} in {:?}", controls.keys()));
        let rect = egui::Rect::from_min_max(
            egui::pos2(x0 as f32, y0 as f32),
            egui::pos2(x1 as f32, y1 as f32),
        );
        rect.intersect(egui::Rect::from_min_max(
            egui::pos2(cx0 as f32, cy0 as f32),
            egui::pos2(cx1 as f32, cy1 as f32),
        ))
    }

    fn opened() -> View {
        let mut view = View::default();
        view.receive(
            &egui::Context::default(),
            Event::Presented(Ok(Presented {
                revision: 1,
                preview_export: view.export_spec(),
                source: MediaMetadata {
                    kind: captures_media::MediaKind::Video,
                    mime_type: "video/mp4".into(),
                    width: 4,
                    height: 2,
                    duration_ms: Some(3100),
                    size_bytes: 40,
                },
                edit: EditSpec::default(),
                export: view.export_spec(),
                position_ms: 700,
                frame: Arc::new(RgbaImage::new(4, 2)),
                dropped_frames: 0,
            })),
        );
        view
    }

    #[test]
    fn dropped_frames_show_the_shipping_header_warning() {
        let tokens = crate::tokens::load().into_iter().next().unwrap().1;
        let ctx = egui::Context::default();
        let mut view = opened();
        let size = egui::vec2(760., 580.);
        let (_, controls) = probe_frame(&ctx, &tokens, &mut view, size, vec![]);
        assert!(!controls.contains_key("Dropped frames warning"));
        view.presented.as_mut().unwrap().dropped_frames = 1_234;
        let (output, controls) = probe_frame(&ctx, &tokens, &mut view, size, vec![]);
        let warning = probed(&controls, "Dropped frames warning");
        assert!(
            warning.width() > 0. && warning.bottom() < probed(&controls, "Preview viewport").top()
        );
        let copy =
            "This source dropped 1,234 frames during capture. The original timing is preserved.";
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text == copy)));
    }

    #[test]
    fn replacement_requires_confirmation_of_unchanged_accepted_identity() {
        let ctx = egui::Context::default();
        let (tx, jobs) = mpsc::channel();
        let mut view = opened();
        assert!(!view.can_replace());
        view.original_path = Some("stale-history-path.mp4".into());
        view.receive(
            &ctx,
            Event::Opened(opened().presented.unwrap(), Some("original.mp4".into())),
        );
        assert!(view.can_replace());
        view.gif = true;
        assert!(!view.can_replace());
        view.gif = false;
        view.begin_replace();
        assert!(jobs.try_recv().is_err());
        assert_eq!(
            view.confirm_replace.as_ref().unwrap().path,
            PathBuf::from("original.mp4")
        );
        view.request_playback(&tx);
        view.request_comparison(&ctx, &tx);
        view.request_estimate(&tx);
        view.request_thumbnails(&tx);
        assert!(jobs.try_recv().is_err() && view.cancel.is_none());
        for change in 0..3 {
            view.confirm_replace = None;
            view.begin_replace();
            match change {
                0 => view.presented.as_mut().unwrap().revision += 1,
                1 => view.original_path = Some("different.mp4".into()),
                _ => view.presented.as_mut().unwrap().export.quality = QualityPreset::Tiny,
            }
            view.confirm_replacement(&tx);
            assert!(jobs.try_recv().is_err(), "stale confirmation {change}");
        }
        view.presented.as_mut().unwrap().export.quality = QualityPreset::Preserve;
        view.begin_replace();
        view.confirm_replacement(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Replace(_)));
        assert!(view.busy && view.cancel.is_some() && view.confirm_replace.is_none());
        view.request_close();
        assert!(!view.closed, "replacement teardown gates close");
    }

    #[test]
    fn replacement_success_after_cancel_rebases_gif_and_discards_source_caches() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let old = view.presented.as_ref().unwrap().frame.clone();
        view.source_texture = view.texture.clone();
        view.thumbnails = view.texture.clone();
        view.estimate = Some(ExportEstimate {
            size_bytes: 50,
            exact: true,
        });
        view.output_size = Some((600, 200));
        view.start_ms = 500;
        view.quality = QualityPreset::Tiny;
        view.maximum_size = true;
        view.preview_loop.store(true, Ordering::Relaxed);
        let cancel = CancelToken::default();
        cancel.cancel();
        view.cancel = Some(cancel);
        view.busy = true;
        let mut rebased = opened().presented.unwrap();
        rebased.revision = 4;
        rebased.position_ms = 0;
        rebased.source.kind = captures_media::MediaKind::Gif;
        rebased.source.mime_type = "image/gif".into();
        rebased.source.width = 1200;
        rebased.source.height = 400;
        rebased.source.duration_ms = Some(1500);
        rebased.frame = Arc::new(RgbaImage::new(1200, 400));
        rebased.export.format = ExportFormat::Gif;
        rebased.preview_export = rebased.export.clone();
        view.receive(&ctx, Event::Replaced(Ok(("original.gif".into(), rebased))));
        assert!(view.history_changed && !view.busy && !view.dirty() && !view.unapplied());
        assert!(
            view.original_replaced,
            "dismiss the stale original mini preview"
        );
        assert!(
            view.estimate.is_none() && view.source_texture.is_none() && view.thumbnails.is_none()
        );
        assert!(view.output_size.is_none() && !view.maximum_size && view.cancel.is_none());
        assert!(!view.preview_loop.load(Ordering::Relaxed));
        assert_eq!((view.start_ms, view.end_ms, view.position_ms), (0, 1500, 0));
        assert_eq!(view.texture.as_ref().unwrap().size(), [1200, 400]);
        assert_eq!(view.gif_maximum_width, Some(1200));
        assert_eq!(view.export_spec().gif_max_colors, None);
        assert_eq!(
            old.dimensions(),
            (4, 2),
            "old retained frame survives rebase"
        );
        assert!(view.can_replace());
        let (tx, jobs) = mpsc::channel();
        view.request_thumbnails(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Thumbnails(_)));
        assert_eq!(
            view.status.as_deref(),
            Some("Replaced original: original.gif")
        );
        view.receive(&ctx, Event::Thumbnails(Err("cancelled".into())));
        assert_eq!(
            view.status.as_deref(),
            Some("Replaced original: original.gif")
        );
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(0, false, _)));
        view.busy = false;
        view.gif_maximum_width = Some(320);
        assert!(
            view.unapplied(),
            "a real post-rebase width edit still stages"
        );
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(
            (edit.output_width, edit.output_height),
            (Some(320), Some(106))
        );
    }

    #[test]
    fn replacement_failure_preserves_edits_but_indeterminate_failure_blocks_media() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.original_path = Some("original.mp4".into());
        view.saved_edit.trim_start_ms = 100;
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.busy = true;
        view.receive(
            &ctx,
            Event::Replaced(Err(ReplaceOriginalError {
                message: "cancelled before commit".into(),
                requires_reopen: false,
            })),
        );
        assert!(view.dirty() && view.can_replace() && !view.history_changed);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        view.receive(
            &ctx,
            Event::Replaced(Err(ReplaceOriginalError {
                message: "rollback failed".into(),
                requires_reopen: true,
            })),
        );
        assert!(view.requires_reopen && view.presented.is_none() && view.texture.is_none());
        assert!(view.error.as_ref().unwrap().contains("Close and reopen"));
        let (tx, jobs) = mpsc::channel();
        view.request_playback(&tx);
        view.request_estimate(&tx);
        view.request_comparison(&ctx, &tx);
        view.request_thumbnails(&tx);
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 0 }),
        );
        assert!(jobs.try_recv().is_err() && !view.can_replace());
        view.request_close();
        assert!(
            view.closed,
            "terminal failure allows close without stale dirty state"
        );
    }

    #[test]
    fn replacement_confirmation_and_save_actions_fit_minimum_window() {
        for (name, tokens) in crate::tokens::load() {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            view.original_path = Some("/recordings/original.mp4".into());
            let (tx, jobs) = mpsc::channel();
            let (events, _) = mpsc::channel();
            for confirming in [false, true] {
                if confirming {
                    view.begin_replace();
                }
                let mut render = || {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(760., 580.),
                            )),
                            ..Default::default()
                        },
                        |ui| show(ui, &tokens, &mut view, &tx, &events, egui::ViewportId::ROOT),
                    );
                    output.textures_delta.clear();
                    output
                };
                render();
                let output = render();
                let labels = if confirming {
                    vec![
                        "Replace the original recording?",
                        "/recordings/original.mp4",
                        "Cancel replacement",
                        "Replace original",
                    ]
                } else {
                    vec![
                        "Filename",
                        "Saving to",
                        "Change…",
                        "Replace original…",
                        "Apply edits",
                        "Save new copy",
                    ]
                };
                let mut rects = Vec::new();
                for label in labels {
                    let rect = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == label => {
                                Some(text.galley.rect.translate(text.pos.to_vec2()))
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("{name}: missing {label}"));
                    assert!(
                        rect.min.x >= 0.
                            && rect.min.y >= 0.
                            && rect.max.x <= 760.
                            && rect.max.y <= 580.,
                        "{label}: {rect:?}"
                    );
                    assert!(
                        rects
                            .iter()
                            .all(|other: &egui::Rect| !other.intersects(rect)),
                        "overlapping {label}"
                    );
                    rects.push(rect);
                }
            }
            assert!(jobs.try_recv().is_err());
        }
    }

    fn comparison(view: &View) -> Comparison {
        let p = view.presented.as_ref().unwrap();
        Comparison {
            revision: p.revision,
            position_ms: p.position_ms,
            after_seek_position_ms: 650,
            export: p.preview_export.clone(),
            frames: [
                Arc::new(RgbaImage::from_pixel(4, 2, image::Rgba([210, 30, 10, 255]))),
                Arc::new(RgbaImage::from_pixel(4, 2, image::Rgba([10, 60, 180, 255]))),
            ],
        }
    }

    #[test]
    fn comparison_uses_accepted_not_paused_pixels_and_does_not_save_or_edit() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let accepted = view.presented.as_ref().unwrap().frame.clone();
        view.playback_position_ms = Some(2300);
        view.position_ms = 2300;
        view.set_frame(&ctx, &RgbaImage::new(2, 1));
        let (tx, jobs) = mpsc::channel();
        view.request_comparison(&ctx, &tx);
        let Job::Compare(generation, cancel) = jobs.recv().unwrap() else {
            panic!("comparison")
        };
        assert!(!cancel.is_cancelled());
        assert_eq!(view.position_ms, 700);
        assert_eq!(view.playback_position_ms, None);
        assert_eq!(view.texture.as_ref().unwrap().size(), [4, 2]);
        view.request_comparison(&ctx, &tx);
        assert!(jobs.try_recv().is_err());
        let result = comparison(&view);
        let frames = result.frames.clone();
        view.receive(&ctx, Event::Compared(generation, Ok(result)));
        let preview = view.comparison.as_ref().unwrap();
        assert!(Arc::ptr_eq(&preview.result.frames[0], &frames[0]));
        assert!(Arc::ptr_eq(&preview.result.frames[1], &frames[1]));
        assert_ne!(preview.textures[0].id(), preview.textures[1].id());
        assert_eq!(preview.result.position_ms, 700);
        assert_eq!(view.comparison_split, 0.5);
        assert!(!view.busy && view.cancel.is_none() && !view.dirty() && !view.history_changed);
        assert!(Arc::ptr_eq(
            &accepted,
            &view.presented.as_ref().unwrap().frame
        ));
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 900 }),
        );
        assert!(view.comparison.is_none());
        view.receive(&ctx, Event::Presented(Err("seek failed".into())));
        assert!(view.comparison.is_none());
        view.request_close();
        assert!(view.closed && !view.confirm_close);
    }

    #[test]
    fn comparison_cancel_failure_stale_identity_and_request_generation_never_publish() {
        let ctx = egui::Context::default();
        for invalid in 0..7 {
            let mut view = opened();
            let (tx, jobs) = mpsc::channel();
            view.request_comparison(&ctx, &tx);
            let Job::Compare(first, cancel) = jobs.recv().unwrap() else {
                panic!("comparison")
            };
            let mut result = comparison(&view);
            match invalid {
                0 => cancel.cancel(),
                1 => result.revision += 1,
                2 => result.position_ms += 1,
                3 => result.export.quality = QualityPreset::Tiny,
                4 => view.gif = true,
                5 => view.closed = true,
                _ => {}
            }
            view.receive(
                &ctx,
                Event::Compared(
                    first,
                    if invalid == 6 {
                        Err("encoder missing".into())
                    } else {
                        Ok(result)
                    },
                ),
            );
            assert!(
                view.comparison.is_none() && !view.busy && view.cancel.is_none(),
                "case {invalid}"
            );
            assert!(view.error.is_some() && !view.history_changed);
            view.gif = false;
            view.closed = false;
            view.request_comparison(&ctx, &tx);
            let Job::Compare(second, _) = jobs.recv().unwrap() else {
                panic!("retry")
            };
            assert!(second > first);
            let result = comparison(&view);
            view.receive(&ctx, Event::Compared(first, Ok(result)));
            assert!(view.busy && view.comparing == Some(second) && view.comparison.is_none());
            let result = comparison(&view);
            view.receive(&ctx, Event::Compared(second, Ok(result)));
            assert!(view.comparison.is_some() && view.error.is_none());
        }
    }

    #[test]
    fn comparison_trim_gates_and_maximum_use_budget_free_identity() {
        let ctx = egui::Context::default();
        let (tx, jobs) = mpsc::channel();
        let mut view = opened();
        view.start_ms = 700;
        view.end_ms = 900;
        view.maximum_size = true;
        view.maximum_value = ".1".into();
        let export = view.export_spec();
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        let p = view.presented.as_mut().unwrap();
        p.edit = edit;
        p.export = export.clone();
        p.preview_export = ExportSpec {
            max_size_bytes: None,
            ..export
        };
        for (position, allowed) in [(699, false), (700, true), (899, true), (900, false)] {
            view.presented.as_mut().unwrap().position_ms = position;
            assert_eq!(view.can_compare(), allowed);
        }
        view.request_comparison(&ctx, &tx);
        assert!(jobs.try_recv().is_err());
        view.presented.as_mut().unwrap().position_ms = 700;
        view.request_comparison(&ctx, &tx);
        let Job::Compare(generation, _) = jobs.recv().unwrap() else {
            panic!("Maximum comparison")
        };
        let result = comparison(&view);
        assert_eq!(result.export.max_size_bytes, None);
        view.receive(&ctx, Event::Compared(generation, Ok(result)));
        assert!(view.comparison.is_some());
        assert_eq!(
            view.presented.as_ref().unwrap().export.max_size_bytes,
            Some(100_000)
        );
        assert!(view.dirty() && !view.history_changed);
        view.request_playback(&tx);
        assert!(view.comparison.is_none());
        assert!(matches!(jobs.recv().unwrap(), Job::Play(..)));
    }

    #[test]
    fn comparison_split_controls_and_clip_follow_input_at_minimum_size() {
        for (name, tokens) in crate::tokens::load() {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            let (tx, jobs) = mpsc::channel();
            let (events, _) = mpsc::channel();
            view.request_comparison(&ctx, &tx);
            let Job::Compare(generation, _) = jobs.recv().unwrap() else {
                panic!("comparison")
            };
            let result = comparison(&view);
            view.receive(&ctx, Event::Compared(generation, Ok(result)));
            let ids = view
                .comparison
                .as_ref()
                .unwrap()
                .textures
                .each_ref()
                .map(|t| t.id());
            let render = |view: &mut View, input| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(760., 580.),
                        )),
                        events: input,
                        ..Default::default()
                    },
                    |ui| show(ui, &tokens, view, &tx, &events, egui::ViewportId::ROOT),
                );
                output.textures_delta.clear();
                output
            };
            render(&mut view, vec![]);
            let output = render(&mut view, vec![]);
            let mut rects = Vec::new();
            for label in [
                "Loop preview",
                "Sound",
                "Hide compare",
                "Fit",
                "100%",
                "Before",
                // The split RangeSlider's readout.
                "50%",
                "Encoded · accepted 0:00.700 · 4 × 2",
                "Save new copy",
            ] {
                let rect = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == label => {
                            Some(text.galley.rect.translate(text.pos.to_vec2()))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing {label} in {name}"));
                assert!(
                    rect.min.x >= 0.
                        && rect.min.y >= 0.
                        && rect.max.x <= 760.
                        && rect.max.y <= 580.,
                    "{label}: {rect:?}"
                );
                for prior in &rects {
                    assert!(!rect.intersects(*prior), "{label} overlaps {prior:?}");
                }
                rects.push(rect);
            }
            let bounds = output
                .shapes
                .iter()
                .find_map(|shape| textured(shape, ids[0]).map(|(bounds, _)| bounds))
                .unwrap();
            let pointer = egui::pos2(bounds.left() + bounds.width() * 0.25, bounds.center().y);
            render(&mut view, vec![egui::Event::PointerMoved(pointer)]);
            render(&mut view, vec![trim_pointer(pointer, true)]);
            render(&mut view, vec![trim_pointer(pointer, false)]);
            assert!((view.comparison_split - 0.25).abs() < 0.001);
            let output = render(&mut view, vec![]);
            let encoded_clip = output
                .shapes
                .iter()
                .find_map(|shape| textured(shape, ids[1]).map(|(_, clip)| clip))
                .unwrap();
            assert!((encoded_clip.left() - pointer.x).abs() < 0.1);
            assert!((encoded_clip.right() - bounds.right()).abs() < 0.1);
            // The track runs under the right-aligned readout: 14 pt readout row,
            // 2 pt gap, then the 20 pt track.
            let slider = egui::pos2(rects[6].right() - 60., rects[6].center().y + 19.);
            render(&mut view, vec![egui::Event::PointerMoved(slider)]);
            render(&mut view, vec![trim_pointer(slider, true)]);
            render(&mut view, vec![trim_pointer(slider, false)]);
            let before_key = view.comparison_split;
            render(&mut view, vec![trim_key(egui::Key::ArrowRight)]);
            assert!(
                view.comparison_split > before_key,
                "focused split slider accepts arrow input"
            );
            assert!(!view.dirty() && !view.history_changed && jobs.try_recv().is_err());
            view.gif = true;
            render(&mut view, vec![]);
            assert!(
                view.comparison.is_none(),
                "staged format must hide comparison"
            );
            view.gif = false;
            render(&mut view, vec![]);
            assert!(
                view.comparison.is_none(),
                "undoing staging does not restore stale pixels"
            );
        }
    }

    #[test]
    fn decimal_size_units_floor_bytes_without_rounding_or_overflow() {
        assert_eq!(FileSizeUnit::Kb.bytes("100.0199"), Some(100_019));
        assert_eq!(FileSizeUnit::Mb.bytes(".1000199"), Some(100_019));
        assert_eq!(FileSizeUnit::Gb.bytes("0.0001000199"), Some(100_019));
        assert_eq!(FileSizeUnit::Mb.bytes("0.099999999"), Some(99_999));
        assert_eq!(FileSizeUnit::Mb.bytes(".1"), Some(100_000));
        assert_eq!(
            FileSizeUnit::Kb.bytes("18446744073709551.615"),
            Some(u64::MAX)
        );
        for invalid in [
            "",
            ".",
            "-1",
            "NaN",
            "inf",
            "1.2.3",
            "1x",
            "０.1",
            "18446744073709551.616",
        ] {
            assert_eq!(FileSizeUnit::Kb.bytes(invalid), None, "{invalid}");
        }
        for unit in [FileSizeUnit::Kb, FileSizeUnit::Mb, FileSizeUnit::Gb] {
            for bytes in [99_999, 100_000, 100_019, 10_123_456, u64::MAX] {
                assert_eq!(unit.bytes(&unit.value(bytes)), Some(bytes));
            }
        }
    }

    #[test]
    fn maximum_size_is_accepted_save_state_not_an_estimate_or_preview_mutation() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.quality = QualityPreset::Tiny;
        view.maximum_size = true;
        view.maximum_value = ".1000199".into();
        let export = view.export_spec();
        assert_eq!(export.max_size_bytes, Some(100_019));
        assert_eq!(export.quality, QualityPreset::Preserve);
        assert!(view.unapplied() && view.dirty());
        let (tx, jobs) = mpsc::channel();
        view.request_estimate(&tx);
        view.request_playback(&tx);
        assert!(jobs.try_recv().is_err());
        let mut accepted = opened().presented.unwrap();
        accepted.export = export.clone();
        let image = accepted.frame.clone();
        view.estimate = Some(ExportEstimate {
            size_bytes: 999,
            exact: true,
        });
        view.receive(&ctx, Event::Presented(Ok(accepted)));
        assert!(!view.unapplied() && view.dirty() && view.estimate.is_none());
        assert_eq!(view.estimate_label(), "≤ 100 KB");
        view.request_estimate(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "maximum mode displays its cap without encoding samples"
        );
        view.set_maximum_unit(FileSizeUnit::Kb);
        assert_eq!(view.maximum_value, "100.019");
        assert_eq!(view.export_spec(), export);
        assert!(!view.unapplied());
        view.receive(&ctx, Event::Saved(Err("cannot fit".into())));
        assert!(view.dirty());
        assert!(Arc::ptr_eq(&image, &view.presented.as_ref().unwrap().frame));
        view.receive(
            &ctx,
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "limited.mp4".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert!(!view.dirty());

        view.maximum_value = "200".into();
        assert!(view.unapplied() && view.dirty());
        view.receive(&ctx, Event::Presented(Err("planner rejected".into())));
        assert_eq!(view.maximum_value, "200");
        assert_eq!(view.presented.as_ref().unwrap().export, export);
        assert!(Arc::ptr_eq(&image, &view.presented.as_ref().unwrap().frame));
        view.maximum_value = "100.019".into();
        let mut seek = opened().presented.unwrap();
        seek.export = export.clone();
        seek.position_ms = 1377;
        view.receive(&ctx, Event::Presented(Ok(seek)));
        assert!(!view.dirty() && !view.unapplied());
        assert_eq!(view.maximum_value, "100.019");
        view.gif = true;
        assert_eq!(view.export_spec().max_size_bytes, Some(100_019));
        view.gif = false;

        for invalid in ["", ".", "99.999", "-10", "NaN"] {
            view.maximum_value = invalid.into();
            assert!(view.maximum_bytes().is_none() && view.unapplied());
            assert_eq!(
                view.export_spec().max_size_bytes,
                Some(0),
                "invalid input never removes the limit"
            );
        }
        view.maximum_size = false;
        assert_eq!(view.export_spec().max_size_bytes, None);
        assert_eq!(
            view.export_spec().quality,
            QualityPreset::Tiny,
            "leaving maximum restores the selected quality"
        );
        let fresh = opened();
        assert!(!fresh.maximum_size);
        assert_eq!(fresh.maximum_value, "10");
        assert_eq!(fresh.maximum_unit.label(), "MB");
    }

    #[test]
    fn actual_preview_scroll_and_crop_mapping_are_display_only() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let ctx = egui::Context::default();
        tokens.apply(&ctx, true);
        let mut view = opened();
        let frame = Arc::new(RgbaImage::new(1200, 800));
        let p = view.presented.as_mut().unwrap();
        p.source.width = 1200;
        p.source.height = 800;
        p.frame = frame.clone();
        view.set_frame(&ctx, &frame);
        view.estimate = Some(ExportEstimate {
            size_bytes: 4567,
            exact: true,
        });
        view.playback_position_ms = Some(1337);
        let (tx, jobs) = mpsc::channel();
        let (events, _) = mpsc::channel();
        let mut time = 0.;
        let mut render = |view: &mut View, input: Vec<egui::Event>| {
            time += 0.1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760., 580.),
                    )),
                    time: Some(time),
                    events: input,
                    ..Default::default()
                },
                |ui| show(ui, &tokens, view, &tx, &events, egui::ViewportId::ROOT),
            );
            output.textures_delta.clear();
            output
        };
        let label = |output: &egui::FullOutput, name: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == name => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {name}"))
        };
        let image = |output: &egui::FullOutput, id: egui::TextureId| {
            output
                .shapes
                .iter()
                .find_map(|shape| textured(shape, id))
                .expect("preview image mesh")
        };
        render(&mut view, vec![]);
        let fit = render(&mut view, vec![]);
        let id = view.texture.as_ref().unwrap().id();
        assert!(image(&fit, id).0.width() < 1200.);
        let actual = label(&fit, "100%");
        let output = render(
            &mut view,
            vec![
                egui::Event::PointerMoved(actual),
                trim_pointer(actual, true),
                trim_pointer(actual, false),
            ],
        );
        assert!(view.preview_actual_size);
        let (actual_image, clip) = image(&output, id);
        assert_eq!(actual_image.size(), egui::vec2(1200., 800.));
        assert!(clip.width() <= 760. && clip.height() < 400.);
        assert!(jobs.try_recv().is_err());
        assert!(!view.dirty() && !view.history_changed);
        assert_eq!(view.estimate.as_ref().unwrap().size_bytes, 4567);
        assert_eq!(view.playback_position_ms, Some(1337));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));

        view.crop = Some(CropRect {
            x: 40,
            y: 20,
            width: 200,
            height: 100,
        });
        view.source_texture = Some(view.texture.as_ref().unwrap().clone());
        view.adjusting_crop = true;
        let output = render(&mut view, vec![]);
        assert!(
            output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.job.text == "200 × 100")),
            "the crop box shows its source-pixel size like shipping"
        );
        let (before, clip) = image(&output, id);
        let origin = before.min + egui::vec2(100., 60.);
        assert!(clip.contains(origin));
        view.crop_gesture = Some(CropGesture {
            initial: view.crop.unwrap(),
            handle: CropDragHandle::Move,
            origin,
            image: before,
            locked: true,
        });
        render(
            &mut view,
            vec![
                egui::Event::PointerMoved(origin),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(-30., -40.),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let output = render(&mut view, vec![]);
        let (scrolled, clip) = image(&output, id);
        assert!(scrolled.left() < before.left() && scrolled.top() < before.top());
        assert_eq!(scrolled.size(), egui::vec2(1200., 800.));
        assert!(
            view.crop_gesture.is_none(),
            "scrolling ends an active source gesture"
        );
        let point = scrolled.min + egui::vec2(100., 60.);
        assert!(clip.contains(point));
        render(
            &mut view,
            vec![
                egui::Event::PointerMoved(point),
                trim_pointer(point, true),
                egui::Event::PointerMoved(point + egui::vec2(13., 7.)),
                trim_pointer(point + egui::vec2(13., 7.), false),
            ],
        );
        assert_eq!(
            view.crop,
            Some(CropRect {
                x: 53,
                y: 27,
                width: 200,
                height: 100
            })
        );
        assert!(
            jobs.try_recv().is_err(),
            "scroll and crop feedback never decode"
        );
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.playback_position_ms, Some(1337));

        view.adjusting_crop = false;
        view.set_frame(&ctx, &RgbaImage::new(200, 80));
        render(&mut view, vec![]);
        let output = render(&mut view, vec![]);
        let (small, clip) = image(&output, id);
        assert_eq!(
            small.size(),
            egui::vec2(200., 80.),
            "100% uses decoded pixels, not source metadata"
        );
        assert!(
            (small.center() - clip.center()).length() < 1.,
            "small images stay centered"
        );
        view.receive(&ctx, Event::Presented(Ok(opened().presented.unwrap())));
        assert!(
            view.preview_actual_size,
            "accepted updates preserve the display preference"
        );
        assert!(!opened().preview_actual_size, "new items default to Fit");
    }

    #[test]
    fn source_crop_view_is_independent_cached_and_invalidated_by_seek() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.crop = Some(CropRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        });
        view.estimate = Some(ExportEstimate {
            size_bytes: 5432,
            exact: true,
        });
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let texture = view.texture.as_ref().unwrap().id();
        let dirty = view.dirty();
        let (tx, jobs) = mpsc::channel();
        view.request_crop_view(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::SourceFrame(_)));
        assert!(view.busy && !view.adjusting_crop);
        view.receive(&ctx, Event::SourceFrame(Ok(Arc::new(RgbaImage::new(4, 2)))));
        assert!(!view.busy && view.adjusting_crop);
        assert_eq!(view.texture.as_ref().unwrap().id(), texture);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.dirty(), dirty);
        assert_eq!(view.estimate.as_ref().unwrap().size_bytes, 5432);
        assert_eq!(view.position_ms, 700);
        view.adjusting_crop = false;
        view.request_crop_view(&tx);
        assert!(
            view.adjusting_crop && jobs.try_recv().is_err(),
            "same-position source is reused"
        );
        view.request_playback(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "source crop mode never starts motion"
        );

        let p = opened().presented.unwrap();
        view.receive(&ctx, Event::Presented(Ok(p)));
        assert!(
            !view.adjusting_crop && view.source_texture.is_some(),
            "same-position Apply retains source pixels"
        );
        let mut p = opened().presented.unwrap();
        p.position_ms = 1337;
        view.receive(&ctx, Event::Presented(Ok(p)));
        assert!(
            view.source_texture.is_none(),
            "Seek cannot reuse another position's source still"
        );
        assert!(
            opened().source_texture.is_none(),
            "another item has no cached source pixels"
        );
    }

    #[test]
    fn source_crop_cancel_late_success_failure_retry_and_busy_gates() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.crop = Some(CropRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        });
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let (tx, jobs) = mpsc::channel();
        view.request_crop_view(&tx);
        let Job::SourceFrame(cancel) = jobs.recv().unwrap() else {
            panic!("source job")
        };
        view.request_crop_view(&tx);
        view.request_close();
        assert!(!view.closed && jobs.try_recv().is_err());
        cancel.cancel();
        view.receive(&ctx, Event::SourceFrame(Ok(Arc::new(RgbaImage::new(4, 2)))));
        assert!(!view.adjusting_crop && view.source_texture.is_none() && !view.busy);
        view.request_crop_view(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::SourceFrame(_)));
        view.receive(&ctx, Event::SourceFrame(Err("missing source".into())));
        assert!(!view.adjusting_crop && !view.busy);
        assert!(view.error.as_ref().unwrap().contains("missing source"));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        view.request_crop_view(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::SourceFrame(_)));
    }

    #[test]
    fn crop_pointer_maps_letterboxed_source_and_preserves_accepted_identity() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let image = egui::Rect::from_min_size(egui::pos2(50., 75.), egui::vec2(400., 225.));
        let initial = CropRect {
            x: 40,
            y: 20,
            width: 160,
            height: 80,
        };
        for (start, end, locked, expected) in [
            (
                egui::pos2(180., 150.),
                egui::pos2(205., 162.5),
                false,
                CropRect {
                    x: 60,
                    y: 30,
                    width: 160,
                    height: 80,
                },
            ),
            (
                egui::pos2(102., 102.),
                egui::pos2(77., 89.5),
                true,
                CropRect {
                    x: 20,
                    y: 10,
                    width: 180,
                    height: 90,
                },
            ),
            (
                egui::pos2(302., 202.),
                egui::pos2(342., 227.),
                false,
                CropRect {
                    x: 40,
                    y: 20,
                    width: 192,
                    height: 100,
                },
            ),
        ] {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, true);
            let mut view = opened();
            view.presented.as_mut().unwrap().source.width = 320;
            view.presented.as_mut().unwrap().source.height = 180;
            view.crop = Some(initial);
            view.crop_aspect_unlocked = !locked;
            let accepted = view.presented.as_ref().unwrap().frame.clone();
            let mut run = |events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(600., 400.),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        show_crop_overlay(ui, &tokens, &mut view, image);
                        if ctx.current_pass_index() == 0 {
                            ctx.request_discard("crop input runs once");
                        }
                    },
                );
                output.textures_delta.clear();
            };
            run(vec![]);
            run(vec![
                egui::Event::PointerMoved(start),
                trim_pointer(start, true),
                egui::Event::PointerMoved(end),
                trim_pointer(end, false),
            ]);
            assert_eq!(view.crop, Some(expected));
            assert!(view.crop_gesture.is_none());
            assert!(Arc::ptr_eq(
                &accepted,
                &view.presented.as_ref().unwrap().frame
            ));
            assert!(view.unapplied(), "only staged geometry changes");
            assert_eq!(view.position_ms, 700);
        }
    }

    #[test]
    fn crop_gesture_ends_on_escape_focus_layout_and_busy_and_keys_use_source_pixels() {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let ctx = egui::Context::default();
        tokens.apply(&ctx, false);
        let image = egui::Rect::from_min_size(egui::pos2(50., 75.), egui::vec2(400., 225.));
        let mut view = opened();
        view.presented.as_mut().unwrap().source.width = 320;
        view.presented.as_mut().unwrap().source.height = 180;
        view.crop = Some(CropRect {
            x: 40,
            y: 20,
            width: 160,
            height: 80,
        });
        let mut run = |events, image, focused, busy| {
            view.busy = busy;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600., 400.),
                    )),
                    events,
                    focused,
                    ..Default::default()
                },
                |ui| show_crop_overlay(ui, &tokens, &mut view, image),
            );
            output.textures_delta.clear();
            (view.crop.unwrap(), view.crop_gesture.is_some())
        };
        run(vec![], image, true, false);
        let start = egui::pos2(180., 150.);
        let moved = egui::pos2(205., 162.5);
        let expected = CropRect {
            x: 60,
            y: 30,
            width: 160,
            height: 80,
        };
        assert_eq!(
            run(
                vec![trim_pointer(start, true), egui::Event::PointerMoved(moved)],
                image,
                true,
                false
            ),
            (expected, true)
        );
        assert_eq!(
            run(
                vec![
                    trim_key(egui::Key::Escape),
                    egui::Event::PointerMoved(egui::pos2(400., 300.))
                ],
                image,
                true,
                false
            ),
            (expected, false)
        );
        run(
            vec![trim_pointer(moved, false), trim_pointer(moved, true)],
            image,
            true,
            false,
        );
        assert_eq!(
            run(
                vec![egui::Event::PointerMoved(egui::pos2(400., 300.))],
                image,
                false,
                false
            ),
            (expected, false)
        );
        assert_eq!(
            run(
                vec![
                    trim_pointer(moved, false),
                    trim_pointer(moved, true),
                    egui::Event::PointerMoved(egui::pos2(400., 300.))
                ],
                image,
                true,
                true
            ),
            (expected, false)
        );
        run(
            vec![trim_pointer(moved, false), trim_pointer(moved, true)],
            image,
            true,
            false,
        );
        assert_eq!(
            run(
                vec![egui::Event::PointerMoved(egui::pos2(400., 300.))],
                image.translate(egui::vec2(10., 0.)),
                true,
                false
            ),
            (expected, false)
        );
        run(
            vec![
                trim_pointer(moved, false),
                trim_pointer(moved, true),
                trim_pointer(moved, false),
            ],
            image,
            true,
            false,
        );
        assert_eq!(
            run(vec![trim_key(egui::Key::ArrowRight)], image, true, false)
                .0
                .x,
            61
        );
        let shift = egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        assert_eq!(
            run(vec![shift], image, true, false).0.y,
            40,
            "Shift nudge is ten source pixels, not ten scaled view points"
        );
    }

    #[test]
    fn crop_pointer_finishes_pending_text_before_starting_a_new_gesture() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let ctx = egui::Context::default();
        tokens.apply(&ctx, true);
        let mut view = opened();
        view.presented.as_mut().unwrap().source.width = 320;
        view.presented.as_mut().unwrap().source.height = 180;
        let initial = CropRect {
            x: 40,
            y: 20,
            width: 160,
            height: 80,
        };
        view.crop = Some(initial);
        let image = egui::Rect::from_min_size(egui::pos2(50., 75.), egui::vec2(400., 225.));
        let id = egui::Id::unique("pending numeric text");
        let mut pending = "200".to_owned();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.add(egui::TextEdit::singleline(&mut pending).id(id))
                    .request_focus();
            });
            output.textures_delta.clear();
        }
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![
                    trim_pointer(egui::pos2(180., 150.), true),
                    egui::Event::PointerMoved(egui::pos2(205., 162.5)),
                    trim_pointer(egui::pos2(205., 162.5), false),
                ],
                ..Default::default()
            },
            |ui| {
                show_crop_overlay(ui, &tokens, &mut view, image);
                ui.add(egui::TextEdit::singleline(&mut pending).id(id));
            },
        );
        output.textures_delta.clear();
        assert_eq!(
            view.crop,
            Some(initial),
            "do not move from stale geometry while a field commits"
        );
        assert!(view.crop_gesture.is_none());
        assert_eq!(pending, "200");
        assert_ne!(ctx.memory(|memory| memory.focused()), Some(id));
    }

    fn timeline_frame(
        ctx: &egui::Context,
        tokens: &Tokens,
        view: &mut View,
        events: Vec<egui::Event>,
        rect: egui::Rect,
        focused: bool,
    ) {
        let duration = view.presented.as_ref().unwrap().source.duration_ms.unwrap();
        let mut passes = 0;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 200.),
                )),
                events,
                focused,
                ..Default::default()
            },
            |ui| {
                passes += 1;
                show_trim_timeline(ui, tokens, view, duration, rect);
                if ctx.current_pass_index() == 0 {
                    ctx.request_discard("trim input must run once");
                }
            },
        );
        output.textures_delta.clear();
        assert_eq!(passes, 2);
    }

    fn trim_pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn trim_key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn trim_pointer_stages_once_preserves_grab_offset_and_never_changes_accepted_frame() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let ctx = egui::Context::default();
        let grip = tokens.number("s-6");
        let rect =
            egui::Rect::from_min_size(egui::pos2(40., 40.), egui::vec2(400. + 2. * grip, 24.));
        let down = egui::pos2(40. + grip / 2., 52.);
        let mut view = opened();
        let accepted = view.presented.as_ref().unwrap().frame.clone();
        let frame =
            |view: &mut View, events| timeline_frame(&ctx, &tokens, view, events, rect, true);
        frame(&mut view, vec![]);
        frame(
            &mut view,
            vec![
                trim_pointer(down, true),
                egui::Event::PointerMoved(down + egui::vec2(2.99, 0.)),
            ],
        );
        assert_eq!(
            view.start_ms, 0,
            "a click or subthreshold move must not jump the trim"
        );
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(down + egui::vec2(3., 0.))],
        );
        assert_eq!(view.start_ms, 23, "3px / 400px × 3100ms rounds to 23ms");
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(down + egui::vec2(40., 0.))],
        );
        assert_eq!(
            view.start_ms, 310,
            "delta is measured from the original grab, not the moved grip"
        );
        assert!(
            view.trim_gesture.is_some(),
            "a second layout pass cannot replay pointer down"
        );
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(egui::pos2(4000., 52.))],
        );
        assert_eq!(
            view.start_ms, 310,
            "wild coordinates retain the last accepted sample"
        );
        let release = down + egui::vec2(100., 0.);
        frame(
            &mut view,
            vec![
                egui::Event::PointerMoved(release),
                trim_pointer(release, false),
                egui::Event::PointerMoved(down + egui::vec2(200., 0.)),
            ],
        );
        assert_eq!((view.start_ms, view.end_ms), (775, 3100));
        assert!(view.trim_gesture.is_none() && view.unapplied() && view.dirty());
        assert_eq!(view.position_ms, 700);
        assert_eq!(view.presented.as_ref().unwrap().edit, EditSpec::default());
        assert!(Arc::ptr_eq(
            &accepted,
            &view.presented.as_ref().unwrap().frame
        ));
        assert!(!view.busy && !view.history_changed);
        assert_eq!(view.estimate_label(), "Apply edits to estimate");
    }

    #[test]
    fn trim_cancellation_and_busy_gates_keep_staged_values_without_resuming_a_drag() {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let grip = tokens.number("s-6");
        let rect =
            egui::Rect::from_min_size(egui::pos2(40., 40.), egui::vec2(400. + 2. * grip, 24.));
        for reason in [
            "escape",
            "pointer gone",
            "focus",
            "resize",
            "busy",
            "picker",
            "close",
        ] {
            let ctx = egui::Context::default();
            let mut view = opened();
            let down = egui::pos2(rect.right() - grip / 2., 52.);
            timeline_frame(&ctx, &tokens, &mut view, vec![], rect, true);
            timeline_frame(
                &ctx,
                &tokens,
                &mut view,
                vec![
                    trim_pointer(down, true),
                    egui::Event::PointerMoved(down - egui::vec2(40., 0.)),
                ],
                rect,
                true,
            );
            assert_eq!(view.end_ms, 2790, "{reason}: end drag must be established");
            let events = match reason {
                "escape" => vec![trim_key(egui::Key::Escape)],
                "pointer gone" => vec![egui::Event::PointerGone],
                "busy" => {
                    view.busy = true;
                    vec![]
                }
                "picker" => {
                    view.picker = true;
                    vec![]
                }
                "close" => {
                    view.request_close();
                    vec![]
                }
                _ => vec![],
            };
            let resized = if reason == "resize" {
                rect.translate(egui::vec2(3., 0.))
            } else {
                rect
            };
            timeline_frame(&ctx, &tokens, &mut view, events, resized, reason != "focus");
            assert!(view.trim_gesture.is_none(), "{reason}");
            view.busy = false;
            view.picker = false;
            view.confirm_close = false;
            timeline_frame(
                &ctx,
                &tokens,
                &mut view,
                vec![egui::Event::PointerMoved(down - egui::vec2(80., 0.))],
                resized,
                true,
            );
            assert_eq!(
                (view.start_ms, view.end_ms),
                (0, 2790),
                "{reason}: no rollback or resurrection"
            );
        }
    }

    #[test]
    fn trim_keyboard_is_focus_scoped_and_preserves_one_millisecond_span() {
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let grip = tokens.number("s-6");
        let rect =
            egui::Rect::from_min_size(egui::pos2(40., 40.), egui::vec2(400. + 2. * grip, 24.));
        for duration in [59_999, 60_000] {
            let ctx = egui::Context::default();
            let mut view = opened();
            view.end_ms = duration;
            view.presented.as_mut().unwrap().source.duration_ms = Some(duration);
            let frame =
                |view: &mut View, events| timeline_frame(&ctx, &tokens, view, events, rect, true);
            frame(&mut view, vec![]);
            frame(&mut view, vec![trim_key(egui::Key::PageDown)]);
            assert_eq!(view.end_ms, duration, "unfocused keys must not change trim");
            let down = egui::pos2(rect.right() - grip / 2., 52.);
            frame(
                &mut view,
                vec![trim_pointer(down, true), trim_pointer(down, false)],
            );
            frame(
                &mut view,
                vec![
                    trim_key(egui::Key::ArrowLeft),
                    trim_key(egui::Key::ArrowLeft),
                ],
            );
            assert_eq!(
                view.end_ms,
                duration - if duration < 60_000 { 2 } else { 20 }
            );
            frame(&mut view, vec![trim_key(egui::Key::PageDown)]);
            assert_eq!(
                view.end_ms,
                duration - if duration < 60_000 { 1002 } else { 1020 }
            );
            view.start_ms = view.end_ms - 3;
            frame(&mut view, vec![trim_key(egui::Key::PageDown)]);
            assert_eq!(view.end_ms, view.start_ms + 1);
            let down = egui::pos2(
                rect.left() + grip + 400. * view.start_ms as f32 / duration as f32 - grip / 2.,
                52.,
            );
            frame(
                &mut view,
                vec![trim_pointer(down, true), trim_pointer(down, false)],
            );
            frame(&mut view, vec![trim_key(egui::Key::PageUp)]);
            assert_eq!(
                view.start_ms,
                view.end_ms - 1,
                "adjacent grips keep distinct focus/hit regions"
            );
        }
    }

    #[test]
    fn estimates_follow_accepted_settings_not_seek_or_staged_values() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.presented.as_mut().unwrap().source.size_bytes = 20000;
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.receive(
            &ctx,
            Event::Estimated(Ok(ExportEstimate {
                size_bytes: 12345,
                exact: true,
            })),
        );
        assert_eq!(view.estimate_label(), "12.3 KB · −38%");
        assert!(!view.dirty() && !view.history_changed);
        let p = view.presented.as_ref().unwrap();
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                revision: p.revision + 1,
                preview_export: p.preview_export.clone(),
                source: p.source.clone(),
                edit: p.edit.clone(),
                export: p.export.clone(),
                position_ms: 1200,
                frame: frame.clone(),
                dropped_frames: 0,
            })),
        );
        assert_eq!(
            view.estimate_label(),
            "12.3 KB · −38%",
            "seek cannot change file size"
        );
        view.receive(&ctx, Event::Presented(Err("seek failed".into())));
        assert_eq!(
            view.estimate_label(),
            "12.3 KB · −38%",
            "a failed seek leaves the accepted estimate valid"
        );
        let (tx, jobs) = mpsc::channel();
        view.gif = true;
        assert_eq!(view.estimate_label(), "Apply edits to estimate");
        view.request_estimate(&tx);
        assert!(jobs.try_recv().is_err() && !view.busy);
        view.receive(&ctx, Event::Presented(Err("bad format preview".into())));
        assert_eq!(view.estimate_label(), "Apply edits to estimate");
        view.gif = false;
        assert_eq!(
            view.estimate_label(),
            "12.3 KB · −38%",
            "reverting staged edits restores the matching result"
        );
        view.gif = true;
        let p = view.presented.as_ref().unwrap();
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                revision: p.revision + 1,
                preview_export: view.export_spec(),
                source: p.source.clone(),
                edit: view.staged_edit(p),
                export: view.export_spec(),
                position_ms: 1200,
                frame: frame.clone(),
                dropped_frames: 0,
            })),
        );
        assert!(
            view.estimate.is_none(),
            "accepting different settings invalidates the old result"
        );
        view.request_estimate(&tx);
        assert!(matches!(jobs.try_recv(), Ok(Job::Estimate(_))));
        view.request_estimate(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "only one worker operation is accepted"
        );
        view.request_close();
        assert!(!view.closed && !view.confirm_close);
        view.receive(
            &ctx,
            Event::Estimated(Ok(ExportEstimate {
                size_bytes: 67890,
                exact: false,
            })),
        );
        assert_eq!(view.estimate_label(), "≈ 67.9 KB · +239%");
        assert!(!view.busy && !view.estimating && view.cancel.is_none() && view.error.is_none());
        assert!(
            view.dirty() && !view.history_changed,
            "estimating does not save edits"
        );
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
    }

    #[test]
    fn estimate_delta_uses_original_bytes_and_shipping_rounding_without_affecting_identity() {
        let ctx = egui::Context::default();
        for (original, size_bytes, suffix) in [
            (8, 7, " · −12%"),
            (8, 9, " · +13%"),
            (256, 255, ""),
            (256, 257, ""),
            (0, 23, ""),
            (100, 0, " · −100%"),
            (100, 100, ""),
        ] {
            let mut view = opened();
            view.presented.as_mut().unwrap().source.size_bytes = original;
            view.receive(
                &ctx,
                Event::Estimated(Ok(ExportEstimate {
                    size_bytes,
                    exact: true,
                })),
            );
            assert_eq!(view.estimate_label(), format!("{size_bytes} B{suffix}"));
            assert!(!view.dirty() && !view.history_changed);
            view.estimate.as_mut().unwrap().exact = false;
            assert_eq!(view.estimate_label(), format!("≈ {size_bytes} B{suffix}"));
            view.estimating = true;
            assert_eq!(view.estimate_label(), "Estimating…");
            view.estimating = false;
            view.maximum_size = true;
            view.maximum_value.clear();
            assert!(
                !view.estimate_label().contains('%'),
                "invalid Maximum never advertises a reduction"
            );
            view.maximum_value = "10".into();
            assert!(
                !view.estimate_label().contains('%'),
                "staged cap is not an estimate"
            );
            view.presented.as_mut().unwrap().export = view.export_spec();
            assert_eq!(view.estimate_label(), "≤ 10.0 MB");
            view.receive(&ctx, Event::Estimated(Err("cancelled".into())));
            assert!(view.estimate.is_none());
        }
        assert_eq!(opened().estimate_label(), "—");
    }

    #[test]
    fn playback_pause_resume_and_eof_preserve_accepted_state() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.start_ms = 123;
        view.presented.as_mut().unwrap().edit.trim_start_ms = 123;
        view.saved_edit = view.presented.as_ref().unwrap().edit.clone();
        let accepted = view.presented.as_ref().unwrap().frame.clone();
        let texture_id = view.texture.as_ref().unwrap().id();
        view.estimate = Some(ExportEstimate {
            size_bytes: 1234,
            exact: true,
        });
        let (tx, jobs) = mpsc::channel();
        // Uncommitted seek text is not the frame currently being displayed.
        view.position_ms = 2200;
        view.request_playback(&tx);
        let Job::Play(position, false, cancel) = jobs.recv().unwrap() else {
            panic!("play queued")
        };
        assert_eq!(position, 700);
        assert_eq!(
            view.position_ms, 700,
            "the playhead labels the retained frame during startup"
        );
        assert!(view.playing && view.busy && !view.dirty());
        view.request_playback(&tx);
        view.request_estimate(&tx);
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 1800 }),
        );
        assert!(
            jobs.try_recv().is_err(),
            "playback owns the serialized worker"
        );
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1100,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        assert_eq!(
            view.texture.as_ref().unwrap().id(),
            texture_id,
            "reuse GPU allocation"
        );
        assert_eq!(view.position_ms, 1100);
        view.pause_playback();
        assert!(
            cancel.is_cancelled() && view.busy,
            "Pause waits for decoder teardown"
        );
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1200,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        assert_eq!(
            view.position_ms, 1100,
            "late frame cannot move a paused playhead"
        );
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        assert!(!view.busy && !view.playing && view.cancel.is_none());
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(1100, false, _)));
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 3066,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Ended)));
        view.request_playback(&tx);
        assert!(
            matches!(jobs.recv().unwrap(), Job::Play(123, false, _)),
            "EOF replays accepted trim"
        );
        let p = view.presented.as_ref().unwrap();
        assert_eq!(p.position_ms, 700);
        assert!(Arc::ptr_eq(&accepted, &p.frame));
        assert_eq!(view.estimate.unwrap().size_bytes, 1234);
        assert!(!view.dirty() && !view.history_changed);
    }

    #[test]
    fn seek_after_playback_replaces_transient_frame_and_resume_position() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.preview_loop.store(true, Ordering::Relaxed);
        view.preview_sound = true;
        let (tx, jobs) = mpsc::channel();
        view.request_playback(&tx);
        jobs.recv().unwrap();
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1800,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.pause_playback();
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 950 }),
        );
        assert!(matches!(jobs.recv().unwrap(), Job::Apply(_)));
        let mut presentation = opened().presented.unwrap();
        presentation.position_ms = 950;
        view.receive(&ctx, Event::Presented(Ok(presentation)));
        assert_eq!(view.texture.as_ref().unwrap().size(), [4, 2]);
        assert_eq!(view.position_ms, 950);
        assert!(view.playback_position_ms.is_none() && !view.playback_ended);
        assert!(
            view.preview_loop.load(Ordering::Relaxed),
            "Pause/seek retains the loop preference"
        );
        assert!(
            view.preview_sound,
            "Pause/seek retains the sound preference"
        );
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(950, true, _)));
        assert!(!view.dirty());
    }

    #[test]
    fn sound_metadata_cancellation_and_device_error_preserve_accepted_state() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.estimate = Some(ExportEstimate {
            size_bytes: 5678,
            exact: true,
        });
        view.preview_sound = true;
        let (tx, jobs) = mpsc::channel();
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(700, true, _)));
        view.receive(
            &ctx,
            Event::PlaybackStarted {
                audio_enabled: true,
            },
        );
        assert!(
            view.playback_audio_enabled && view.status.as_ref().unwrap().contains("default output")
        );
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1337,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.pause_playback();
        view.receive(
            &ctx,
            Event::PlaybackStarted {
                audio_enabled: false,
            },
        );
        assert!(
            view.playback_audio_enabled,
            "late metadata is ignored after Pause"
        );
        assert_eq!(
            view.position_ms, 1337,
            "metadata never changes presentation time"
        );
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        assert!(view.preview_sound && !view.playback_audio_enabled);
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(1337, true, _)));
        view.receive(
            &ctx,
            Event::PlaybackStarted {
                audio_enabled: false,
            },
        );
        assert!(!view.playback_audio_enabled);
        assert!(view.status.as_ref().unwrap().contains("has no sound"));
        view.receive(
            &ctx,
            Event::PlaybackFinished(Err("output device unavailable".into())),
        );
        assert!(view.preview_sound && view.status.is_none());
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("output device unavailable")
        );
        assert_eq!(view.position_ms, 700);
        assert!(
            jobs.try_recv().is_err(),
            "device failure never silently retries without sound"
        );
        view.preview_sound = false;
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(700, false, _)));
        assert!(view.error.is_none());
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.estimate.unwrap().size_bytes, 5678);
        assert!(!view.dirty() && !view.unapplied() && !view.history_changed);
        assert!(
            !opened().preview_sound,
            "new items default to silent playback"
        );
    }

    #[test]
    fn playback_error_restores_still_and_close_retains_dirty_confirmation() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let (tx, jobs) = mpsc::channel();
        view.start_ms = 123;
        view.request_playback(&tx);
        assert!(jobs.try_recv().is_err(), "unapplied fields block Play");
        view.presented.as_mut().unwrap().edit.trim_start_ms = 123;
        assert!(view.dirty() && !view.unapplied());
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(700, false, _)));
        view.receive_playback_frame(
            &ctx,
            PlaybackFrame {
                position_ms: 1500,
                pixels: Arc::new(RgbaImage::new(2, 1)),
            },
        );
        view.receive(&ctx, Event::PlaybackFinished(Err("source removed".into())));
        assert_eq!(view.position_ms, 700);
        assert_eq!(view.texture.as_ref().unwrap().size(), [4, 2]);
        assert!(view.playback_position_ms.is_none() && view.dirty());
        assert!(view.error.as_ref().unwrap().contains("source removed"));
        view.request_playback(&tx);
        let Job::Play(_, false, cancel) = jobs.recv().unwrap() else {
            panic!("retry queued")
        };
        assert!(!cancel.is_cancelled() && view.error.is_none());
        view.request_close();
        assert!(cancel.is_cancelled() && !view.closed && !view.confirm_close);
        view.receive(&ctx, Event::PlaybackFinished(Ok(PlaybackEnd::Paused)));
        assert!(
            view.confirm_close && !view.closed,
            "only ask discard after playback teardown"
        );
    }

    #[test]
    fn playback_mailbox_is_latest_only_and_drains_before_completion() {
        let ctx = egui::Context::default();
        let (tx, jobs) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let mut view = opened();
        view.request_playback(&tx);
        assert!(matches!(jobs.recv().unwrap(), Job::Play(_, false, _)));
        let editor = Editor {
            viewport: egui::ViewportId::ROOT,
            view: Arc::new(Mutex::new(view)),
            playback_frame: Arc::new(Mutex::new(None)),
            tx,
            events,
            rx,
            worker: None,
        };
        let old = Arc::new(RgbaImage::new(2, 1));
        *editor.playback_frame.lock().unwrap() = Some(PlaybackFrame {
            position_ms: 900,
            pixels: old.clone(),
        });
        *editor.playback_frame.lock().unwrap() = Some(PlaybackFrame {
            position_ms: 3066,
            pixels: Arc::new(RgbaImage::new(2, 1)),
        });
        assert_eq!(
            Arc::strong_count(&old),
            1,
            "replacing latest promptly releases superseded pixels"
        );
        editor
            .events
            .send(Event::PlaybackFinished(Ok(PlaybackEnd::Ended)))
            .unwrap();
        editor.receive(&ctx);
        assert_eq!(editor.view.lock().unwrap().position_ms, 3066);
        assert!(editor.playback_frame.lock().unwrap().is_none());
        assert!(
            editor.flush(&ctx).is_ok(),
            "motion alone never blocks clean quit"
        );
        editor.view.lock().unwrap().request_playback(&editor.tx);
        let Job::Play(_, false, cancel) = jobs.recv().unwrap() else {
            panic!("replay")
        };
        assert!(editor.flush(&ctx).is_err());
        assert!(
            cancel.is_cancelled(),
            "quit requests teardown without freeing a live worker"
        );
        editor
            .events
            .send(Event::PlaybackFinished(Ok(PlaybackEnd::Paused)))
            .unwrap();
        editor.receive(&ctx);
        assert!(editor.flush(&ctx).is_ok());
        editor.view.lock().unwrap().request_playback(&editor.tx);
        editor.view.lock().unwrap().request_close();
        editor
            .events
            .send(Event::PlaybackFinished(Ok(PlaybackEnd::Paused)))
            .unwrap();
        editor.receive(&ctx);
        assert!(
            editor.closed(),
            "clean close finishes only after worker completion"
        );
    }

    #[test]
    fn thumbnail_cancel_failure_retry_keeps_edits_preview_and_estimate() {
        let ctx = egui::Context::default();
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.estimate = Some(ExportEstimate {
            size_bytes: 987,
            exact: true,
        });
        let (tx, jobs) = mpsc::channel();
        view.request_thumbnails(&tx);
        let Job::Thumbnails(cancel) = jobs.recv().unwrap() else {
            panic!("thumbnails queued")
        };
        assert!(view.busy && view.loading_thumbnails && !view.dirty());
        view.request_thumbnails(&tx);
        view.request_estimate(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "generation is serialized with other media work"
        );
        view.request_close();
        assert!(!view.closed && !view.confirm_close);
        view.cancel.as_ref().unwrap().cancel();
        assert!(cancel.is_cancelled());
        view.receive(&ctx, Event::Thumbnails(Err("cancelled".into())));
        assert!(!view.busy && !view.loading_thumbnails && view.cancel.is_none());
        assert!(
            view.error.is_none(),
            "the busy-close warning no longer applies"
        );
        assert_eq!(view.thumbnail_error.as_deref(), Some("cancelled"));
        assert!(!view.dirty() && !view.history_changed);
        assert_eq!(view.estimate.unwrap().size_bytes, 987);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));

        view.start_ms = 300;
        view.request_thumbnails(&tx);
        let Job::Thumbnails(retry) = jobs.recv().unwrap() else {
            panic!("retry queued")
        };
        assert!(!retry.is_cancelled() && view.thumbnail_error.is_none());
        view.receive(&ctx, Event::Thumbnails(Err("source missing".into())));
        assert!(view.dirty() && view.unapplied());
        assert_eq!(
            (view.start_ms, view.end_ms, view.position_ms),
            (300, 3100, 700)
        );
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.thumbnail_error.as_deref(), Some("source missing"));
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 1000 }),
        );
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Apply(_))),
            "thumbnail failure does not disable the worker"
        );
    }

    #[test]
    fn thumbnails_queue_only_on_first_presentation_and_never_reuse_another_editors_texture() {
        let ctx = egui::Context::default();
        let (tx, jobs) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let editor = Editor {
            viewport: egui::ViewportId::ROOT,
            view: Arc::new(Mutex::new(View {
                busy: true,
                ..View::default()
            })),
            playback_frame: Arc::new(Mutex::new(None)),
            tx,
            events,
            rx,
            worker: None,
        };
        editor
            .events
            .send(Event::Presented(Ok(opened().presented.take().unwrap())))
            .unwrap();
        editor.receive(&ctx);
        assert!(matches!(jobs.try_recv(), Ok(Job::Thumbnails(_))));
        assert!(editor.flush(&ctx).is_err(), "quit waits for generation");
        editor
            .events
            .send(Event::Thumbnails(Err("cancelled".into())))
            .unwrap();
        editor.receive(&ctx);
        assert!(editor.flush(&ctx).is_ok());
        // Uploads are covered by the real X11 test. Retention across published
        // edits/seek must not enqueue another decode or replace this texture.
        let texture = ctx.load_texture(
            "test-source-strip",
            egui::ColorImage::filled([12, 2], egui::Color32::RED),
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        editor.view.lock().unwrap().thumbnails = Some(texture);
        editor
            .events
            .send(Event::Presented(Ok(opened().presented.take().unwrap())))
            .unwrap();
        editor.receive(&ctx);
        assert!(jobs.try_recv().is_err());
        let mut view = editor.view.lock().unwrap();
        assert_eq!(view.thumbnails.as_ref().unwrap().id(), id);
        view.request_thumbnails(&editor.tx);
        assert!(
            jobs.try_recv().is_err(),
            "successful strips are retained for this source"
        );
        assert!(
            View::default().thumbnails.is_none(),
            "new editor/source starts without stale pixels"
        );
    }

    #[test]
    fn cancelled_estimate_preserves_frame_and_can_be_retried() {
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let (tx, jobs) = mpsc::channel();
        view.request_estimate(&tx);
        let Job::Estimate(cancel) = jobs.recv().unwrap() else {
            panic!("estimate queued")
        };
        view.cancel.as_ref().unwrap().cancel();
        assert!(cancel.is_cancelled() && view.busy && view.estimating);
        view.receive(
            &egui::Context::default(),
            Event::Estimated(Err("cancelled".into())),
        );
        assert!(!view.busy && !view.estimating && view.cancel.is_none() && view.estimate.is_none());
        assert_eq!(view.error.as_deref(), Some("cancelled"));
        assert!(!view.dirty() && !view.history_changed);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        view.request_estimate(&tx);
        let Job::Estimate(retry) = jobs.recv().unwrap() else {
            panic!("retry queued")
        };
        assert!(!retry.is_cancelled() && view.error.is_none());
    }

    #[test]
    fn estimate_and_save_controls_fit_the_minimum_window() {
        for (name, tokens) in crate::tokens::load() {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            view.presented.as_mut().unwrap().source.size_bytes = 200_000_000_000;
            view.estimate = Some(ExportEstimate {
                size_bytes: 123456789012,
                exact: true,
            });
            view.thumbnail_error = Some("source missing".into());
            view.saved_path = Some("/exports/saved.mp4".into());
            view.start_ms = 1550;
            view.end_ms = 1551;
            let accepted = &mut view.presented.as_mut().unwrap().edit;
            accepted.trim_start_ms = 1550;
            accepted.trim_end_ms = Some(1551);
            for pass in 0..4 {
                view.estimate.as_mut().unwrap().exact = pass < 2;
                // The toolbar and save footer fit the minimum window; the
                // page scrolls to the option cards, checked at minimum width.
                let (_, controls) =
                    probe_frame(&ctx, &tokens, &mut view, egui::vec2(760., 580.), vec![]);
                let (output, cards) =
                    probe_frame(&ctx, &tokens, &mut view, egui::vec2(760., 1800.), vec![]);
                if pass % 2 == 0 {
                    continue;
                }
                for (controls, labels, height) in [
                    (
                        &controls,
                        &[
                            "Play preview",
                            "Sound",
                            "Compare",
                            "Loop preview",
                            "Fit",
                            "100%",
                            "Show in Folder",
                            "Replace original…",
                            "Apply edits",
                            "Save new copy",
                        ][..],
                        580.,
                    ),
                    (
                        &cards,
                        &[
                            "Timeline track",
                            "Retry thumbnails",
                            "Est. size",
                            "Est. size delta",
                            "Estimate size",
                        ][..],
                        1800.,
                    ),
                ] {
                    let mut rects = Vec::new();
                    for label in labels {
                        let rect = probed(controls, label);
                        assert!(
                            rect.width() > 0.
                                && rect.left() >= 0.
                                && rect.right() <= 760.
                                && rect.top() >= 0.
                                && rect.bottom() <= height,
                            "{name}: {label} outside window: {rect:?}"
                        );
                        for other in &rects {
                            assert!(
                                !rect.shrink(0.5).intersects(*other),
                                "{name}: overlapping {label} {rect:?} {other:?}"
                            );
                        }
                        rects.push(rect);
                    }
                }
                let track = probed(&cards, "Timeline track");
                for grip in ["Trim start", "Trim end"] {
                    assert!(
                        !probed(&cards, "Retry thumbnails").intersects(probed(&cards, grip)),
                        "retry must never overlay either grip, even for a 1 ms middle selection"
                    );
                    assert!(probed(&cards, grip).intersects(track.expand(tokens.number("s-6"))));
                }
                let label = if pass < 2 { "123 GB" } else { "≈ 123 GB" };
                assert!(
                    output.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Text(text) if text.galley.job.text == label)),
                    "{name}: missing {label}"
                );
                assert!(
                    output.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Text(text) if text.galley.job.text == "−38%")),
                    "{name}: missing delta"
                );
            }
        }
    }

    #[test]
    fn preview_toggles_are_transient_and_sound_waits_for_worker_teardown() {
        for label in ["Loop preview", "Sound"] {
            for state in 0..6 {
                let tokens = crate::tokens::load().remove("light-mustard").unwrap();
                let ctx = egui::Context::default();
                tokens.apply(&ctx, true);
                let mut view = opened();
                let accepted = view.presented.as_ref().unwrap().frame.clone();
                view.estimate = Some(ExportEstimate {
                    size_bytes: 4321,
                    exact: true,
                });
                let (tx, _jobs) = mpsc::channel();
                let (events, _) = mpsc::channel();
                assert!(!view.preview_loop.load(Ordering::Relaxed));
                if state == 1 || state == 2 {
                    view.request_playback(&tx);
                }
                if state == 2 {
                    view.pause_playback();
                }
                view.busy |= state == 3;
                view.picker = state == 4;
                view.confirm_close = state == 5;
                let mut toggle = egui::Pos2::ZERO;
                for pass in 0..3 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(760., 580.),
                            )),
                            events: if pass == 2 {
                                vec![
                                    egui::Event::PointerMoved(toggle),
                                    trim_pointer(toggle, true),
                                    trim_pointer(toggle, false),
                                ]
                            } else {
                                Vec::new()
                            },
                            ..Default::default()
                        },
                        |ui| show(ui, &tokens, &mut view, &tx, &events, egui::ViewportId::ROOT),
                    );
                    output.textures_delta.clear();
                    if pass == 1 {
                        toggle = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.job.text == label => {
                                    Some(text.pos + text.galley.rect.center().to_vec2())
                                }
                                _ => None,
                            })
                            .expect("preview control visible at minimum size");
                    }
                }
                assert_eq!(
                    if label == "Sound" {
                        view.preview_sound
                    } else {
                        view.preview_loop.load(Ordering::Relaxed)
                    },
                    state == 0 || (label == "Loop preview" && state == 1),
                    "{label}: sound changes only idle; loop also changes playing; both wait for teardown/other work"
                );
                assert!(!view.dirty() && !view.unapplied());
                assert_eq!(view.estimate.as_ref().unwrap().size_bytes, 4321);
                assert!(Arc::ptr_eq(
                    &accepted,
                    &view.presented.as_ref().unwrap().frame
                ));
                assert!(
                    !opened().preview_loop.load(Ordering::Relaxed),
                    "new editors default to non-looping playback"
                );
                assert!(!opened().preview_sound);
            }
        }
    }

    #[test]
    fn fixed_pause_is_clickable_while_worker_owns_the_minimum_window() {
        for (name, tokens) in crate::tokens::load() {
            let ctx = egui::Context::default();
            tokens.apply(&ctx, name.contains("light"));
            let mut view = opened();
            let (tx, jobs) = mpsc::channel();
            let (events, _) = mpsc::channel();
            view.request_playback(&tx);
            let Job::Play(_, false, cancel) = jobs.recv().unwrap() else {
                panic!("play")
            };
            let mut pause = egui::Pos2::ZERO;
            for pass in 0..3 {
                let input = if pass == 2 {
                    vec![
                        egui::Event::PointerMoved(pause),
                        trim_pointer(pause, true),
                        trim_pointer(pause, false),
                    ]
                } else {
                    Vec::new()
                };
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(760., 580.),
                        )),
                        events: input,
                        ..Default::default()
                    },
                    |ui| show(ui, &tokens, &mut view, &tx, &events, egui::ViewportId::ROOT),
                );
                output.textures_delta.clear();
                if pass == 1 {
                    let rect = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == "Pause playback" => {
                                Some(text.galley.rect.translate(text.pos.to_vec2()))
                            }
                            _ => None,
                        })
                        .expect("fixed footer pause");
                    assert!(
                        rect.left() >= 0.
                            && rect.right() <= 760.
                            && rect.top() > 400.
                            && rect.bottom() < 580.,
                        "{name}: {rect:?}"
                    );
                    pause = rect.center();
                }
            }
            assert!(
                cancel.is_cancelled() && view.busy && view.playing,
                "Pause stays enabled but never releases a live worker"
            );
            assert!(
                jobs.try_recv().is_err(),
                "Pause cancels directly, not behind the active Play job"
            );
        }
    }

    #[test]
    fn failed_seek_retains_frame_position_and_failed_trim_stays_dirty() {
        let mut view = opened();
        let frame = view.presented.as_ref().unwrap().frame.clone();
        view.position_ms = 2900;
        view.start_ms = 1800;
        view.end_ms = 1100;
        view.receive(
            &egui::Context::default(),
            Event::Presented(Err("invalid trim".into())),
        );
        assert_eq!(view.position_ms, 700);
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!((view.start_ms, view.end_ms), (1800, 1100));
        assert!(view.dirty());
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn crop_and_size_are_staged_atomically_and_failures_keep_the_accepted_preview() {
        let mut view = opened();
        let crop = CropRect {
            x: 2,
            y: 0,
            width: 2,
            height: 2,
        };
        view.crop = Some(crop);
        view.output_size = Some((8, 6));
        view.start_ms = 300;
        view.end_ms = 2200;
        let p = view.presented.as_ref().unwrap();
        let frame = p.frame.clone();
        let edit = view.staged_edit(p);
        assert_eq!(
            edit,
            EditSpec {
                trim_start_ms: 300,
                trim_end_ms: Some(2200),
                crop: Some(crop),
                output_width: Some(8),
                output_height: Some(6),
                audio: p.edit.audio.clone(),
            }
        );
        assert!(view.unapplied() && view.dirty());
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("decode failed".into())));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.crop, Some(crop));
        assert_eq!(view.output_size, Some((8, 6)));
        assert!(view.unapplied());
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                revision: 2,
                preview_export: view.export_spec(),
                source: view.presented.as_ref().unwrap().source.clone(),
                edit,
                export: view.export_spec(),
                position_ms: 700,
                frame: Arc::new(RgbaImage::new(8, 6)),
                dropped_frames: 0,
            })),
        );
        assert!(!view.unapplied() && view.dirty());
        // Removing one transform must retain the accepted trim and other transform.
        view.crop = None;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert!(view.unapplied());
        assert_eq!((edit.trim_start_ms, edit.trim_end_ms), (300, Some(2200)));
        assert_eq!((edit.output_width, edit.output_height), (Some(8), Some(6)));
        assert_eq!(edit.crop, None);
        view.crop = Some(crop);
        view.output_size = None;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(edit.crop, Some(crop));
        assert_eq!((edit.output_width, edit.output_height), (None, None));
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn locked_crop_dimensions_follow_current_ratio_and_origin_bounds() {
        for (source, initial, horizontal, value, expected) in [
            ((320, 180), (0, 0, 320, 180), true, 160, (160, 90)),
            ((640, 1440), (0, 0, 640, 1440), false, 720, (320, 720)),
            ((320, 180), (10, 60, 160, 90), true, 300, (213, 120)),
            ((320, 180), (200, 6, 80, 60), false, 170, (120, 90)),
            ((320, 180), (10, 6, 160, 90), true, 0, (2, 2)),
            ((320, 180), (10, 6, 90, 160), false, 0, (2, 2)),
            ((400, 300), (20, 30, 101, 61), true, 73, (73, 44)),
        ] {
            let mut crop = CropRect {
                x: initial.0,
                y: initial.1,
                width: initial.2,
                height: initial.3,
            };
            crop = crop.resize_aspect_locked(
                source.0,
                source.1,
                if horizontal {
                    CropResizeAxis::Width
                } else {
                    CropResizeAxis::Height
                },
                value,
            );
            assert_eq!((crop.width, crop.height), expected);
            assert_eq!((crop.x, crop.y), (initial.0, initial.1));
        }
        let mut view = opened();
        assert!(
            !view.crop_aspect_unlocked,
            "crop starts locked, as in Tauri"
        );
        view.crop_aspect_unlocked = true;
        assert!(!view.dirty(), "the input preference alone is not an edit");
    }

    #[test]
    fn resolution_presets_use_crop_and_remain_presets_after_acceptance() {
        let mut view = opened();
        let p = view.presented.as_mut().unwrap();
        p.source.width = 4001;
        p.source.height = 2003;
        let frame = p.frame.clone();
        view.crop = Some(CropRect {
            x: 20,
            y: 30,
            width: 1001,
            height: 1501,
        });
        view.max_resolution = MaxResolution::P720;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(
            (edit.output_width, edit.output_height),
            (Some(480), Some(720))
        );
        assert!(view.unapplied());
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("preview failed".into())));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.max_resolution, MaxResolution::P720);
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                revision: 2,
                preview_export: view.export_spec(),
                source: view.presented.as_ref().unwrap().source.clone(),
                edit,
                export: view.export_spec(),
                position_ms: 700,
                frame: Arc::new(RgbaImage::new(480, 720)),
                dropped_frames: 0,
            })),
        );
        assert_eq!(view.max_resolution, MaxResolution::P720);
        assert!(
            view.output_size.is_none(),
            "acceptance must not turn a preset into a custom size"
        );
        assert!(!view.unapplied() && view.dirty());
        view.crop.as_mut().unwrap().height = 501;
        assert_eq!(view.output_dimensions((4001, 2003)), Some((1000, 500)));
        assert!(
            view.unapplied(),
            "crop changes recalculate the preset without upscaling"
        );
        view.output_size = Some((81, 61));
        assert_eq!(view.output_dimensions((4001, 2003)), Some((81, 61)));
        view.output_size = None;
        view.max_resolution = MaxResolution::Original;
        assert_eq!(view.output_dimensions((4001, 2003)), None);
    }

    #[test]
    fn resolution_caps_preserve_orientation_round_even_and_do_not_upscale() {
        for (preset, source, expected) in [
            (MaxResolution::P1080, (4001, 2003), (2156, 1080)),
            (MaxResolution::P720, (4001, 2003), (1438, 720)),
            (MaxResolution::P1080, (1001, 2003), (540, 1080)),
            (MaxResolution::P720, (1283, 721), (1280, 720)),
            (MaxResolution::P720, (1283, 719), (1282, 718)),
        ] {
            let view = View {
                max_resolution: preset,
                ..View::default()
            };
            assert_eq!(view.output_dimensions(source), Some(expected));
        }
    }

    #[test]
    fn gif_width_derives_from_even_crop_preset_or_custom_base_without_upscaling() {
        let mut view = opened();
        view.gif = true;
        assert_eq!(view.gif_maximum_width.unwrap_or(800), 800);
        view.output_size = Some((1601, 901));
        for (maximum, expected) in [
            (320, (320, 180)),
            (480, (480, 270)),
            (640, (640, 360)),
            (800, (800, 450)),
            (1200, (1200, 674)),
        ] {
            view.gif_maximum_width = Some(maximum);
            let edit = view.staged_edit(view.presented.as_ref().unwrap());
            assert_eq!(edit.output_width.zip(edit.output_height), Some(expected));
        }
        view.gif_maximum_width = Some(320);
        for (base, expected) in [
            ((301, 151), (300, 150)), // Never upscale; normalize both axes first.
            ((9984, 234), (320, 6)),  // Shipping height * (cap / width), not height * cap / width.
            ((1600, 2), (320, 2)),    // Preserve the encoder's minimum dimension.
        ] {
            view.output_size = Some(base);
            let edit = view.staged_edit(view.presented.as_ref().unwrap());
            assert_eq!(edit.output_width.zip(edit.output_height), Some(expected));
        }
        view.output_size = None;
        view.crop = Some(CropRect {
            x: 17,
            y: 29,
            width: 501,
            height: 1001,
        });
        view.gif_maximum_width = Some(800);
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(edit.output_width.zip(edit.output_height), Some((500, 1000)));
        view.max_resolution = MaxResolution::P720;
        view.gif_maximum_width = Some(320);
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(edit.output_width.zip(edit.output_height), Some((320, 640)));
    }

    #[test]
    fn gif_width_acceptance_keeps_uncapped_base_for_repeat_changes_seek_and_mp4() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.gif = true;
        view.output_size = Some((1601, 901));
        for (maximum, expected) in [(800, (800, 450)), (1200, (1200, 674)), (320, (320, 180))] {
            view.gif_maximum_width = Some(maximum);
            assert!(view.unapplied() && view.dirty());
            let (tx, jobs) = mpsc::channel();
            view.request_estimate(&tx);
            view.request_playback(&tx);
            assert!(jobs.try_recv().is_err());
            let mut p = opened().presented.unwrap();
            p.edit = view.staged_edit(view.presented.as_ref().unwrap());
            p.export = view.export_spec();
            assert_eq!(
                p.edit.output_width.zip(p.edit.output_height),
                Some(expected)
            );
            view.receive(&ctx, Event::Presented(Ok(p)));
            assert!(!view.unapplied() && view.dirty());
            assert_eq!(view.output_size, Some((1601, 901)));
        }
        view.receive(
            &ctx,
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "width.gif".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert!(!view.dirty());
        let accepted = view.presented.as_ref().unwrap();
        let frame = accepted.frame.clone();
        let edit = accepted.edit.clone();
        view.gif_maximum_width = Some(1200);
        view.receive(&ctx, Event::Presented(Err("preview failed".into())));
        assert!(Arc::ptr_eq(&frame, &view.presented.as_ref().unwrap().frame));
        assert_eq!(view.presented.as_ref().unwrap().edit, edit);
        assert_eq!(view.gif_maximum_width, Some(1200));
        assert!(view.unapplied());
        view.gif_maximum_width = Some(320);
        let mut seek = opened().presented.unwrap();
        seek.edit = edit;
        seek.export = view.export_spec();
        seek.position_ms = 1391;
        view.receive(&ctx, Event::Presented(Ok(seek)));
        assert!(!view.dirty() && !view.unapplied());
        assert_eq!(view.output_size, Some((1601, 901)));
        view.gif = false;
        let mut mp4 = opened().presented.unwrap();
        mp4.edit = view.staged_edit(view.presented.as_ref().unwrap());
        mp4.export = view.export_spec();
        assert_eq!(
            mp4.edit.output_width.zip(mp4.edit.output_height),
            Some((1601, 901))
        );
        view.receive(&ctx, Event::Presented(Ok(mp4)));
        assert!(!view.unapplied());
        view.gif = true;
        assert_eq!(view.gif_maximum_width, Some(320));
        assert_eq!(
            view.staged_edit(view.presented.as_ref().unwrap())
                .output_width,
            Some(320)
        );
        assert_eq!(
            opened().gif_maximum_width,
            None,
            "each new editor resets to 800"
        );
    }

    #[test]
    fn audio_changes_stage_with_geometry_and_survive_failed_apply_and_gif() {
        let mut view = opened();
        let p = view.presented.as_mut().unwrap();
        p.edit.audio.source_has_system_audio = true;
        p.edit.audio.source_has_microphone_audio = true;
        view.audio = p.edit.audio.clone();
        view.saved_edit = p.edit.clone();
        let frame = p.frame.clone();
        view.audio.system_volume = 0.25;
        view.audio.microphone_volume = 1.75;
        view.audio.mute_system_audio = true;
        view.audio.mono_output = true;
        view.start_ms = 300;
        let edit = view.staged_edit(view.presented.as_ref().unwrap());
        assert_eq!(edit.trim_start_ms, 300);
        assert_eq!(
            edit.audio,
            AudioEdit {
                system_volume: 0.25,
                microphone_volume: 1.75,
                mute_system_audio: true,
                mute_microphone: false,
                mono_output: true,
                source_has_system_audio: true,
                source_has_microphone_audio: true,
            }
        );
        assert!(view.unapplied() && view.dirty());
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("decode failed".into())));
        let p = view.presented.as_ref().unwrap();
        assert!(Arc::ptr_eq(&frame, &p.frame));
        assert_eq!(p.edit.audio.system_volume, 1.);
        assert!(!p.edit.audio.mute_system_audio && !p.edit.audio.mono_output);
        assert_eq!(view.audio, edit.audio);
        let source = p.source.clone();
        view.gif = true;
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                revision: 2,
                preview_export: view.export_spec(),
                source,
                edit: view.staged_edit(view.presented.as_ref().unwrap()),
                export: view.export_spec(),
                position_ms: 700,
                frame,
                dropped_frames: 0,
            })),
        );
        assert!(!view.unapplied() && view.dirty());
        view.gif = false;
        assert_eq!(
            view.staged_edit(view.presented.as_ref().unwrap()).audio,
            edit.audio
        );
        assert!(view.unapplied());
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn gif_frame_rate_is_accepted_output_state_and_retained_across_mp4() {
        let ctx = egui::Context::default();
        let mut view = opened();
        view.gif = true;
        assert_eq!(view.gif_frames_per_second.unwrap_or(15), 15);
        view.gif_frames_per_second = Some(8);
        view.estimate = Some(ExportEstimate {
            size_bytes: 1234,
            exact: true,
        });
        assert!(view.unapplied() && view.dirty());
        let (tx, jobs) = mpsc::channel();
        view.request_estimate(&tx);
        view.request_playback(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "staged FPS gates estimate and playback"
        );
        assert_eq!(view.estimate_label(), "Apply edits to estimate");
        let export = view.export_spec();
        assert_eq!(export.format, ExportFormat::Gif);
        assert_eq!(export.frames_per_second, Some(8));
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::UpdatePreview {
                edit: view.staged_edit(view.presented.as_ref().unwrap()),
                export,
            }),
        );
        let Job::Apply(RecordingEditorRequest::UpdatePreview { edit, export }) =
            jobs.recv().unwrap()
        else {
            panic!("frame rate is sent through Apply")
        };
        let mut p = opened().presented.unwrap();
        p.edit = edit;
        p.export = export;
        view.receive(&ctx, Event::Presented(Ok(p)));
        assert!(!view.unapplied() && view.dirty() && view.estimate.is_none());
        view.receive(
            &ctx,
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "eight-fps.gif".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert!(!view.dirty());
        let accepted_frame = view.presented.as_ref().unwrap().frame.clone();
        view.gif_frames_per_second = Some(24);
        view.receive(&ctx, Event::Presented(Err("preview failed".into())));
        assert!(Arc::ptr_eq(
            &accepted_frame,
            &view.presented.as_ref().unwrap().frame
        ));
        assert_eq!(
            view.gif_frames_per_second,
            Some(24),
            "failed Apply keeps the correction available"
        );
        assert_eq!(
            view.presented.as_ref().unwrap().export.frames_per_second,
            Some(8)
        );
        view.gif_frames_per_second = Some(8);
        let mut seek = opened().presented.unwrap();
        seek.edit = view.presented.as_ref().unwrap().edit.clone();
        seek.export = view.export_spec();
        seek.position_ms = 1377;
        view.receive(&ctx, Event::Presented(Ok(seek)));
        assert!(!view.dirty() && !view.unapplied());
        assert_eq!(view.gif_frames_per_second, Some(8));
        view.gif = false;
        assert_eq!(
            view.export_spec().frames_per_second,
            None,
            "GIF FPS never changes MP4 cadence"
        );
        let mut mp4 = opened().presented.unwrap();
        mp4.export = view.export_spec();
        view.receive(&ctx, Event::Presented(Ok(mp4)));
        assert_eq!(
            view.gif_frames_per_second,
            Some(8),
            "MP4 acceptance retains the GIF choice"
        );
        view.gif = true;
        assert_eq!(view.export_spec().frames_per_second, Some(8));
        assert_eq!(
            opened().gif_frames_per_second,
            None,
            "new items start at the 15 FPS default"
        );
        assert!(!view.history_changed);
    }

    #[test]
    fn gif_quality_controls_palette_even_when_maximum_uses_preserve() {
        let ctx = egui::Context::default();
        for (quality, colors) in [
            (QualityPreset::Tiny, 64),
            (QualityPreset::Small, 96),
            (QualityPreset::Standard, 128),
            (QualityPreset::High, 256),
            (QualityPreset::Highest, 256),
            (QualityPreset::Preserve, 256),
        ] {
            let mut view = opened();
            view.gif = true;
            view.quality = quality;
            assert_eq!(view.export_spec().gif_max_colors, Some(colors));
            view.maximum_size = true;
            let export = view.export_spec();
            assert_eq!(export.quality, QualityPreset::Preserve);
            assert_eq!(export.gif_max_colors, Some(colors));
            let mut accepted = opened().presented.unwrap();
            accepted.edit = view.staged_edit(view.presented.as_ref().unwrap());
            accepted.export = export.clone();
            view.receive(&ctx, Event::Presented(Ok(accepted)));
            assert!(!view.unapplied());
            assert_eq!(view.export_spec(), export);
            view.receive(&ctx, Event::Presented(Err("preview failed".into())));
            assert_eq!(view.presented.as_ref().unwrap().export, export);
            assert_eq!(view.export_spec(), export);
            view.gif = false;
            assert_eq!(view.export_spec().gif_max_colors, None);
            let mut mp4 = opened().presented.unwrap();
            mp4.export = view.export_spec();
            view.receive(&ctx, Event::Presented(Ok(mp4)));
            view.gif = true;
            assert_eq!(view.export_spec().gif_max_colors, Some(colors));
            view.maximum_size = false;
            assert_eq!(view.export_spec().quality, quality);
            assert_eq!(view.export_spec().gif_max_colors, Some(colors));
        }
    }

    #[test]
    fn format_and_quality_changes_require_preview_acceptance_and_a_successful_save() {
        let mut view = opened();
        assert!(!view.dirty());
        view.gif = true;
        view.quality = QualityPreset::High;
        assert!(view.unapplied() && view.dirty());
        let frame = view.presented.as_ref().unwrap().frame.clone();
        let ctx = egui::Context::default();
        view.receive(&ctx, Event::Presented(Err("preview failed".into())));
        let p = view.presented.as_ref().unwrap();
        assert!(Arc::ptr_eq(&frame, &p.frame));
        assert_eq!(p.export.format, ExportFormat::Mp4);
        assert_eq!(p.export.quality, QualityPreset::Preserve);
        assert!(view.gif && view.quality == QualityPreset::High && view.unapplied());
        let export = ExportSpec {
            format: ExportFormat::Gif,
            quality: QualityPreset::High,
            max_size_bytes: None,
            frames_per_second: None,
            gif_max_colors: Some(256),
        };
        view.receive(
            &ctx,
            Event::Presented(Ok(Presented {
                revision: p.revision + 1,
                preview_export: export.clone(),
                source: p.source.clone(),
                edit: view.staged_edit(p),
                position_ms: p.position_ms,
                frame,
                export: export.clone(),
                dropped_frames: 0,
            })),
        );
        assert!(
            !view.unapplied() && view.dirty(),
            "format-only accepted work is unsaved"
        );
        view.receive(&ctx, Event::Saved(Err("destination exists".into())));
        assert!(view.dirty());
        view.receive(
            &ctx,
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "saved.gif".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert_eq!(view.saved_export, Some(export));
        assert!(!view.dirty());
        view.quality = QualityPreset::Standard;
        assert!(
            view.unapplied(),
            "quality-only changes also need preview acceptance"
        );
        view.request_close();
        assert!(view.confirm_close && !view.closed);
    }

    #[test]
    fn close_blocks_accepted_work_and_save_warning_is_not_failure() {
        let mut view = opened();
        view.busy = true;
        view.request_close();
        assert!(!view.closed && !view.confirm_close);
        let edit = EditSpec {
            trim_start_ms: 300,
            trim_end_ms: Some(2200),
            ..EditSpec::default()
        };
        view.presented.as_mut().unwrap().edit = edit.clone();
        view.start_ms = 300;
        view.end_ms = 2200;
        view.receive(
            &egui::Context::default(),
            Event::Saved(Ok(SavedRecording::SavedWithoutHistory {
                path: "saved.mp4".into(),
                warning: "History unavailable".into(),
            })),
        );
        assert_eq!(view.saved_edit, edit);
        assert!(!view.dirty() && !view.history_changed);
        assert!(view.status.as_ref().unwrap().contains("saved.mp4"));
        assert_eq!(view.error.as_deref(), Some("History unavailable"));
        view.request_close();
        assert!(view.closed);
    }

    #[test]
    fn worker_queue_accepts_only_one_operation() {
        let (tx, rx) = mpsc::channel();
        let mut view = opened();
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 900 }),
        );
        view.send(
            &tx,
            Job::Apply(RecordingEditorRequest::Seek { position_ms: 1200 }),
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(RecordingEditorRequest::Seek {
                position_ms: 900
            }))
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn error_and_cancel_render_in_every_theme_and_cancel_keeps_work_pending() {
        for (name, tokens) in crate::tokens::load() {
            for estimating in [false, true] {
                let ctx = egui::Context::default();
                tokens.apply(&ctx, name.contains("light"));
                let mut view = opened();
                view.error = Some("Export cannot replace an existing recording.".into());
                view.busy = true;
                view.estimating = estimating;
                let cancel_label = if estimating {
                    "Cancel estimate"
                } else {
                    "Cancel export"
                };
                let cancel = CancelToken::default();
                view.cancel = Some(cancel.clone());
                let (tx, jobs) = mpsc::channel();
                let (events, _) = mpsc::channel();
                let frame = |view: &mut View, input| {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(760., 580.),
                            )),
                            events: input,
                            ..Default::default()
                        },
                        |ui| show(ui, &tokens, view, &tx, &events, egui::ViewportId::ROOT),
                    );
                    output.textures_delta.clear();
                    output
                };
                frame(&mut view, vec![]);
                let output = frame(&mut view, vec![]);
                let button = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == cancel_label => {
                            Some(text.pos + text.galley.rect.center().to_vec2())
                        }
                        _ => None,
                    })
                    .expect("cancel action is visible even with an error at minimum size");
                assert!(button.y < 580.);
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.job.text == view.error.as_ref().unwrap().as_str())));
                frame(&mut view, vec![egui::Event::PointerMoved(button)]);
                for pressed in [true, false] {
                    frame(
                        &mut view,
                        vec![egui::Event::PointerButton {
                            pos: button,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                }
                assert!(cancel.is_cancelled());
                assert!(
                    view.busy,
                    "cancel waits for the worker's publication outcome"
                );
                assert!(
                    jobs.try_recv().is_err(),
                    "cancel does not queue behind the export"
                );
            }
        }
    }
}
