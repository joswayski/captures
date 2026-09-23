//! Host-independent recording editing over one immutable History source.
//!
//! Hosts serialize session calls on a worker. Preview frames are retained RGBA
//! buffers; media bytes, FFmpeg commands, and source-audio identity stay shared.

use std::{
    fs,
    io::{Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use captures_history::{ArtifactKind, HistoryEntry};
use captures_media::{
    CancelToken, EditSpec, ExportEstimate, ExportFormat, ExportProgress, ExportSpec, MediaKind,
    MediaMetadata, MediaPlayback, MediaToolError, MediaToolchain, ProbeResult, QualityPreset,
    TimelineSpriteSpec, validate_edit_spec, validate_export_spec,
};
use image::{ImageFormat, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Artifact,
    editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS},
};

const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FRAME_BYTES: u64 = 128 * 1024 * 1024;
const TIMELINE_FRAME_COUNT: u32 = 12;
const TIMELINE_FRAME_WIDTH: u32 = 160;
const TIMELINE_FRAME_HEIGHT: u32 = 90;

/// Snapshot of the OS file ID without retaining an open Windows handle across
/// permanent/History renames. Content and change times are checked separately.
struct FileIdentity {
    metadata: fs::Metadata,
    id: file_id::FileId,
}

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

/// Additive request contract whose export is the accepted Save-new-copy
/// configuration. Its retained visual preview omits only the byte budget.
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingEditorRequestV2 {
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

/// The v1 snapshot plus the accepted Save-new-copy configuration. Flattening
/// preserves every existing v1 field and adds only `save_export`.
#[derive(Debug, Serialize)]
pub struct RecordingEditorSnapshotV2<'a> {
    #[serde(flatten)]
    pub editor: RecordingEditorSnapshot<'a>,
    pub save_export: &'a ExportSpec,
}

/// A retained full-source thumbnail strip independent of accepted editor state.
#[derive(Clone)]
pub struct RecordingTimelineThumbnails {
    pub frame_count: u32,
    pub frame_width: u32,
    pub frame_height: u32,
    pub sprite_width: u32,
    pub sprite_height: u32,
    pixels: Arc<RgbaImage>,
}

/// Encoded first-attempt comparison detached from the accepted session. The
/// identity is session-scoped: hosts must also guard item switches with their
/// own generation, not just revision and position.
pub struct RecordingExportComparison {
    pub revision: u64,
    pub position_ms: u64,
    pub after_seek_position_ms: u64,
    pub sample_start_ms: u64,
    pub sample_duration_ms: u64,
    pub export: ExportSpec,
    pub attempts: u8,
    before: Arc<RgbaImage>,
    after: Arc<RgbaImage>,
}

impl RecordingExportComparison {
    #[must_use]
    pub fn before_frame(&self) -> Arc<RgbaImage> {
        self.before.clone()
    }

    #[must_use]
    pub fn after_frame(&self) -> Arc<RgbaImage> {
        self.after.clone()
    }
}

impl RecordingTimelineThumbnails {
    /// Top-down straight-alpha sRGB RGBA8 pixels for the complete sprite.
    #[must_use]
    pub fn pixels(&self) -> &RgbaImage {
        &self.pixels
    }
}

/// A retained playback frame independent of the editor session and stream.
pub struct RecordingPlaybackFrame {
    pub position_ms: u64,
    pixels: Arc<RgbaImage>,
}

impl RecordingPlaybackFrame {
    /// Retain the top-down straight-alpha sRGB RGBA8 pixels.
    #[must_use]
    pub fn pixels(&self) -> Arc<RgbaImage> {
        self.pixels.clone()
    }
}

/// Clock-paced playback of the accepted edit and preview export.
pub struct RecordingPlayback {
    inner: MediaPlayback,
}

impl RecordingPlayback {
    #[must_use]
    pub fn width(&self) -> u32 {
        self.inner.width()
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.inner.height()
    }

    #[must_use]
    pub fn frames_per_second(&self) -> u16 {
        self.inner.frames_per_second()
    }

    #[must_use]
    pub fn start_position_ms(&self) -> u64 {
        self.inner.start_position_ms()
    }

    #[must_use]
    pub fn audio_enabled(&self) -> bool {
        self.inner.audio_enabled()
    }

    pub fn next_frame(&mut self) -> Result<Option<RecordingPlaybackFrame>, String> {
        let Some(frame) = self.inner.next_frame().map_err(|error| error.to_string())? else {
            return Ok(None);
        };
        let position_ms = frame.position_ms;
        let pixels = RgbaImage::from_raw(self.width(), self.height(), frame.into_pixels())
            .ok_or("Decoded playback pixels do not match the planned dimensions.")?;
        Ok(Some(RecordingPlaybackFrame {
            position_ms,
            pixels: Arc::new(pixels),
        }))
    }
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

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReplacedRecording {
    Replaced {
        path: PathBuf,
        artifact: Box<Artifact>,
    },
}

#[derive(Debug, Serialize)]
pub struct ReplaceOriginalError {
    pub message: String,
    pub requires_reopen: bool,
}

impl ReplaceOriginalError {
    fn unchanged(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            requires_reopen: false,
        }
    }

    fn indeterminate(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            requires_reopen: true,
        }
    }
}

impl std::fmt::Display for ReplaceOriginalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

enum SessionRequest {
    Snapshot,
    UpdateEdit {
        edit: EditSpec,
    },
    UpdatePreview {
        edit: EditSpec,
        preview_export: ExportSpec,
        save_export: ExportSpec,
    },
    Seek {
        position_ms: u64,
    },
}

pub struct RecordingEditorSession {
    history_root: PathBuf,
    artifact_id: String,
    source_entry: HistoryEntry,
    source_path: PathBuf,
    source_identity: FileIdentity,
    metadata_identity: FileIdentity,
    permanent_identity: Option<FileIdentity>,
    tools: MediaToolchain,
    probe: ProbeResult,
    edit: EditSpec,
    preview_export: ExportSpec,
    save_export: ExportSpec,
    position_ms: u64,
    revision: u64,
    has_system_audio: bool,
    has_microphone_audio: bool,
    frame: Arc<RgbaImage>,
    scratch: tempfile::TempDir,
    invalidated: bool,
}

