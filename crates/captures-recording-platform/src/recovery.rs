//! Recovery of interrupted native recording bundles. Roots are supplied by a
//! host's isolated development profile; this does not migrate installed data.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use captures_history::{ArtifactKind, HistoryEntry};
use captures_media::{CancelToken, MediaToolchain, RecordingAssemblyKind, RecordingSegmentInput};
use captures_recording::{DraftStore, RecordingDraftManifest, RecordingKind, RecordingState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const INTENT: &str = "publication-intent-v1.json";

#[derive(Debug, Serialize)]
pub struct RecoveryDraft {
    pub session_id: String,
    pub status: &'static str,
    pub kind: Option<RecordingKind>,
    pub created_at_ms: Option<u64>,
    pub completed_duration_ms: u64,
    pub identity: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecoveryOutcome {
    pub status: &'static str,
    pub entry: HistoryEntry,
    pub path: PathBuf,
    pub warning: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProgress {
    Scanning,
    Assembling,
    Poster,
    Publishing,
}

#[derive(Serialize, Deserialize)]
struct PublicationIntent {
    version: u8,
    session_id: String,
    artifact_id: String,
    content_sha256: String,
}

pub struct RecordingRecovery {
    root: PathBuf,
    history_root: PathBuf,
    tools: MediaToolchain,
}

/// The lockfile is never deleted: removing it could create independent locks
/// on separate inodes. A live native session holds this until finish/discard.
pub(crate) fn lease(root: &Path) -> Result<File, String> {
    fs::create_dir_all(root).map_err(string)?;
    if !fs::symlink_metadata(root)
        .map_err(string)?
        .file_type()
        .is_dir()
    {
        return Err("Recovery root must be a real directory".into());
    }
    let lock = root.join(".recording-recovery.lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock)
        && !metadata.file_type().is_file()
    {
        return Err("Recovery lock must be a regular file".into());
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock)
        .map_err(string)?;
    file.try_lock()
        .map_err(|_| "A native recording or recovery operation is active".to_owned())?;
    Ok(file)
}

impl RecordingRecovery {
    pub fn new(history_root: PathBuf, tools: MediaToolchain) -> Self {
        let root = history_root.with_file_name("recording-recovery");
        Self {
            root,
            history_root,
            tools,
        }
    }

    pub fn list(&self) -> Result<Vec<RecoveryDraft>, String> {
        let _lease = lease(&self.root)?;
        let mut drafts = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(string)? {
            let entry = entry.map_err(string)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let parsed = self.read(&name);
            let (status, kind, created_at_ms, completed_duration_ms, identity, reason) =
                match parsed {
                    Ok((manifest, identity)) => {
                        if manifest.state == RecordingState::Ready
                            || manifest.state == RecordingState::Discarded
                        {
                            continue;
                        }
                        let valid = recoverable(manifest.state);
                        (
                            if valid { "recoverable" } else { "unavailable" },
                            Some(manifest.options.kind),
                            Some(manifest.created_at_ms),
                            manifest
                                .segments
                                .iter()
                                .filter(|s| s.complete)
                                .map(|s| s.duration_ms)
                                .sum(),
                            Some(identity),
                            (!valid).then(|| "Draft is not interrupted".into()),
                        )
                    }
                    Err(error) => ("unavailable", None, None, 0, None, Some(error)),
                };
            if !path.exists() && !path.is_symlink() {
                continue;
            }
            drafts.push(RecoveryDraft {
                session_id: name,
                status,
                kind,
                created_at_ms,
                completed_duration_ms,
                identity,
                reason,
            });
        }
        drafts.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
        Ok(drafts)
    }

    pub fn recover(
        &self,
        session_id: &str,
        expected_identity: &str,
        cancel: &CancelToken,
        mut progress: impl FnMut(RecoveryProgress),
    ) -> Result<RecoveryOutcome, String> {
        let _lease = lease(&self.root)?;
        check_cancel(cancel)?;
        let (mut manifest, identity) = self.read(session_id)?;
        if identity != expected_identity {
            return Err("Recovery draft changed since listing".into());
        }
        let bundle = self.root.join(session_id);
        let intent_path = bundle.join(INTENT);
        let intent = if intent_path.exists() {
            let intent: PublicationIntent =
                serde_json::from_slice(&regular_bytes(&intent_path)?).map_err(string)?;
            if intent.version != 1
                || intent.session_id != session_id
                || intent.artifact_id != session_id
            {
                return Err("Recovery publication intent is invalid".into());
            }
            intent
        } else {
            PublicationIntent {
                version: 1,
                session_id: session_id.into(),
                artifact_id: session_id.into(),
                content_sha256: String::new(),
            }
        };
        if let Some(previous) = self.previous(&intent, &manifest)? {
            return Ok(previous);
        }
        if !recoverable(manifest.state) {
            return Err("Draft is not interrupted".into());
        }
        check_cancel(cancel)?;
        progress(RecoveryProgress::Scanning);
        let mut segments = Vec::new();
        for segment in &mut manifest.segments {
            let video = child(&bundle, &segment.relative_path)?;
            if !video.exists() && !segment.complete {
                continue;
            }
            if !video.is_file() {
                return Err(format!("Completed segment {} is missing", segment.index));
            }
            let probe = match self.tools.probe_with_cancel(&video, cancel) {
                Ok(probe) => probe,
                Err(_) if !segment.complete && !cancel.is_cancelled() => continue,
                Err(error) => return Err(string(error)),
            };
            if probe.metadata.mime_type != "video/mp4" || probe.metadata.duration_ms == Some(0) {
                return Err("Recovery segment is not a complete MP4".into());
            }
            segment.complete = true;
            segment.duration_ms = probe.metadata.duration_ms.unwrap_or(0);
            segment.width = probe.metadata.width;
            segment.height = probe.metadata.height;
            segment.size_bytes = probe.metadata.size_bytes;
            let sidecar = |relative: &Option<String>| -> Result<Option<PathBuf>, String> {
                relative
                    .as_ref()
                    .map(|relative| child(&bundle, relative))
                    .transpose()
                    .map(|path| path.filter(|path| path.is_file()))
            };
            segments.push(RecordingSegmentInput {
                video_path: video,
                system_audio_path: sidecar(&segment.system_audio_relative_path)?,
                system_audio_offset_ms: segment.system_audio_offset_ms,
                microphone_path: sidecar(&segment.microphone_relative_path)?,
                microphone_offset_ms: segment.microphone_offset_ms,
                duration_ms: segment.duration_ms,
            });
        }
        if segments.is_empty() {
            return Err("No playable segments could be recovered".into());
        }
        check_cancel(cancel)?;
        let scratch = tempfile::Builder::new()
            .prefix(".recovery-")
            .tempdir_in(&bundle)
            .map_err(string)?;
        let (extension, kind) = match manifest.options.kind {
            RecordingKind::Video => (
                "mp4",
                RecordingAssemblyKind::Video {
                    capture_system_audio: manifest.options.audio.capture_system_audio,
                },
            ),
            RecordingKind::Gif => (
                "gif",
                RecordingAssemblyKind::Gif {
                    frames_per_second: manifest.options.frames_per_second,
                    max_width: manifest.options.gif.max_width,
                    max_colors: manifest.options.gif.max_colors,
                },
            ),
        };
        let assembled = scratch.path().join(format!("assembled.{extension}"));
        progress(RecoveryProgress::Assembling);
        let outcome = self
            .tools
            .assemble_recording(&segments, &assembled, scratch.path(), kind, cancel)
            .map_err(string)?;
        check_cancel(cancel)?;
        let probe = self
            .tools
            .probe_with_cancel(&assembled, cancel)
            .map_err(string)?;
        if probe.metadata.mime_type
            != if extension == "gif" {
                "image/gif"
            } else {
                "video/mp4"
            }
            || probe.metadata.width == 0
            || probe.metadata.height == 0
            || probe.metadata.duration_ms == Some(0)
        {
            return Err("Recovered media has an invalid format or dimensions".into());
        }
        progress(RecoveryProgress::Poster);
        let poster = scratch.path().join("poster.png");
        self.tools
            .create_poster(&assembled, &poster, cancel)
            .map_err(string)?;
        let poster = regular_bytes(&poster)?;
        check_cancel(cancel)?;
        // Recheck source and metadata after slow media work. Intent is recorded
        // before History publication, so a process kill can retry by ID.
        if self.read(session_id)?.1 != identity {
            return Err("Recovery draft changed during assembly".into());
        }
        if self.history_entry(session_id)?.is_some() {
            return Err("History artifact ID is already occupied".into());
        }
        let intent = PublicationIntent {
            content_sha256: digest_file(&assembled)?,
            ..intent
        };
        atomic_intent(&intent_path, &intent)?;
        check_cancel(cancel)?;
        let entry = HistoryEntry {
            id: intent.artifact_id,
            kind: if extension == "gif" {
                ArtifactKind::Gif
            } else {
                ArtifactKind::Video
            },
            preview_url: String::new(),
            full_url: String::new(),
            width: probe.metadata.width,
            height: probe.metadata.height,
            size_bytes: probe.metadata.size_bytes,
            created_at: chrono::Utc::now().to_rfc3339(),
            mode: None,
            saved_path: None,
            mime_type: Some(probe.metadata.mime_type),
            duration_ms: probe.metadata.duration_ms,
            target: Some(manifest.options.target.clone()),
            has_system_audio: extension == "mp4"
                && probe.has_audio
                && manifest.options.audio.capture_system_audio,
            has_microphone_audio: extension == "mp4"
                && probe.has_audio
                && outcome.has_microphone_audio,
            dropped_frames: manifest
                .segments
                .iter()
                .filter(|s| s.complete)
                .map(|s| s.dropped_frames)
                .sum(),
        };
        progress(RecoveryProgress::Publishing);
        let path =
            captures_history::save_recording(&self.history_root, &entry, &poster, &assembled)
                .map_err(string)?;
        // Do not turn successful publication into a retryable failure.
        manifest.state = RecordingState::Ready;
        manifest.final_path = Some(path.to_string_lossy().into_owned());
        let mut warning = DraftStore::new(self.root.clone())
            .save(&manifest)
            .err()
            .map(|error| error.to_string());
        if warning.is_none() && manifest.options.kind == RecordingKind::Video {
            warning = DraftStore::new(self.root.clone())
                .remove(session_id)
                .err()
                .map(|error| error.to_string());
        }
        Ok(RecoveryOutcome {
            status: "recovered",
            entry,
            path,
            warning,
        })
    }

    pub fn discard(
        &self,
        session_id: &str,
        expected_identity: &str,
    ) -> Result<&'static str, String> {
        let _lease = lease(&self.root)?;
        let (manifest, identity) = self.read(session_id)?;
        if identity != expected_identity {
            return Err("Recovery draft changed since listing".into());
        }
        if !recoverable(manifest.state) {
            return Err("Only interrupted drafts may be discarded".into());
        }
        let bundle = self.root.join(session_id);
        reject_links(&bundle)?;
        let bundle_id = file_id::get_file_id(&bundle).map_err(string)?;
        let quarantine = self.root.join(format!(".discard-{}", uuid::Uuid::new_v4()));
        fs::rename(&bundle, &quarantine).map_err(string)?;
        // Never recursively remove an unexpected tree substituted at the path.
        if file_id::get_file_id(&quarantine).map_err(string)? != bundle_id {
            return Err("Recovery bundle changed while discarding".into());
        }
        reject_links(&quarantine)?;
        fs::remove_dir_all(quarantine).map_err(string)?;
        Ok("discarded")
    }

    fn read(&self, id: &str) -> Result<(RecordingDraftManifest, String), String> {
        uuid::Uuid::parse_str(id).map_err(string)?;
        let bundle = self.root.join(id);
        if !fs::symlink_metadata(&bundle)
            .map_err(string)?
            .file_type()
            .is_dir()
        {
            return Err("Recovery bundle is not a real directory".into());
        }
        let manifest_path = bundle.join("manifest.json");
        let bytes = regular_bytes(&manifest_path)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Recovery manifest is too large".into());
        }
        let manifest: RecordingDraftManifest = serde_json::from_slice(&bytes).map_err(string)?;
        if manifest.schema_version != 1 || manifest.session_id != id {
            return Err("Foreign recovery manifest".into());
        }
        let directory_id = file_id::get_file_id(&bundle).map_err(string)?;
        let mut hash = Sha256::new();
        hash.update(format!("{directory_id:?}"));
        hash.update(&bytes);
        for segment in &manifest.segments {
            for relative in [
                Some(&segment.relative_path),
                segment.system_audio_relative_path.as_ref(),
                segment.microphone_relative_path.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                let path = child(&bundle, relative)?;
                hash.update(relative.as_bytes());
                if path.exists() {
                    hash.update(digest_file(&path)?.as_bytes());
                } else {
                    hash.update(b"missing");
                }
            }
        }
        Ok((manifest, format!("{:x}", hash.finalize())))
    }

    fn history_entry(&self, id: &str) -> Result<Option<HistoryEntry>, String> {
        let path = captures_history::entry_directory(&self.history_root, id).map_err(string)?;
        if fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(None);
        }
        if !fs::symlink_metadata(&path)
            .map_err(string)?
            .file_type()
            .is_dir()
        {
            return Err("History artifact path is not a real directory".into());
        }
        let entry: HistoryEntry = serde_json::from_slice(&regular_bytes(
            &path.join(captures_history::HISTORY_METADATA_FILE),
        )?)
        .map_err(string)?;
        Ok(Some(entry))
    }

    fn previous(
        &self,
        intent: &PublicationIntent,
        manifest: &RecordingDraftManifest,
    ) -> Result<Option<RecoveryOutcome>, String> {
        let Some(entry) = self.history_entry(&intent.artifact_id)? else {
            return Ok(None);
        };
        if !self.root.join(&intent.session_id).join(INTENT).is_file()
            || entry.id != intent.artifact_id
            || entry.target.as_ref() != Some(&manifest.options.target)
            || entry.kind
                != if manifest.options.kind == RecordingKind::Gif {
                    ArtifactKind::Gif
                } else {
                    ArtifactKind::Video
                }
            || entry.saved_path.is_some()
        {
            return Err("History artifact ID is already occupied".into());
        }
        let path = entry
            .recording_media_path(&self.history_root)
            .ok_or("Published media is unavailable")?;
        if !fs::symlink_metadata(&path)
            .map_err(string)?
            .file_type()
            .is_file()
            || digest_file(&path)? != intent.content_sha256
        {
            return Err("Published media is unavailable".into());
        }
        Ok(Some(RecoveryOutcome {
            status: "already_recovered",
            entry,
            path,
            warning: None,
        }))
    }
}

fn recoverable(state: RecordingState) -> bool {
    matches!(
        state,
        RecordingState::Recording
            | RecordingState::Paused
            | RecordingState::Finalizing
            | RecordingState::Failed
    )
}

fn child(bundle: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Invalid recovery media path".into());
    }
    let mut path = bundle.to_path_buf();
    for (index, component) in relative.components().enumerate() {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("Recovery media contains a link".into());
            }
            Ok(metadata) if index + 1 < relative.components().count() && !metadata.is_dir() => {
                return Err("Recovery media parent is not a directory".into());
            }
            Ok(metadata) if index + 1 == relative.components().count() && !metadata.is_file() => {
                return Err("Recovery media is not a file".into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(string(error)),
            _ => {}
        }
    }
    Ok(bundle.join(relative))
}

