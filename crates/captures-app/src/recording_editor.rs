//! Host-independent recording editing over one immutable History source.
//!
//! Hosts serialize session calls on a worker. Preview frames are retained RGBA
//! buffers; media bytes, FFmpeg commands, and source-audio identity stay shared.

use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

use captures_history::{ArtifactKind, HistoryEntry};
use captures_media::{
    CancelToken, EditSpec, ExportFormat, ExportProgress, ExportSpec, MediaKind, MediaMetadata,
    MediaToolchain, ProbeResult, QualityPreset, validate_edit_spec,
};
use image::{ImageFormat, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::{
    Artifact,
    editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS},
};

const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FRAME_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingEditorOpenRequest {
    pub history_root: PathBuf,
    pub artifact_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingEditorRequest {
    Snapshot,
    UpdateEdit { edit: EditSpec },
    UpdatePreview { edit: EditSpec, export: ExportSpec },
    Seek { position_ms: u64 },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingSaveRequest {
    pub destination: PathBuf,
    pub export: ExportSpec,
}

#[derive(Debug, Serialize)]
pub struct RecordingEditorSnapshot<'a> {
    pub artifact_id: &'a str,
    pub source: &'a MediaMetadata,
    pub edit: &'a EditSpec,
    /// Accepted export configuration represented by the retained preview frame.
    /// Size-budget retries are not previewed; this always has no byte budget.
    pub preview_export: &'a ExportSpec,
    /// Source-relative position, independent of trim start.
    pub position_ms: u64,
    /// Increments only after an accepted edit or seek publishes its frame.
    pub revision: u64,
    pub has_system_audio: bool,
    pub has_microphone_audio: bool,
}

/// Save-new-copy either publishes a distinct History artifact or preserves the
/// already-published path with a recoverable post-publication warning.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SavedRecording {
    Saved {
        path: PathBuf,
        artifact: Box<Artifact>,
    },
    SavedWithoutHistory {
        path: PathBuf,
        warning: String,
    },
}

pub struct RecordingEditorSession {
    history_root: PathBuf,
    artifact_id: String,
    source_entry: HistoryEntry,
    source_path: PathBuf,
    tools: MediaToolchain,
    probe: ProbeResult,
    edit: EditSpec,
    preview_export: ExportSpec,
    position_ms: u64,
    revision: u64,
    has_system_audio: bool,
    has_microphone_audio: bool,
    frame: Arc<RgbaImage>,
    scratch: tempfile::TempDir,
}