impl RecordingEditorSession {
    pub fn open(
        request: RecordingEditorOpenRequest,
        tools: MediaToolchain,
    ) -> Result<Self, String> {
        let directory =
            captures_history::entry_directory(&request.history_root, &request.artifact_id)
                .map_err(|error| error.to_string())?;
        let metadata_path = directory.join(captures_history::HISTORY_METADATA_FILE);
        let metadata_identity = file_identity(&metadata_path)?;
        let metadata = read_bounded(&metadata_path, MAX_METADATA_BYTES)?;
        let entry: HistoryEntry =
            serde_json::from_slice(&metadata).map_err(|error| error.to_string())?;
        if entry.id != request.artifact_id || !entry.kind.is_recording() || entry.target.is_none() {
            return Err("Select a recording from History to edit.".into());
        }
        let source_path = entry
            .recording_media_path(&request.history_root)
            .filter(|path| path.is_file())
            .ok_or("The recording media is no longer available.")?;
        let source_identity = file_identity(&source_path)?;
        let probe = tools
            .probe(&source_path)
            .map_err(|error| error.to_string())?;
        validate_source(&entry, &probe)?;
        let (has_system_audio, has_microphone_audio) = trusted_audio(&entry, &probe);
        let mut edit = EditSpec::default();
        set_source_audio(&mut edit, has_system_audio, has_microphone_audio);
        validate_session_edit(&probe, &edit)?;
        let preview_export = default_preview_export();
        let save_export = preview_export.clone();
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
        if !unchanged_open_path(&source_identity, &source_path)? {
            return Err("Recording source changed while opening the editor.".into());
        }
        if !unchanged_open_path(&metadata_identity, &metadata_path)? {
            return Err("Recording History changed while opening the editor.".into());
        }
        let permanent_identity = entry
            .saved_path
            .as_ref()
            .and_then(|path| file_identity(Path::new(path)).ok());
        Ok(Self {
            history_root: request.history_root,
            artifact_id: request.artifact_id,
            source_entry: entry,
            source_path,
            source_identity,
            metadata_identity,
            permanent_identity,
            tools,
            probe,
            edit,
            preview_export,
            save_export,
            position_ms: 0,
            revision: 0,
            has_system_audio,
            has_microphone_audio,
            frame: Arc::new(frame),
            scratch,
            invalidated: false,
        })
    }