fn reject_links(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(string)?;
    if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
        return Err("Recovery bundle contains a link or special file".into());
    }
    if metadata.is_dir() {
        for item in fs::read_dir(path).map_err(string)? {
            reject_links(&item.map_err(string)?.path())?;
        }
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(string)?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(string)?;
        if read == 0 {
            break;
        }
        hash.update(&chunk[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn regular_bytes(path: &Path) -> Result<Vec<u8>, String> {
    if !fs::symlink_metadata(path)
        .map_err(string)?
        .file_type()
        .is_file()
    {
        return Err("Recovery file must be regular".into());
    }
    fs::read(path).map_err(string)
}

fn atomic_intent(path: &Path, intent: &PublicationIntent) -> Result<(), String> {
    let parent = path.parent().ok_or("Recovery bundle is unavailable")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(string)?;
    tmp.write_all(&serde_json::to_vec(intent).map_err(string)?)
        .map_err(string)?;
    tmp.as_file().sync_all().map_err(string)?;
    tmp.persist(path).map_err(string)?;
    Ok(())
}

fn check_cancel(token: &CancelToken) -> Result<(), String> {
    if token.is_cancelled() {
        Err("Recording recovery cancelled".into())
    } else {
        Ok(())
    }
}

fn string(error: impl std::fmt::Display) -> String {
    error.to_string()
}