impl RecordingEditorSession {
    pub fn open(
        request: RecordingEditorOpenRequest,
        tools: MediaToolchain,
    ) -> Result<Self, String> {
        let directory =
            captures_history::entry_directory(&request.history_root, &request.artifact_id)
                .map_err(|error| error.to_string())?;
        let metadata = read_bounded(
            &directory.join(captures_history::HISTORY_METADATA_FILE),
            MAX_METADATA_BYTES,
        )?;
        let entry: HistoryEntry =
            serde_json::from_slice(&metadata).map_err(|error| error.to_string())?;
        if entry.id != request.artifact_id || !entry.kind.is_recording() || entry.target.is_none() {
            return Err("Select a recording from History to edit.".into());
        }
        let source_path = entry
            .recording_media_path(&request.history_root)
            .filter(|path| path.is_file())
            .ok_or("The recording media is no longer available.")?;
        let probe = tools
            .probe(&source_path)
            .map_err(|error| error.to_string())?;
        validate_source(&entry, &probe)?;
        let (has_system_audio, has_microphone_audio) = trusted_audio(&entry, &probe);
        let mut edit = EditSpec::default();
        set_source_audio(&mut edit, has_system_audio, has_microphone_audio);
        validate_session_edit(&probe, &edit)?;
        let preview_export = default_preview_export();
        let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
        let frame = extract_preview(
            &tools,
            &source_path,
            &probe,
            &edit,
            &preview_export,
            0,
            scratch.path(),
        )?;
        Ok(Self {
            history_root: request.history_root,
            artifact_id: request.artifact_id,
            source_entry: entry,
            source_path,
            tools,
            probe,
            edit,
            preview_export,
            position_ms: 0,
            revision: 0,
            has_system_audio,
            has_microphone_audio,
            frame: Arc::new(frame),
            scratch,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> RecordingEditorSnapshot<'_> {
        RecordingEditorSnapshot {
            artifact_id: &self.artifact_id,
            source: &self.probe.metadata,
            edit: &self.edit,
            preview_export: &self.preview_export,
            position_ms: self.position_ms,
            revision: self.revision,
            has_system_audio: self.has_system_audio,
            has_microphone_audio: self.has_microphone_audio,
        }
    }

    #[must_use]
    pub fn frame(&self) -> Arc<RgbaImage> {
        self.frame.clone()
    }

    pub fn execute(&mut self, request: RecordingEditorRequest) -> Result<(), String> {
        match request {
            RecordingEditorRequest::Snapshot => Ok(()),
            RecordingEditorRequest::UpdateEdit { mut edit } => {
                set_source_audio(&mut edit, self.has_system_audio, self.has_microphone_audio);
                validate_session_edit(&self.probe, &edit)?;
                let frame = extract_preview(
                    &self.tools,
                    &self.source_path,
                    &self.probe,
                    &edit,
                    &self.preview_export,
                    self.position_ms,
                    self.scratch.path(),
                )?;
                self.edit = edit;
                self.frame = Arc::new(frame);
                self.revision = self.revision.saturating_add(1);
                Ok(())
            }
            RecordingEditorRequest::UpdatePreview { mut edit, export } => {
                set_source_audio(&mut edit, self.has_system_audio, self.has_microphone_audio);
                validate_session_edit(&self.probe, &edit)?;
                validate_preview_export(&export)?;
                let frame = extract_preview(
                    &self.tools,
                    &self.source_path,
                    &self.probe,
                    &edit,
                    &export,
                    self.position_ms,
                    self.scratch.path(),
                )?;
                self.edit = edit;
                self.preview_export = export;
                self.frame = Arc::new(frame);
                self.revision = self.revision.saturating_add(1);
                Ok(())
            }
            RecordingEditorRequest::Seek { position_ms } => {
                validate_position(&self.probe, position_ms)?;
                let frame = extract_preview(
                    &self.tools,
                    &self.source_path,
                    &self.probe,
                    &self.edit,
                    &self.preview_export,
                    position_ms,
                    self.scratch.path(),
                )?;
                self.position_ms = position_ms;
                self.frame = Arc::new(frame);
                self.revision = self.revision.saturating_add(1);
                Ok(())
            }
        }
    }

    pub fn save_new(
        &self,
        request: RecordingSaveRequest,
        cancel: &CancelToken,
        on_progress: impl FnMut(ExportProgress),
    ) -> Result<SavedRecording, String> {
        validate_destination(&request.destination, request.export.format)?;
        self.tools
            .export(
                &self.source_path,
                &request.destination,
                &self.edit,
                &request.export,
                cancel,
                on_progress,
            )
            .map_err(|error| error.to_string())?;

        match self.publish_history(&request, cancel) {
            Ok(artifact) => Ok(SavedRecording::Saved {
                path: request.destination,
                artifact: Box::new(artifact),
            }),
            Err(warning) => Ok(SavedRecording::SavedWithoutHistory {
                path: request.destination,
                warning,
            }),
        }
    }

    fn publish_history(
        &self,
        request: &RecordingSaveRequest,
        cancel: &CancelToken,
    ) -> Result<Artifact, String> {
        let probe = self
            .tools
            .probe(&request.destination)
            .map_err(|error| error.to_string())?;
        validate_output_probe(&probe)?;
        let poster_path = self
            .scratch
            .path()
            .join(format!("poster-{}.png", uuid::Uuid::new_v4()));
        let poster_result = self
            .tools
            .create_poster(&request.destination, &poster_path, cancel)
            .map_err(|error| error.to_string())
            .and_then(|()| read_bounded(&poster_path, MAX_FRAME_BYTES));
        let _ = fs::remove_file(&poster_path);
        let poster = poster_result?;
        let id = uuid::Uuid::new_v4().to_string();
        let kind = match request.export.format {
            ExportFormat::Gif => ArtifactKind::Gif,
            ExportFormat::Mp4 | ExportFormat::WebM => ArtifactKind::Video,
        };
        let (has_system_audio, has_microphone_audio) = output_audio(
            kind,
            &self.edit,
            self.has_system_audio,
            self.has_microphone_audio,
            probe.has_audio,
        );
        let entry = HistoryEntry {
            id: id.clone(),
            kind,
            preview_url: String::new(),
            full_url: String::new(),
            width: probe.metadata.width,
            height: probe.metadata.height,
            size_bytes: probe.metadata.size_bytes,
            created_at: chrono::Utc::now().to_rfc3339(),
            mode: None,
            saved_path: Some(request.destination.to_string_lossy().into_owned()),
            mime_type: Some(probe.metadata.mime_type.clone()),
            duration_ms: probe.metadata.duration_ms,
            target: self.source_entry.target.clone(),
            has_system_audio,
            has_microphone_audio,
            dropped_frames: self.source_entry.dropped_frames,
        };
        captures_history::save_recording(&self.history_root, &entry, &poster, &request.destination)
            .map_err(|error| error.to_string())?;
        let directory = captures_history::entry_directory(&self.history_root, &id)
            .map_err(|error| error.to_string())?;
        let preview_path = directory.join(captures_history::HISTORY_PREVIEW_FILE);
        Ok(Artifact {
            entry,
            image_path: preview_path.clone(),
            preview_path,
        })
    }
}

fn set_source_audio(edit: &mut EditSpec, system: bool, microphone: bool) {
    edit.audio.source_has_system_audio = system;
    edit.audio.source_has_microphone_audio = microphone;
}

fn trusted_audio(entry: &HistoryEntry, probe: &ProbeResult) -> (bool, bool) {
    if !probe.has_audio {
        return (false, false);
    }
    let microphone =
        entry.has_microphone_audio && (!entry.has_system_audio || probe.audio_stream_count >= 2);
    let system = entry.has_system_audio || !microphone;
    (system, microphone)
}

fn output_audio(
    kind: ArtifactKind,
    edit: &EditSpec,
    source_system: bool,
    source_microphone: bool,
    output_has_audio: bool,
) -> (bool, bool) {
    if kind == ArtifactKind::Gif || !output_has_audio {
        return (false, false);
    }
    match (
        source_system && !edit.audio.mute_system_audio,
        source_microphone && !edit.audio.mute_microphone,
    ) {
        (false, true) => (false, true),
        (true, _) => (true, false),
        (false, false) => (false, false),
    }
}

fn validate_source(entry: &HistoryEntry, probe: &ProbeResult) -> Result<(), String> {
    let expected = if entry.kind == ArtifactKind::Gif {
        MediaKind::Gif
    } else {
        MediaKind::Video
    };
    if probe.metadata.kind != expected {
        return Err("History recording kind does not match its retained media.".into());
    }
    validate_dimensions(probe.metadata.width, probe.metadata.height)?;
    if probe
        .metadata
        .duration_ms
        .is_none_or(|duration| duration == 0)
    {
        return Err("Recording duration is unavailable.".into());
    }
    Ok(())
}

fn validate_output_probe(probe: &ProbeResult) -> Result<(), String> {
    validate_dimensions(probe.metadata.width, probe.metadata.height)?;
    if probe
        .metadata
        .duration_ms
        .is_none_or(|duration| duration == 0)
    {
        return Err("Exported recording duration is unavailable.".into());
    }
    Ok(())
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_RENDER_DIMENSION
        || height > MAX_RENDER_DIMENSION
        || pixels > MAX_RENDER_PIXELS
    {
        return Err("Recording frames exceed the dimension or decoded-pixel limit.".into());
    }
    Ok(())
}

fn validate_session_edit(probe: &ProbeResult, edit: &EditSpec) -> Result<(), String> {
    validate_edit_spec(probe, edit).map_err(|error| error.to_string())?;
    let width = edit
        .output_width
        .or_else(|| edit.crop.map(|crop| crop.width))
        .unwrap_or(probe.metadata.width);
    let height = edit
        .output_height
        .or_else(|| edit.crop.map(|crop| crop.height))
        .unwrap_or(probe.metadata.height);
    validate_dimensions(width, height)
}

fn default_preview_export() -> ExportSpec {
    ExportSpec {
        format: ExportFormat::Mp4,
        quality: QualityPreset::Preserve,
        max_size_bytes: None,
        frames_per_second: None,
        gif_max_colors: None,
    }
}

fn validate_preview_export(export: &ExportSpec) -> Result<(), String> {
    if export.format == ExportFormat::WebM {
        return Err("WebM preview is unavailable because WebM export is not supported.".into());
    }
    if export.max_size_bytes.is_some() {
        return Err(
            "Size-budget previews are unavailable because the successful retry is not known before encoding."
                .into(),
        );
    }
    Ok(())
}

fn validate_position(probe: &ProbeResult, position_ms: u64) -> Result<(), String> {
    let duration = probe
        .metadata
        .duration_ms
        .ok_or("Recording duration is unavailable.")?;
    if position_ms >= duration {
        return Err("Seek position must stay within the source recording.".into());
    }
    Ok(())
}

fn extract_preview(
    tools: &MediaToolchain,
    source: &Path,
    probe: &ProbeResult,
    edit: &EditSpec,
    export: &ExportSpec,
    position_ms: u64,
    scratch: &Path,
) -> Result<RgbaImage, String> {
    validate_position(probe, position_ms)?;
    let path = scratch.join(format!("frame-{}.png", uuid::Uuid::new_v4()));
    let result = tools
        .extract_edited_frame(
            source,
            edit,
            export,
            position_ms,
            &path,
            &CancelToken::default(),
        )
        .map_err(|error| error.to_string())
        .and_then(|()| decode_frame(&path));
    let _ = fs::remove_file(path);
    result
}

fn decode_frame(path: &Path) -> Result<RgbaImage, String> {
    let bytes = read_bounded(path, MAX_FRAME_BYTES)?;
    let reader = || ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Png);
    let (width, height) = reader()
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    validate_dimensions(width, height)?;
    reader()
        .decode()
        .map_err(|error| error.to_string())
        .map(image::DynamicImage::into_rgba8)
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > maximum {
        return Err("Recording editor input exceeds its size limit.".into());
    }
    fs::read(path).map_err(|error| error.to_string())
}

fn validate_destination(destination: &Path, format: ExportFormat) -> Result<(), String> {
    if destination.as_os_str().is_empty() || destination.file_name().is_none() {
        return Err("Choose a file name for the edited recording.".into());
    }
    let expected = match format {
        ExportFormat::Mp4 => "mp4",
        ExportFormat::Gif => "gif",
        ExportFormat::WebM => "webm",
    };
    if !destination
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
    {
        return Err(format!(
            "The file extension must match the selected {expected} format."
        ));
    }
    if destination.exists() {
        return Err("Refusing to replace an existing recording file.".into());
    }
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or("Choose a destination folder for the edited recording.")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())
}