    fn ensure_active(&self) -> Result<(), String> {
        if self.invalidated {
            Err("Recording replacement is indeterminate; close and reopen this editor.".into())
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn requires_reopen(&self) -> bool {
        self.invalidated
    }

    /// The accepted session's permanent-save hint, without filesystem work or
    /// any replacement eligibility guarantee. Hosts use this exact path in
    /// confirmation UI instead of a potentially stale History-list artifact.
    #[must_use]
    pub fn original_save_path(&self) -> Option<&Path> {
        self.source_entry.saved_path.as_deref().map(Path::new)
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
    pub fn snapshot_v2(&self) -> RecordingEditorSnapshotV2<'_> {
        RecordingEditorSnapshotV2 {
            editor: self.snapshot(),
            save_export: &self.save_export,
        }
    }

    #[must_use]
    pub fn frame(&self) -> Arc<RgbaImage> {
        self.frame.clone()
    }

    /// Decode the immutable full source at the accepted source-relative
    /// position without applying trim, crop, output, or export effects.
    pub fn source_frame(&self, cancel: &CancelToken) -> Result<Arc<RgbaImage>, String> {
        self.ensure_active()?;
        extract_source_frame(
            &self.tools,
            &self.source_path,
            &self.probe,
            self.position_ms,
            self.scratch.path(),
            cancel,
        )
        .map(Arc::new)
    }

    /// Compare the accepted first-attempt preview against an encoded sample.
    /// A selected position outside the accepted trim is an error, not a
    /// silently clamped comparison of a different point in the recording.
    pub fn export_comparison(
        &self,
        cancel: &CancelToken,
    ) -> Result<RecordingExportComparison, String> {
        self.ensure_active()?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled.to_string());
        }
        validate_session_edit(&self.probe, &self.edit)?;
        validate_preview_export(&self.preview_export)?;
        validate_export_spec(&self.probe, &self.edit, &self.preview_export)
            .map_err(|error| error.to_string())?;
        let end = self.edit.trim_end_ms.unwrap_or(
            self.probe
                .metadata
                .duration_ms
                .ok_or("Recording duration is unavailable.")?,
        );
        if self.position_ms < self.edit.trim_start_ms || self.position_ms >= end {
            return Err("Selected position must stay within the accepted trim.".into());
        }
        let comparison = self
            .tools
            .compare_encoded_frame(
                &self.source_path,
                &self.probe,
                &self.edit,
                &self.preview_export,
                self.position_ms,
                self.scratch.path(),
                cancel,
            )
            .map_err(|error| error.to_string())?;
        let before = decode_png_bytes(&comparison.before_png)?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled.to_string());
        }
        let after = decode_png_bytes(&comparison.after_png)?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled.to_string());
        }
        // The already accepted frame is rendered by the same first-attempt
        // filter; retries in the generic capped media operation may instead
        // differ in dimensions and are deliberately not subjected to this.
        let expected = self.frame.dimensions();
        if before.dimensions() != expected || after.dimensions() != expected {
            return Err(
                "Encoded comparison dimensions do not match the accepted first attempt.".into(),
            );
        }
        Ok(RecordingExportComparison {
            revision: self.revision,
            position_ms: comparison.position_ms,
            after_seek_position_ms: comparison.after_seek_position_ms,
            sample_start_ms: comparison.sample_start_ms,
            sample_duration_ms: comparison.sample_duration_ms,
            export: self.preview_export.clone(),
            attempts: comparison.attempts,
            before: Arc::new(before),
            after: Arc::new(after),
        })
    }

    /// Estimate the accepted edit and preview export without changing session state.
    pub fn estimate_export(&self, cancel: &CancelToken) -> Result<ExportEstimate, String> {
        self.ensure_active()?;
        self.tools
            .estimate_export_size(&self.source_path, &self.edit, &self.preview_export, cancel)
            .map_err(|error| error.to_string())
    }

    /// Estimate the accepted Save-new-copy export without changing session state.
    pub fn estimate_save_export(&self, cancel: &CancelToken) -> Result<ExportEstimate, String> {
        self.ensure_active()?;
        self.tools
            .estimate_export_size(&self.source_path, &self.edit, &self.save_export, cancel)
            .map_err(|error| error.to_string())
    }

    /// Start silent playback from a source-relative position using the accepted
    /// edit and preview-export configuration without changing session state.
    pub fn playback(
        &self,
        position_ms: u64,
        cancel: &CancelToken,
    ) -> Result<RecordingPlayback, String> {
        self.ensure_active()?;
        validate_session_edit(&self.probe, &self.edit)?;
        validate_preview_export(&self.preview_export)?;
        self.tools
            .playback(
                &self.source_path,
                &self.probe,
                &self.edit,
                &self.preview_export,
                position_ms,
                cancel,
            )
            .map(|inner| RecordingPlayback { inner })
            .map_err(|error| error.to_string())
    }

    /// Start playback with accepted audio when it has an audible source. GIF,
    /// audio-less, muted, and zero-gain edits remain silent without opening an
    /// output device.
    pub fn playback_with_audio(
        &self,
        position_ms: u64,
        cancel: &CancelToken,
    ) -> Result<RecordingPlayback, String> {
        self.ensure_active()?;
        validate_session_edit(&self.probe, &self.edit)?;
        validate_preview_export(&self.preview_export)?;
        self.tools
            .playback_with_audio(
                &self.source_path,
                &self.probe,
                &self.edit,
                &self.preview_export,
                position_ms,
                cancel,
            )
            .map(|inner| RecordingPlayback { inner })
            .map_err(|error| error.to_string())
    }

    /// Generate the shipping full-source thumbnail strip without changing any
    /// accepted edit, preview, frame, position, revision, or History state.
    pub fn timeline_thumbnails(
        &self,
        cancel: &CancelToken,
    ) -> Result<RecordingTimelineThumbnails, String> {
        self.ensure_active()?;
        extract_timeline_thumbnails(
            &self.tools,
            &self.source_path,
            self.probe
                .metadata
                .duration_ms
                .ok_or("Recording duration is unavailable.")?,
            self.scratch.path(),
            cancel,
        )
    }

    pub fn execute(&mut self, request: RecordingEditorRequest) -> Result<(), String> {
        self.ensure_active()?;
        let request = match request {
            RecordingEditorRequest::Snapshot => SessionRequest::Snapshot,
            RecordingEditorRequest::UpdateEdit { edit } => SessionRequest::UpdateEdit { edit },
            RecordingEditorRequest::UpdatePreview { edit, export } => {
                validate_preview_export(&export)?;
                SessionRequest::UpdatePreview {
                    edit,
                    preview_export: export.clone(),
                    save_export: export,
                }
            }
            RecordingEditorRequest::Seek { position_ms } => SessionRequest::Seek { position_ms },
        };
        self.execute_request(request)
    }

    /// Execute the additive accepted-save-export contract. Existing v1 calls
    /// retain their budget rejection and snapshot shape.
    pub fn execute_v2(&mut self, request: RecordingEditorRequestV2) -> Result<(), String> {
        self.ensure_active()?;
        let request = match request {
            RecordingEditorRequestV2::Snapshot => SessionRequest::Snapshot,
            RecordingEditorRequestV2::UpdateEdit { edit } => SessionRequest::UpdateEdit { edit },
            RecordingEditorRequestV2::UpdatePreview { edit, export } => {
                validate_save_export_policy(&export)?;
                let mut preview_export = export.clone();
                preview_export.max_size_bytes = None;
                SessionRequest::UpdatePreview {
                    edit,
                    preview_export,
                    save_export: export,
                }
            }
            RecordingEditorRequestV2::Seek { position_ms } => SessionRequest::Seek { position_ms },
        };
        self.execute_request(request)
    }

    fn execute_request(&mut self, request: SessionRequest) -> Result<(), String> {
        match request {
            SessionRequest::Snapshot => Ok(()),
            SessionRequest::UpdateEdit { mut edit } => {
                set_source_audio(&mut edit, self.has_system_audio, self.has_microphone_audio);
                validate_session_edit(&self.probe, &edit)?;
                validate_export_spec(&self.probe, &edit, &self.save_export)
                    .map_err(|error| error.to_string())?;
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
            SessionRequest::UpdatePreview {
                mut edit,
                preview_export,
                save_export,
            } => {
                set_source_audio(&mut edit, self.has_system_audio, self.has_microphone_audio);
                validate_session_edit(&self.probe, &edit)?;
                validate_export_spec(&self.probe, &edit, &save_export)
                    .map_err(|error| error.to_string())?;
                let frame = extract_preview(
                    &self.tools,
                    &self.source_path,
                    &self.probe,
                    &edit,
                    &preview_export,
                    self.position_ms,
                    self.scratch.path(),
                )?;
                self.edit = edit;
                self.preview_export = preview_export;
                self.save_export = save_export;
                self.frame = Arc::new(frame);
                self.revision = self.revision.saturating_add(1);
                Ok(())
            }
            SessionRequest::Seek { position_ms } => {
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
        self.ensure_active()?;
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

    /// Replace only an existing permanent recording and its byte-identical
    /// private recovery copy. No cancellation-only exit occurs after the
    /// permanent rename: History is published or the permanent path is
    /// compensated from the intact recovery source. This is not crash-atomic
    /// across the two directories.
    pub fn replace_original(
        &mut self,
        cancel: &CancelToken,
        on_progress: impl FnMut(ExportProgress),
    ) -> Result<ReplacedRecording, ReplaceOriginalError> {
        self.replace_original_with(
            cancel,
            on_progress,
            |root, entry, poster, path| {
                captures_history::save_recording(root, entry, poster, path)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
            |recovery, rollback| fs::copy(recovery, rollback).map(|_| ()),
        )
    }

    fn replace_original_with(
        &mut self,
        cancel: &CancelToken,
        on_progress: impl FnMut(ExportProgress),
        publish: impl FnOnce(&Path, &HistoryEntry, &[u8], &Path) -> Result<(), String>,
        copy_for_rollback: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<ReplacedRecording, ReplaceOriginalError> {
        let unchanged = ReplaceOriginalError::unchanged;
        if self.invalidated {
            return Err(ReplaceOriginalError::indeterminate(
                "Recording replacement is indeterminate; close and reopen this editor.",
            ));
        }
        if cancel.is_cancelled() {
            return Err(unchanged(MediaToolError::Cancelled.to_string()));
        }
        let extension = match (self.source_entry.kind, self.save_export.format) {
            (ArtifactKind::Video, ExportFormat::Mp4) => "mp4",
            (ArtifactKind::Gif, ExportFormat::Gif) => "gif",
            _ => {
                return Err(unchanged(
                    "Replace original requires the source's MP4 or GIF format.".into(),
                ));
            }
        };
        let directory = captures_history::entry_directory(&self.history_root, &self.artifact_id)
            .map_err(|error| unchanged(error.to_string()))?;
        let recovery = directory.join(format!("media.{extension}"));
        if self.source_path != recovery {
            return Err(unchanged(
                "Replace original requires an existing private recovery copy.".into(),
            ));
        }
        if !unchanged_file_at_path(&self.source_identity, &recovery).map_err(unchanged)? {
            return Err(unchanged(
                "The accepted recording source changed since opening the editor.".into(),
            ));
        }
        let permanent = self
            .source_entry
            .saved_path
            .as_ref()
            .map(PathBuf::from)
            .ok_or_else(|| {
                unchanged("Replace original requires an existing permanent save.".into())
            })?;
        let accepted_permanent = self.permanent_identity.as_ref().ok_or_else(|| {
            unchanged("The permanent save was not present when this editor opened.".into())
        })?;
        if !unchanged_file_at_path(accepted_permanent, &permanent).map_err(unchanged)? {
            return Err(unchanged(
                "The permanent save changed since opening the editor.".into(),
            ));
        }
        if !permanent
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            return Err(unchanged(
                "Permanent recording extension does not match its source.".into(),
            ));
        }
        let history_root =
            fs::canonicalize(&self.history_root).map_err(|error| unchanged(error.to_string()))?;
        let permanent_canonical =
            fs::canonicalize(&permanent).map_err(|error| unchanged(error.to_string()))?;
        if permanent_canonical.starts_with(&history_root)
            || permanent_canonical
                == fs::canonicalize(&recovery).map_err(|error| unchanged(error.to_string()))?
        {
            return Err(unchanged(
                "The permanent save must be outside private History.".into(),
            ));
        }
        let mut original_recovery = regular_file(&recovery).map_err(unchanged)?;
        let original_permanent = regular_file(&permanent).map_err(unchanged)?;
        if !matches_original(&mut original_recovery, &permanent).map_err(unchanged)? {
            return Err(unchanged(
                "The permanent save differs from its History recovery copy.".into(),
            ));
        }
        let recovery_identity = file_identity(&recovery).map_err(unchanged)?;
        let permanent_identity = file_identity(&permanent).map_err(unchanged)?;
        let old_digest = file_digest(&recovery).map_err(unchanged)?;
        let metadata_path = directory.join(captures_history::HISTORY_METADATA_FILE);
        if !unchanged_file_at_path(&self.metadata_identity, &metadata_path).map_err(unchanged)? {
            return Err(unchanged(
                "History metadata changed since opening the editor.".into(),
            ));
        }
        let original_metadata =
            read_bounded(&metadata_path, MAX_METADATA_BYTES).map_err(unchanged)?;
        if serde_json::to_value(
            serde_json::from_slice::<HistoryEntry>(&original_metadata)
                .map_err(|error| unchanged(error.to_string()))?,
        )
        .map_err(|error| unchanged(error.to_string()))?
            != serde_json::to_value(&self.source_entry)
                .map_err(|error| unchanged(error.to_string()))?
        {
            return Err(unchanged(
                "History metadata no longer matches the opened recording.".into(),
            ));
        }
        validate_session_edit(&self.probe, &self.edit).map_err(unchanged)?;
        validate_save_export_policy(&self.save_export).map_err(unchanged)?;
        validate_export_spec(&self.probe, &self.edit, &self.save_export)
            .map_err(|error| unchanged(error.to_string()))?;

        let parent = permanent
            .parent()
            .ok_or_else(|| unchanged("Permanent save folder is unavailable.".into()))?;
        let stage_dir = tempfile::Builder::new()
            .prefix(".captures-replace-")
            .tempdir_in(parent)
            .map_err(|error| unchanged(error.to_string()))?;
        let stage = stage_dir.path().join(format!("staged.{extension}"));
        self.tools
            .export(
                &recovery,
                &stage,
                &self.edit,
                &self.save_export,
                cancel,
                on_progress,
            )
            .map_err(|error| unchanged(error.to_string()))?;
        if cancel.is_cancelled() {
            return Err(unchanged(MediaToolError::Cancelled.to_string()));
        }
        let new_probe = self
            .tools
            .probe(&stage)
            .map_err(|error| unchanged(error.to_string()))?;
        validate_source(&self.source_entry, &new_probe).map_err(unchanged)?;
        if !new_probe
            .metadata
            .mime_type
            .eq_ignore_ascii_case(if extension == "mp4" {
                "video/mp4"
            } else {
                "image/gif"
            })
        {
            return Err(unchanged(
                "Exported recording format does not match its source.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(unchanged(MediaToolError::Cancelled.to_string()));
        }
        let poster_path = stage_dir.path().join("poster.png");
        self.tools
            .create_poster(&stage, &poster_path, cancel)
            .map_err(|error| unchanged(error.to_string()))?;
        let poster = read_bounded(&poster_path, MAX_FRAME_BYTES).map_err(unchanged)?;
        if cancel.is_cancelled() {
            return Err(unchanged(MediaToolError::Cancelled.to_string()));
        }
        let (new_system, new_microphone) = output_audio(
            self.source_entry.kind,
            &self.edit,
            self.has_system_audio,
            self.has_microphone_audio,
            &new_probe,
        );
        let mut new_edit = EditSpec::default();
        set_source_audio(&mut new_edit, new_system, new_microphone);
        let new_export = ExportSpec {
            format: self.save_export.format,
            ..default_preview_export()
        };
        let new_frame = extract_preview(
            &self.tools,
            &stage,
            &new_probe,
            &new_edit,
            &new_export,
            0,
            self.scratch.path(),
        )
        .map_err(unchanged)?;
        if cancel.is_cancelled() {
            return Err(unchanged(MediaToolError::Cancelled.to_string()));
        }
        let mut new_entry = self.source_entry.clone();
        new_entry.width = new_probe.metadata.width;
        new_entry.height = new_probe.metadata.height;
        new_entry.size_bytes = new_probe.metadata.size_bytes;
        new_entry.duration_ms = new_probe.metadata.duration_ms;
        new_entry.mime_type = Some(new_probe.metadata.mime_type.clone());
        new_entry.has_system_audio = new_system;
        new_entry.has_microphone_audio = new_microphone;
        fs::File::open(&stage)
            .and_then(|file| file.sync_all())
            .map_err(|error| unchanged(error.to_string()))?;
        if cancel.is_cancelled() {
            return Err(unchanged(MediaToolError::Cancelled.to_string()));
        }
        if !unchanged_file_at_path(&self.metadata_identity, &metadata_path).map_err(unchanged)?
            || fs::read(&metadata_path).map_err(|error| unchanged(error.to_string()))?
                != original_metadata
            || !same_file_at_path(&recovery_identity, &recovery).map_err(unchanged)?
            || !same_file_at_path(&permanent_identity, &permanent).map_err(unchanged)?
            || !unchanged_file_at_path(accepted_permanent, &permanent).map_err(unchanged)?
            || !unchanged_file_at_path(&self.source_identity, &recovery).map_err(unchanged)?
            || file_digest(&recovery).map_err(unchanged)? != old_digest
            || file_digest(&permanent).map_err(unchanged)? != old_digest
        {
            return Err(unchanged(
                "Recording source or History changed while preparing replacement.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(unchanged(MediaToolError::Cancelled.to_string()));
        }
        // Windows cannot replace an open destination or rename an open History
        // directory. Keep metadata identity + digest, not live handles, across
        // publication and compensation.
        drop(original_recovery);
        drop(original_permanent);

        // A panic or failed compensation leaves this guard set. Existing
        // infallible snapshot/frame accessors retain old data, but no media
        // operation can use it until the session is reopened.
        self.invalidated = true;
        if let Err(error) = fs::rename(&stage, &permanent) {
            if same_file_at_path(&permanent_identity, &permanent).unwrap_or(false)
                && file_digest(&permanent).is_ok_and(|digest| digest == old_digest)
            {
                self.invalidated = false;
                return Err(unchanged(error.to_string()));
            }
            return Err(ReplaceOriginalError::indeterminate(format!(
                "Permanent replacement failed and its old bytes cannot be verified: {error}"
            )));
        }
        if let Err(error) = publish(&self.history_root, &new_entry, &poster, &permanent) {
            let history_intact = unchanged_file_at_path(&self.metadata_identity, &metadata_path)
                .unwrap_or(false)
                && fs::read(&metadata_path).is_ok_and(|bytes| bytes == original_metadata)
                && same_file_at_path(&recovery_identity, &recovery).unwrap_or(false)
                && file_digest(&recovery).is_ok_and(|digest| digest == old_digest);
            if history_intact {
                let rollback = stage_dir.path().join(format!("rollback.{extension}"));
                let restored = copy_for_rollback(&recovery, &rollback)
                    .and_then(|_| fs::File::open(&rollback)?.sync_all())
                    .and_then(|_| fs::rename(&rollback, &permanent));
                if restored.is_ok()
                    && file_digest(&permanent).is_ok_and(|digest| digest == old_digest)
                {
                    self.permanent_identity = Some(file_identity(&permanent).map_err(|stat_error| {
                        ReplaceOriginalError::indeterminate(format!(
                            "Permanent bytes were restored but their identity could not be verified: {stat_error}"
                        ))
                    })?);
                    self.invalidated = false;
                    return Err(unchanged(error.to_string()));
                }
            }
            return Err(ReplaceOriginalError::indeterminate(format!(
                "History replacement failed and the permanent save could not safely be restored: {error}"
            )));
        }
        let published_identity = file_identity(&recovery).map_err(|error| {
            ReplaceOriginalError::indeterminate(format!(
                "Recording published, but its recovery source cannot be verified: {error}"
            ))
        })?;
        let published_metadata = file_identity(&metadata_path)
            .map_err(|error| ReplaceOriginalError::indeterminate(error.to_string()))?;
        let published_permanent = file_identity(&permanent)
            .map_err(|error| ReplaceOriginalError::indeterminate(error.to_string()))?;
        self.source_entry = new_entry.clone();
        self.source_path = recovery;
        self.source_identity = published_identity;
        self.metadata_identity = published_metadata;
        self.permanent_identity = Some(published_permanent);
        self.probe = new_probe;
        self.has_system_audio = new_system;
        self.has_microphone_audio = new_microphone;
        self.edit = new_edit;
        self.preview_export = new_export.clone();
        self.save_export = new_export;
        self.position_ms = 0;
        self.frame = Arc::new(new_frame);
        self.revision = self.revision.saturating_add(1);
        self.invalidated = false;
        let preview_path = directory.join(captures_history::HISTORY_PREVIEW_FILE);
        Ok(ReplacedRecording::Replaced {
            path: permanent,
            artifact: Box::new(Artifact {
                entry: new_entry,
                image_path: preview_path.clone(),
                preview_path,
            }),
        })
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
            &probe,
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
    output: &ProbeResult,
) -> (bool, bool) {
    if kind == ArtifactKind::Gif || !output.has_audio {
        return (false, false);
    }
    let system = source_system && !edit.audio.mute_system_audio;
    let microphone = source_microphone && !edit.audio.mute_microphone;
    match (system, microphone) {
        (true, true) if output.audio_stream_count >= 2 => (true, true),
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

fn validate_save_export_policy(export: &ExportSpec) -> Result<(), String> {
    if export.format == ExportFormat::WebM {
        return Err("WebM export is not supported.".into());
    }
    if let Some(maximum) = export.max_size_bytes {
        if maximum < 100_000 {
            return Err("Maximum file size must be at least 100000 bytes.".into());
        }
        if export.quality != QualityPreset::Preserve {
            return Err("Maximum file size requires Preserve quality.".into());
        }
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

fn extract_source_frame(
    tools: &MediaToolchain,
    source: &Path,
    probe: &ProbeResult,
    position_ms: u64,
    scratch: &Path,
    cancel: &CancelToken,
) -> Result<RgbaImage, String> {
    validate_position(probe, position_ms)?;
    let path = scratch.join(format!("source-frame-{}.png", uuid::Uuid::new_v4()));
    let result = tools
        .extract_frame(source, position_ms, &path, cancel)
        .map_err(|error| error.to_string())
        .and_then(|()| decode_frame(&path))
        .and_then(|frame| {
            if frame.dimensions() == (probe.metadata.width, probe.metadata.height) {
                Ok(frame)
            } else {
                Err("Decoded source frame dimensions do not match the recording metadata.".into())
            }
        });
    let _ = fs::remove_file(path);
    result
}

fn extract_timeline_thumbnails(
    tools: &MediaToolchain,
    source: &Path,
    duration_ms: u64,
    scratch: &Path,
    cancel: &CancelToken,
) -> Result<RecordingTimelineThumbnails, String> {
    let path = scratch.join(format!("timeline-{}.png", uuid::Uuid::new_v4()));
    let result = (|| {
        tools
            .create_timeline_sprite(
                source,
                &path,
                TimelineSpriteSpec {
                    duration_ms,
                    frame_count: TIMELINE_FRAME_COUNT as u16,
                    frame_width: TIMELINE_FRAME_WIDTH,
                    frame_height: TIMELINE_FRAME_HEIGHT,
                },
                cancel,
            )
            .map_err(|error| error.to_string())?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled.to_string());
        }
        let pixels = decode_frame(&path)?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled.to_string());
        }
        let sprite_width = TIMELINE_FRAME_WIDTH * TIMELINE_FRAME_COUNT;
        if pixels.dimensions() != (sprite_width, TIMELINE_FRAME_HEIGHT) {
            return Err("Timeline thumbnail dimensions do not match the shared media plan.".into());
        }
        Ok(RecordingTimelineThumbnails {
            frame_count: TIMELINE_FRAME_COUNT,
            frame_width: TIMELINE_FRAME_WIDTH,
            frame_height: TIMELINE_FRAME_HEIGHT,
            sprite_width,
            sprite_height: TIMELINE_FRAME_HEIGHT,
            pixels: Arc::new(pixels),
        })
    })();
    let _ = fs::remove_file(path);
    result
}

fn decode_frame(path: &Path) -> Result<RgbaImage, String> {
    let bytes = read_bounded(path, MAX_FRAME_BYTES)?;
    decode_png_bytes(&bytes)
}

fn decode_png_bytes(bytes: &[u8]) -> Result<RgbaImage, String> {
    if bytes.len() as u64 > MAX_FRAME_BYTES {
        return Err("Recording editor input exceeds its size limit.".into());
    }
    let reader = || ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
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

fn regular_file(path: &Path) -> Result<fs::File, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("Replace original requires a regular, non-symlink recording file.".into());
    }
    fs::File::open(path).map_err(|error| error.to_string())
}

fn file_identity(path: &Path) -> Result<FileIdentity, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let id = file_id::get_file_id(path).map_err(|error| error.to_string())?;
    Ok(FileIdentity { metadata, id })
}

fn same_file_at_path(old: &FileIdentity, path: &Path) -> Result<bool, String> {
    // In replacement only regular non-symlink files are eligible. Open the
    // file ID afresh, and drop its internal Windows handle before renaming.
    let current = regular_file(path)?;
    let new = current.metadata().map_err(|error| error.to_string())?;
    drop(current);
    if old.id != file_id::get_file_id(path).map_err(|error| error.to_string())? {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(old.metadata.dev() == new.dev() && old.metadata.ino() == new.ino())
    }
    #[cfg(not(unix))]
    {
        Ok(old.metadata.len() == new.len())
    }
}

fn unchanged_file_at_path(old: &FileIdentity, path: &Path) -> Result<bool, String> {
    if !same_file_at_path(old, path)? {
        return Ok(false);
    }
    unchanged_open_path(old, path)
}

fn unchanged_open_path(old: &FileIdentity, path: &Path) -> Result<bool, String> {
    let current = fs::metadata(path).map_err(|error| error.to_string())?;
    let current_identity = file_identity(path)?;
    if old.id != current_identity.id {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(old.metadata.dev() == current.dev()
            && old.metadata.ino() == current.ino()
            && old.metadata.len() == current.len()
            && old.metadata.mtime() == current.mtime()
            && old.metadata.mtime_nsec() == current.mtime_nsec()
            && old.metadata.ctime() == current.ctime()
            && old.metadata.ctime_nsec() == current.ctime_nsec())
    }
    #[cfg(not(unix))]
    {
        Ok(old.metadata.len() == current.len()
            && old.metadata.modified().ok() == current.modified().ok())
    }
}

fn file_digest(path: &Path) -> Result<[u8; 32], String> {
    let mut file = regular_file(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}

fn matches_original(original: &mut fs::File, path: &Path) -> Result<bool, String> {
    let mut current = regular_file(path)?;
    if original
        .metadata()
        .map_err(|error| error.to_string())?
        .len()
        != current.metadata().map_err(|error| error.to_string())?.len()
    {
        return Ok(false);
    }
    original
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut old = [0_u8; 64 * 1024];
    let mut new = [0_u8; 64 * 1024];
    loop {
        let count = original.read(&mut old).map_err(|error| error.to_string())?;
        current
            .read_exact(&mut new[..count])
            .map_err(|error| error.to_string())?;
        if old[..count] != new[..count] {
            return Ok(false);
        }
        if count == 0 {
            return Ok(true);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use captures_recording::RecordingTarget;
    use std::process::Command;

    fn real_tools() -> Option<(MediaToolchain, PathBuf)> {
        let ffmpeg = std::env::var_os("CAPTURES_TEST_FFMPEG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ffmpeg"));
        let ffprobe = std::env::var_os("CAPTURES_TEST_FFPROBE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ffprobe"));
        let tools = MediaToolchain::new(ffmpeg.clone(), ffprobe);
        match tools.verify() {
            Ok(()) => Some((tools, ffmpeg)),
            Err(error) => {
                eprintln!("timeline scratch test skipped: {error}");
                None
            }
        }
    }

    fn create_video(ffmpeg: &Path, path: &Path, duration: &str) {
        let status = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size=640x360:rate=30:duration={duration}"),
                "-c:v",
                "mpeg4",
                "-q:v",
                "2",
                "-an",
            ])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn open_session(
        tools: MediaToolchain,
        source: &Path,
        history_root: &Path,
    ) -> RecordingEditorSession {
        let probe = tools.probe(source).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let entry = HistoryEntry {
            id: id.clone(),
            kind: ArtifactKind::Video,
            preview_url: String::new(),
            full_url: String::new(),
            width: probe.metadata.width,
            height: probe.metadata.height,
            size_bytes: probe.metadata.size_bytes,
            created_at: "2026-09-23T00:00:00Z".into(),
            mode: None,
            saved_path: Some(source.to_string_lossy().into_owned()),
            mime_type: Some(probe.metadata.mime_type),
            duration_ms: probe.metadata.duration_ms,
            target: Some(RecordingTarget::Display {
                display_id: "test-display".into(),
            }),
            has_system_audio: false,
            has_microphone_audio: false,
            dropped_frames: 0,
        };
        captures_history::save_recording(history_root, &entry, b"poster", source).unwrap();
        RecordingEditorSession::open(
            RecordingEditorOpenRequest {
                history_root: history_root.to_path_buf(),
                artifact_id: id,
            },
            tools,
        )
        .unwrap()
    }

    #[test]
    fn history_failure_compensates_permanent_from_intact_recovery() {
        let Some((tools, ffmpeg)) = real_tools() else {
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let permanent = data.path().join("original.mp4");
        let history = data.path().join("history");
        create_video(&ffmpeg, &permanent, "1.5");
        let mut session = open_session(tools, &permanent, &history);
        let old = fs::read(&permanent).unwrap();
        let metadata = fs::read(history.join(&session.artifact_id).join("metadata.json")).unwrap();
        let accepted = serde_json::to_value(session.snapshot_v2()).unwrap();
        let frame = session.frame();
        let error = session
            .replace_original_with(
                &CancelToken::default(),
                |_| {},
                |_, _, _, _| Err("injected History failure".into()),
                |source, rollback| fs::copy(source, rollback).map(|_| ()),
            )
            .unwrap_err();
        assert_eq!(error.message, "injected History failure");
        assert!(!error.requires_reopen);
        assert!(!session.requires_reopen());
        assert_eq!(fs::read(&permanent).unwrap(), old);
        assert_eq!(
            fs::read(history.join(&session.artifact_id).join("media.mp4")).unwrap(),
            old
        );
        assert_eq!(
            fs::read(history.join(&session.artifact_id).join("metadata.json")).unwrap(),
            metadata
        );
        assert_eq!(
            serde_json::to_value(session.snapshot_v2()).unwrap(),
            accepted
        );
        assert!(Arc::ptr_eq(&frame, &session.frame()));
        session
            .execute(RecordingEditorRequest::Seek { position_ms: 300 })
            .unwrap();
        session
            .replace_original(&CancelToken::default(), |_| {})
            .unwrap();
        assert!(!session.requires_reopen());
        assert_eq!(session.snapshot().revision, 2);
        assert_eq!(session.snapshot().position_ms, 0);
        assert_eq!(
            fs::read(&permanent).unwrap(),
            fs::read(history.join(&session.artifact_id).join("media.mp4")).unwrap()
        );
    }

    #[test]
    fn failed_compensation_or_post_commit_panic_requires_reopen() {
        let Some((tools, ffmpeg)) = real_tools() else {
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let permanent = data.path().join("original.mp4");
        let history = data.path().join("history");
        create_video(&ffmpeg, &permanent, "1.5");
        let mut session = open_session(tools.clone(), &permanent, &history);
        let error = session
            .replace_original_with(
                &CancelToken::default(),
                |_| {},
                |_, _, _, _| Err("injected History failure".into()),
                |_, _| Err(std::io::Error::other("injected rollback failure")),
            )
            .unwrap_err();
        assert!(error.requires_reopen);
        assert!(session.requires_reopen());
        assert!(session.execute(RecordingEditorRequest::Snapshot).is_err());
        assert!(session.source_frame(&CancelToken::default()).is_err());
        assert!(session.playback(0, &CancelToken::default()).is_err());
        assert!(
            session
                .save_new(
                    RecordingSaveRequest {
                        destination: data.path().join("copy.mp4"),
                        export: default_preview_export(),
                    },
                    &CancelToken::default(),
                    |_| {}
                )
                .is_err()
        );
        assert!(
            session
                .replace_original(&CancelToken::default(), |_| {})
                .unwrap_err()
                .requires_reopen
        );

        // A distinct fixture isolates the panic case from the previous commit.
        let second = data.path().join("other.mp4");
        create_video(&ffmpeg, &second, "1.5");
        let mut session = open_session(tools, &second, &history);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = session.replace_original_with(
                &CancelToken::default(),
                |_| {},
                |_, _, _, _| panic!("injected post-publication panic"),
                |source, rollback| fs::copy(source, rollback).map(|_| ()),
            );
        }));
        assert!(result.is_err());
        assert!(session.requires_reopen());
        assert!(session.execute(RecordingEditorRequest::Snapshot).is_err());
    }

    fn maximum_gif() -> ExportSpec {
        ExportSpec {
            format: ExportFormat::Gif,
            quality: QualityPreset::Preserve,
            max_size_bytes: Some(100_000),
            frames_per_second: Some(30),
            gif_max_colors: Some(256),
        }
    }

    fn maximum_mp4() -> ExportSpec {
        ExportSpec {
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            max_size_bytes: Some(100_000),
            frames_per_second: Some(30),
            gif_max_colors: None,
        }
    }

    #[test]
    fn read_only_frame_scratch_is_clean_after_success_failure_and_cancellation() {
        let Some((tools, ffmpeg)) = real_tools() else {
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("source.mp4");
        let status = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:size=32x24:rate=10:duration=1",
                "-c:v",
                "mpeg4",
                "-an",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let scratch = tempfile::tempdir().unwrap();
        let is_clean = || fs::read_dir(scratch.path()).unwrap().next().is_none();
        let probe = tools.probe(&source).unwrap();

        extract_source_frame(
            &tools,
            &source,
            &probe,
            0,
            scratch.path(),
            &CancelToken::default(),
        )
        .unwrap();
        assert!(is_clean());

        let source_cancel = CancelToken::default();
        source_cancel.cancel();
        assert!(
            extract_source_frame(&tools, &source, &probe, 0, scratch.path(), &source_cancel)
                .is_err()
        );
        assert!(is_clean());

        assert!(
            extract_source_frame(
                &tools,
                &data.path().join("missing.mp4"),
                &probe,
                0,
                scratch.path(),
                &CancelToken::default(),
            )
            .is_err()
        );
        assert!(is_clean());

        extract_timeline_thumbnails(
            &tools,
            &source,
            1_000,
            scratch.path(),
            &CancelToken::default(),
        )
        .unwrap();
        assert!(is_clean());

        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(
            extract_timeline_thumbnails(&tools, &source, 1_000, scratch.path(), &cancel).is_err()
        );
        assert!(is_clean());

        assert!(
            extract_timeline_thumbnails(
                &tools,
                &data.path().join("missing.mp4"),
                1_000,
                scratch.path(),
                &CancelToken::default(),
            )
            .is_err()
        );
        assert!(is_clean());
    }

    #[test]
    fn v2_budget_preview_is_atomic_and_retry_output_can_change_dimensions() {
        let Some((tools, ffmpeg)) = real_tools() else {
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("source.mp4");
        create_video(&ffmpeg, &source, "0.8");
        let history_root = data.path().join("history");
        let mut session = open_session(tools.clone(), &source, &history_root);
        let source_bytes = fs::read(&session.source_path).unwrap();
        let original_frame = session.frame();
        let original = serde_json::to_value(session.snapshot_v2()).unwrap();

        for export in [
            ExportSpec {
                max_size_bytes: Some(99_999),
                ..maximum_gif()
            },
            ExportSpec {
                quality: QualityPreset::Standard,
                ..maximum_gif()
            },
            ExportSpec {
                format: ExportFormat::WebM,
                ..maximum_gif()
            },
        ] {
            assert!(
                session
                    .execute_v2(RecordingEditorRequestV2::UpdatePreview {
                        edit: EditSpec::default(),
                        export,
                    })
                    .is_err()
            );
            assert_eq!(
                serde_json::to_value(session.snapshot_v2()).unwrap(),
                original
            );
            assert!(Arc::ptr_eq(&session.frame(), &original_frame));
        }

        session
            .execute_v2(RecordingEditorRequestV2::UpdatePreview {
                edit: EditSpec::default(),
                export: maximum_gif(),
            })
            .unwrap();
        let accepted = session.snapshot_v2();
        assert_eq!(accepted.save_export.max_size_bytes, Some(100_000));
        assert_eq!(accepted.editor.preview_export.max_size_bytes, None);
        assert_eq!(session.frame().dimensions(), (640, 360));
        let preview_estimate = session.estimate_export(&CancelToken::default()).unwrap();
        let save_estimate = session
            .estimate_save_export(&CancelToken::default())
            .unwrap();
        assert!(preview_estimate.exact && save_estimate.exact);
        assert!(preview_estimate.size_bytes > 100_000);
        assert!(save_estimate.size_bytes <= 100_000);

        let destination = data.path().join("retry.gif");
        let saved = session
            .save_new(
                RecordingSaveRequest {
                    destination: destination.clone(),
                    export: accepted.save_export.clone(),
                },
                &CancelToken::default(),
                |_| {},
            )
            .unwrap();
        let SavedRecording::Saved { artifact, .. } = saved else {
            panic!("History publication must succeed")
        };
        assert!(fs::metadata(&destination).unwrap().len() <= 100_000);
        assert_eq!((artifact.entry.width, artifact.entry.height), (320, 180));
        assert_eq!(session.frame().dimensions(), (640, 360));
        assert_eq!(session.snapshot_v2().editor.revision, 1);
        assert_eq!(fs::read(&session.source_path).unwrap(), source_bytes);
    }

    #[test]
    fn v2_mp4_budget_revalidates_edits_and_v1_resets_only_through_preview() {
        let Some((tools, ffmpeg)) = real_tools() else {
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("source.mp4");
        create_video(&ffmpeg, &source, "4");
        let history_root = data.path().join("history");
        let mut session = open_session(tools, &source, &history_root);
        let source_bytes = fs::read(&session.source_path).unwrap();
        let initial_frame = session.frame();
        let mut short_edit = EditSpec {
            trim_end_ms: Some(2_500),
            ..EditSpec::default()
        };

        let uncapped_mp4 = ExportSpec {
            max_size_bytes: None,
            ..maximum_mp4()
        };
        session
            .execute(RecordingEditorRequest::UpdatePreview {
                edit: short_edit.clone(),
                export: uncapped_mp4,
            })
            .unwrap();
        let before_budget_frame = session.frame();

        session
            .execute_v2(RecordingEditorRequestV2::UpdatePreview {
                edit: short_edit.clone(),
                export: maximum_mp4(),
            })
            .unwrap();
        let accepted = session.snapshot_v2();
        assert_eq!(accepted.editor.revision, 2);
        assert_eq!(accepted.editor.edit, &short_edit);
        assert_eq!(accepted.editor.preview_export.max_size_bytes, None);
        assert_eq!(accepted.save_export.max_size_bytes, Some(100_000));
        assert!(!Arc::ptr_eq(&session.frame(), &initial_frame));
        assert!(!Arc::ptr_eq(&session.frame(), &before_budget_frame));

        let destination = data.path().join("capped.mp4");
        let accepted_save_export = accepted.save_export.clone();
        let mut final_attempt = 0;
        session
            .save_new(
                RecordingSaveRequest {
                    destination: destination.clone(),
                    export: accepted_save_export,
                },
                &CancelToken::default(),
                |progress| final_attempt = final_attempt.max(progress.attempt),
            )
            .unwrap();
        assert!(fs::metadata(destination).unwrap().len() <= 100_000);
        assert!((1..=4).contains(&final_attempt));
        assert_eq!(session.frame().dimensions(), (640, 360));

        // Both request versions retain the accepted Save-new-copy budget when
        // changing only the edit, and therefore revalidate its retry plan.
        short_edit.crop = Some(captures_media::CropRect {
            x: 20,
            y: 10,
            width: 600,
            height: 340,
        });
        session
            .execute(RecordingEditorRequest::UpdateEdit {
                edit: short_edit.clone(),
            })
            .unwrap();
        assert_eq!(session.snapshot_v2().save_export, &maximum_mp4());
        let before_invalid_edit = serde_json::to_value(session.snapshot_v2()).unwrap();
        let before_invalid_frame = session.frame();
        assert!(
            session
                .execute_v2(RecordingEditorRequestV2::UpdateEdit {
                    edit: EditSpec::default(),
                })
                .unwrap_err()
                .contains("maximum file size cannot be reached")
        );
        assert_eq!(
            serde_json::to_value(session.snapshot_v2()).unwrap(),
            before_invalid_edit
        );
        assert!(Arc::ptr_eq(&session.frame(), &before_invalid_frame));

        let preview_export = session.snapshot().preview_export.clone();
        let save_export = session.snapshot_v2().save_export.clone();
        session
            .execute_v2(RecordingEditorRequestV2::Seek { position_ms: 750 })
            .unwrap();
        assert_eq!(session.snapshot().preview_export, &preview_export);
        assert_eq!(session.snapshot_v2().save_export, &save_export);
        let after_seek = serde_json::to_value(session.snapshot_v2()).unwrap();
        let after_seek_frame = session.frame();
        let playback = session.playback(750, &CancelToken::default()).unwrap();
        assert_eq!(playback.start_position_ms(), 750);
        drop(playback);
        assert_eq!(
            serde_json::to_value(session.snapshot_v2()).unwrap(),
            after_seek
        );
        assert!(Arc::ptr_eq(&session.frame(), &after_seek_frame));

        // A v1 preview acceptance intentionally replaces both exports with its
        // budget-free spec. A later v1 edit is consequently no longer subject
        // to the previously accepted v2 budget.
        session
            .execute(RecordingEditorRequest::UpdatePreview {
                edit: short_edit,
                export: preview_export.clone(),
            })
            .unwrap();
        assert_eq!(session.snapshot().preview_export, &preview_export);
        assert_eq!(session.snapshot_v2().save_export, &preview_export);
        session
            .execute(RecordingEditorRequest::UpdateEdit {
                edit: EditSpec::default(),
            })
            .unwrap();
        assert_eq!(session.snapshot_v2().save_export.max_size_bytes, None);
        assert_eq!(fs::read(&session.source_path).unwrap(), source_bytes);
    }

    #[test]
    fn encoded_comparison_is_retained_immutable_and_rejects_outside_trim() {
        let Some((tools, ffmpeg)) = real_tools() else {
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("source.mp4");
        create_video(&ffmpeg, &source, "2");
        let history_root = data.path().join("history");
        let mut session = open_session(tools, &source, &history_root);
        let source_bytes = fs::read(&session.source_path).unwrap();
        let edit = EditSpec {
            trim_start_ms: 200,
            trim_end_ms: Some(1_700),
            crop: Some(captures_media::CropRect {
                x: 40,
                y: 20,
                width: 480,
                height: 270,
            }),
            ..EditSpec::default()
        };
        session
            .execute_v2(RecordingEditorRequestV2::UpdatePreview {
                edit,
                export: ExportSpec {
                    format: ExportFormat::Gif,
                    quality: QualityPreset::Preserve,
                    max_size_bytes: Some(100_000),
                    frames_per_second: Some(8),
                    gif_max_colors: Some(64),
                },
            })
            .unwrap();
        let outside_snapshot = serde_json::to_value(session.snapshot_v2()).unwrap();
        let outside_frame = session.frame();
        assert!(
            session
                .export_comparison(&CancelToken::default())
                .err()
                .unwrap()
                .contains("accepted trim")
        );
        assert_eq!(
            serde_json::to_value(session.snapshot_v2()).unwrap(),
            outside_snapshot
        );
        assert!(Arc::ptr_eq(&session.frame(), &outside_frame));
        assert_eq!(fs::read_dir(session.scratch.path()).unwrap().count(), 0);

        session
            .execute(RecordingEditorRequest::Seek { position_ms: 500 })
            .unwrap();
        let snapshot = serde_json::to_value(session.snapshot_v2()).unwrap();
        let frame = session.frame();
        let cancelled = CancelToken::default();
        cancelled.cancel();
        assert!(session.export_comparison(&cancelled).is_err());
        let comparison = session.export_comparison(&CancelToken::default()).unwrap();
        assert_eq!(comparison.position_ms, 500);
        assert_eq!(comparison.revision, session.snapshot().revision);
        assert_eq!(comparison.export.max_size_bytes, None);
        assert_eq!(comparison.attempts, 1);
        assert_eq!(comparison.before_frame().dimensions(), frame.dimensions());
        assert_eq!(comparison.after_frame().dimensions(), frame.dimensions());
        assert_eq!(
            serde_json::to_value(session.snapshot_v2()).unwrap(),
            snapshot
        );
        assert!(Arc::ptr_eq(&session.frame(), &frame));
        assert_eq!(fs::read_dir(session.scratch.path()).unwrap().count(), 0);
        assert_eq!(fs::read(&session.source_path).unwrap(), source_bytes);
        let before = comparison.before_frame();
        let after = comparison.after_frame();
        assert_ne!(
            before.as_raw(),
            after.as_raw(),
            "palette encoding must change high-color source pixels"
        );
        drop(comparison);
        drop(session);
        assert_eq!(before.dimensions(), (480, 270));
        assert_eq!(after.dimensions(), (480, 270));
    }

    #[test]
    fn failed_cancelled_and_unattainable_saves_do_not_publish() {
        let Some((tools, ffmpeg)) = real_tools() else {
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("source.mp4");
        create_video(&ffmpeg, &source, "4");
        let history_root = data.path().join("history");
        let mut session = open_session(tools, &source, &history_root);
        session
            .execute_v2(RecordingEditorRequestV2::UpdatePreview {
                edit: EditSpec::default(),
                export: maximum_gif(),
            })
            .unwrap();
        let accepted = serde_json::to_value(session.snapshot_v2()).unwrap();
        let frame = session.frame();
        let source_bytes = fs::read(&session.source_path).unwrap();
        let history_entries = || fs::read_dir(&history_root).unwrap().count();
        let original_history_entries = history_entries();

        let existing = data.path().join("existing.gif");
        fs::write(&existing, b"keep").unwrap();
        assert!(
            session
                .save_new(
                    RecordingSaveRequest {
                        destination: existing.clone(),
                        export: maximum_gif(),
                    },
                    &CancelToken::default(),
                    |_| {},
                )
                .is_err()
        );
        assert_eq!(fs::read(existing).unwrap(), b"keep");

        let cancelled_path = data.path().join("cancelled.gif");
        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(
            session
                .save_new(
                    RecordingSaveRequest {
                        destination: cancelled_path.clone(),
                        export: maximum_gif(),
                    },
                    &cancel,
                    |_| {},
                )
                .is_err()
        );
        assert!(!cancelled_path.exists());

        let unattainable = data.path().join("unattainable.gif");
        assert!(
            session
                .save_new(
                    RecordingSaveRequest {
                        destination: unattainable.clone(),
                        export: maximum_gif(),
                    },
                    &CancelToken::default(),
                    |_| {},
                )
                .unwrap_err()
                .contains("maximum file size cannot be reached")
        );
        assert!(!unattainable.exists());
        assert_eq!(history_entries(), original_history_entries);
        assert_eq!(
            serde_json::to_value(session.snapshot_v2()).unwrap(),
            accepted
        );
        assert!(Arc::ptr_eq(&session.frame(), &frame));
        assert_eq!(fs::read(&session.source_path).unwrap(), source_bytes);
    }
}
