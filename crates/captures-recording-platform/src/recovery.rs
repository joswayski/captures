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
    source_identity: String,
    source_sha256: String,
    manifest_sha256: String,
    published_manifest_sha256: String,
    entry_sha256: String,
}

pub struct RecordingRecovery {
    root: PathBuf,
    history_root: PathBuf,
    tools: MediaToolchain,
}

pub(crate) struct RecoveryLease(File);

impl Drop for RecoveryLease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// The lockfile is never deleted: removing it could create independent locks
/// on separate inodes. A live native session holds this until finish/discard.
pub(crate) fn lease(root: &Path) -> Result<RecoveryLease, String> {
    fs::create_dir_all(root).map_err(string)?;
    let metadata = fs::symlink_metadata(root).map_err(string)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err("Recovery root must be a real directory".into());
    }
    let lock = root.join(".recording-recovery.lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock)
        && (!metadata.file_type().is_file() || is_link(&metadata))
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
    Ok(RecoveryLease(file))
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
        progress: impl FnMut(RecoveryProgress),
    ) -> Result<RecoveryOutcome, String> {
        self.recover_with_cleanup(
            session_id,
            expected_identity,
            cancel,
            progress,
            |store, id| store.remove(id).map_err(string),
        )
    }

    fn recover_with_cleanup(
        &self,
        session_id: &str,
        expected_identity: &str,
        cancel: &CancelToken,
        mut progress: impl FnMut(RecoveryProgress),
        cleanup: impl Fn(&DraftStore, &str) -> Result<(), String>,
    ) -> Result<RecoveryOutcome, String> {
        let _lease = lease(&self.root)?;
        check_cancel(cancel)?;
        let (mut manifest, identity) = self.read(session_id)?;
        let bundle = self.root.join(session_id);
        let intent_path = bundle.join(INTENT);
        let intent = if intent_path.exists() {
            let intent: PublicationIntent =
                serde_json::from_slice(&regular_bytes(&intent_path, 64 * 1024)?).map_err(string)?;
            if intent.version != 1
                || intent.session_id != session_id
                || intent.artifact_id != session_id
            {
                return Err("Recovery publication intent is invalid".into());
            }
            if intent.source_identity != expected_identity
                || manifest_digest(&manifest)?.as_str()
                    != if manifest.state == RecordingState::Ready {
                        intent.published_manifest_sha256.as_str()
                    } else {
                        intent.manifest_sha256.as_str()
                    }
                || source_digest(&bundle, &manifest, cancel)? != intent.source_sha256
            {
                return Err("Recovery source changed since publication intent".into());
            }
            intent
        } else {
            if identity != expected_identity {
                return Err("Recovery draft changed since listing".into());
            }
            PublicationIntent {
                version: 1,
                session_id: session_id.into(),
                artifact_id: session_id.into(),
                content_sha256: String::new(),
                source_identity: identity.clone(),
                source_sha256: source_digest(&bundle, &manifest, cancel)?,
                manifest_sha256: manifest_digest(&manifest)?,
                published_manifest_sha256: String::new(),
                entry_sha256: String::new(),
            }
        };
        if let Some(mut previous) = self.previous(&intent, &manifest, cancel)? {
            manifest.state = RecordingState::Ready;
            manifest.final_path = Some(previous.path.to_string_lossy().into_owned());
            previous.warning = DraftStore::new(self.root.clone())
                .save(&manifest)
                .err()
                .map(|error| error.to_string());
            if previous.warning.is_none() && manifest.options.kind == RecordingKind::Video {
                previous.warning = cleanup(&DraftStore::new(self.root.clone()), session_id).err();
            }
            return Ok(previous);
        }
        if !recoverable(manifest.state) {
            return Err("Draft is not interrupted".into());
        }
        check_cancel(cancel)?;
        progress(RecoveryProgress::Scanning);
        let mut segments = Vec::new();
        let mut system_audio_available = false;
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
            let system_audio_path = sidecar(&segment.system_audio_relative_path)?;
            system_audio_available |= probe.has_audio || system_audio_path.is_some();
            segments.push(RecordingSegmentInput {
                video_path: video,
                system_audio_path,
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
                    capture_system_audio: manifest.options.audio.capture_system_audio
                        && system_audio_available,
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
        let poster = regular_bytes(&poster, 128 * 1024 * 1024)?;
        check_cancel(cancel)?;
        // Recheck source and metadata after slow media work. Intent is recorded
        // before History publication, so a process kill can retry by ID.
        if self.read(session_id)?.1 != identity
            || source_digest(&bundle, &manifest, cancel)? != intent.source_sha256
        {
            return Err("Recovery draft changed during assembly".into());
        }
        if self.history_entry(session_id)?.is_some() {
            return Err("History artifact ID is already occupied".into());
        }
        let mut intent = PublicationIntent {
            content_sha256: digest_file(&assembled, cancel)?,
            published_manifest_sha256: manifest_digest(&manifest)?,
            ..intent
        };
        let entry = HistoryEntry {
            id: intent.artifact_id.clone(),
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
                && manifest.options.audio.capture_system_audio
                && system_audio_available,
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
        intent.entry_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec_pretty(&entry).map_err(string)?)
        );
        atomic_intent(&intent_path, &intent)?;
        check_cancel(cancel)?;
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
            warning = cleanup(&DraftStore::new(self.root.clone()), session_id).err();
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
        self.discard_with(session_id, expected_identity, |path| {
            fs::remove_dir_all(path)
        })
    }

    fn discard_with(
        &self,
        session_id: &str,
        expected_identity: &str,
        remove: impl FnOnce(&Path) -> std::io::Result<()>,
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
        if file_id::get_file_id(&quarantine).ok() != Some(bundle_id) {
            return Err(format!(
                "Recovery bundle changed while discarding; remaining data is at {}",
                quarantine.display()
            ));
        }
        if let Err(error) =
            reject_links(&quarantine).and_then(|()| remove(&quarantine).map_err(string))
        {
            let restored = fs::rename(&quarantine, &bundle);
            return Err(match restored {
                Ok(()) => error,
                Err(restore) => format!(
                    "Discard failed ({error}); remaining bundle is at {} (restore failed: {restore})",
                    quarantine.display()
                ),
            });
        }
        Ok("discarded")
    }

    fn read(&self, id: &str) -> Result<(RecordingDraftManifest, String), String> {
        uuid::Uuid::parse_str(id).map_err(string)?;
        let bundle = self.root.join(id);
        let metadata = fs::symlink_metadata(&bundle).map_err(string)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err("Recovery bundle is not a real directory".into());
        }
        reject_links(&bundle)?;
        let manifest_path = bundle.join("manifest.json");
        let bytes = regular_bytes(&manifest_path, 8 * 1024 * 1024)?;
        let manifest: RecordingDraftManifest = serde_json::from_slice(&bytes).map_err(string)?;
        if manifest.schema_version != 1 || manifest.session_id != id {
            return Err("Foreign recovery manifest".into());
        }
        let directory_id = file_id::get_file_id(&bundle).map_err(string)?;
        let mut identity = format!("{directory_id:?}:{}", file_fingerprint(&manifest_path)?);
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
                identity.push_str(relative);
                if path.exists() {
                    identity.push_str(&file_fingerprint(&path)?);
                } else {
                    identity.push_str("missing");
                }
            }
        }
        Ok((
            manifest,
            format!("{:x}", Sha256::digest(identity.as_bytes())),
        ))
    }

    fn history_entry(&self, id: &str) -> Result<Option<HistoryEntry>, String> {
        let path = captures_history::entry_directory(&self.history_root, id).map_err(string)?;
        if fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(None);
        }
        let metadata = fs::symlink_metadata(&path).map_err(string)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err("History artifact path is not a real directory".into());
        }
        let entry: HistoryEntry = serde_json::from_slice(&regular_bytes(
            &path.join(captures_history::HISTORY_METADATA_FILE),
            8 * 1024 * 1024,
        )?)
        .map_err(string)?;
        Ok(Some(entry))
    }

    fn previous(
        &self,
        intent: &PublicationIntent,
        manifest: &RecordingDraftManifest,
        cancel: &CancelToken,
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
            || format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec_pretty(&entry).map_err(string)?)
            ) != intent.entry_sha256
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
            || digest_file(&path, cancel)? != intent.content_sha256
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
            Ok(metadata) if is_link(&metadata) => {
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
    if is_link(&metadata) || !(metadata.is_dir() || metadata.is_file()) {
        return Err("Recovery bundle contains a link or special file".into());
    }
    if metadata.is_dir() {
        for item in fs::read_dir(path).map_err(string)? {
            reject_links(&item.map_err(string)?.path())?;
        }
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Directory junctions and other reparse points need rejection too.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn digest_file(path: &Path, cancel: &CancelToken) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(string)?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let read = file.read(&mut chunk).map_err(string)?;
        if read == 0 {
            break;
        }
        hash.update(&chunk[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn file_fingerprint(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(string)?;
    if !metadata.file_type().is_file() {
        return Err("Recovery source is not a regular file".into());
    }
    let id = file_id::get_file_id(path).map_err(string)?;
    let changed = metadata.modified().map_err(string)?;
    #[cfg(unix)]
    let change_time = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.ctime(), metadata.ctime_nsec())
    };
    #[cfg(not(unix))]
    let change_time = String::new();
    Ok(format!(
        "{id:?}:{}:{changed:?}:{change_time}",
        metadata.len()
    ))
}

fn source_digest(
    bundle: &Path,
    manifest: &RecordingDraftManifest,
    cancel: &CancelToken,
) -> Result<String, String> {
    let mut hash = Sha256::new();
    for segment in &manifest.segments {
        for relative in [
            Some(&segment.relative_path),
            segment.system_audio_relative_path.as_ref(),
            segment.microphone_relative_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            check_cancel(cancel)?;
            hash.update(relative.as_bytes());
            let path = child(bundle, relative)?;
            if path.exists() {
                let mut file = File::open(path).map_err(string)?;
                let mut bytes = [0u8; 64 * 1024];
                loop {
                    check_cancel(cancel)?;
                    let count = file.read(&mut bytes).map_err(string)?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&bytes[..count]);
                }
            } else {
                hash.update(b"missing");
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn manifest_digest(manifest: &RecordingDraftManifest) -> Result<String, String> {
    let mut normalized = manifest.clone();
    // Publication is allowed to transition Ready and set final_path only.
    normalized.state = RecordingState::Ready;
    normalized.final_path = None;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&normalized).map_err(string)?)
    ))
}

fn regular_bytes(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    if !fs::symlink_metadata(path)
        .map_err(string)?
        .file_type()
        .is_file()
    {
        return Err("Recovery file must be regular".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(string)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(string)?;
    if bytes.len() as u64 > max_bytes {
        return Err("Recovery metadata exceeds its size limit".into());
    }
    Ok(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;
    use captures_capture::DisplayDescriptor;
    use captures_recording::{
        AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingOptions,
        RecordingSegmentManifest, RecordingTarget,
    };
    use std::process::Command;

    fn test_ffmpeg() -> std::ffi::OsString {
        std::env::var_os("CAPTURES_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
    }

    fn test_ffprobe() -> std::ffi::OsString {
        std::env::var_os("CAPTURES_TEST_FFPROBE").unwrap_or_else(|| "ffprobe".into())
    }

    fn options(kind: RecordingKind) -> RecordingOptions {
        RecordingOptions {
            kind,
            target: RecordingTarget::Region {
                display_id: "fixture".into(),
                rect: CaptureRect {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 48,
                },
            },
            frames_per_second: 15,
            max_resolution: MaxResolution::Original,
            countdown_seconds: 0,
            show_cursor: false,
            highlight_clicks: false,
            show_keystrokes: false,
            audio: AudioOptions {
                capture_system_audio: true,
                ..AudioOptions::default()
            },
            gif: GifOptions::default(),
        }
    }

    fn fixture(kind: RecordingKind) -> Option<(tempfile::TempDir, RecordingRecovery, String)> {
        let tools = MediaToolchain::new(test_ffmpeg().into(), test_ffprobe().into());
        if tools.verify().is_err() {
            return None;
        }
        let base = tempfile::tempdir().unwrap();
        let history = base.path().join("history");
        let recovery = RecordingRecovery::new(history, tools);
        let id = uuid::Uuid::new_v4().to_string();
        let mut manifest = RecordingDraftManifest::new(id.clone(), options(kind), 10);
        manifest.state = RecordingState::Failed;
        let store = DraftStore::new(recovery.root.clone());
        let directory = store.create(&manifest).unwrap();
        for (index, color) in ["red", "blue"].iter().enumerate() {
            let name = format!("segment-{index:03}.mp4");
            let path = directory.join(&name);
            let status = Command::new(test_ffmpeg())
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                ])
                .arg(format!("color=c={color}:size=64x48:rate=10:duration=0.5"))
                .args([
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.5",
                    "-c:v",
                    "mpeg4",
                    "-c:a",
                    "aac",
                    "-shortest",
                ])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            manifest.segments.push(RecordingSegmentManifest {
                index: index as u32,
                relative_path: name,
                system_audio_relative_path: None,
                system_audio_offset_ms: 0,
                system_audio_warning: None,
                microphone_relative_path: None,
                microphone_offset_ms: 0,
                microphone_warning: None,
                started_at_ms: index as u64 * 500,
                duration_ms: 500,
                width: 64,
                height: 48,
                size_bytes: fs::metadata(path).unwrap().len(),
                dropped_frames: index as u64,
                complete: true,
            });
        }
        store.save(&manifest).unwrap();
        Some((base, recovery, id))
    }

    fn identity(recovery: &RecordingRecovery, id: &str) -> String {
        recovery
            .list()
            .unwrap()
            .into_iter()
            .find(|row| row.session_id == id)
            .unwrap()
            .identity
            .unwrap()
    }

    #[test]
    fn native_lease_excludes_recovery_and_remains_stable_after_release() {
        let base = tempfile::tempdir().unwrap();
        let history = base.path().join("history");
        let recovery = RecordingRecovery::new(history, MediaToolchain::from_command_names());
        let display = DisplayDescriptor {
            id: "fixture".into(),
            name: "Fixture".into(),
            x: 0,
            y: 0,
            width: 64,
            height: 48,
            scale_factor: 1.,
            is_primary: true,
        };
        let mut live = crate::RecordingSession::prepare(
            recovery.root.clone(),
            options(RecordingKind::Video),
            display,
        )
        .unwrap();
        assert!(recovery.list().unwrap_err().contains("active"));
        assert!(lease(&recovery.root).is_err());
        live.discard().unwrap();
        assert!(recovery.list().unwrap().is_empty());
        assert!(recovery.root.join(".recording-recovery.lock").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn lease_guard_explicitly_unlocks_while_a_duplicated_descriptor_is_open() {
        let base = tempfile::tempdir().unwrap();
        let guard = lease(base.path()).unwrap();
        let duplicate = guard.0.try_clone().unwrap();
        assert!(lease(base.path()).is_err());
        drop(guard);
        // Closing just the guard's fd would leave the duplicated open-file
        // description locked. The guard must explicitly unlock it instead.
        assert!(lease(base.path()).is_ok());
        drop(duplicate);
    }

    #[test]
    fn interrupted_recovery_operation_unlocks_on_unwind() {
        let base = tempfile::tempdir().unwrap();
        let recovery = RecordingRecovery::new(
            base.path().join("history"),
            MediaToolchain::from_command_names(),
        );
        let id = uuid::Uuid::new_v4().to_string();
        let mut manifest =
            RecordingDraftManifest::new(id.clone(), options(RecordingKind::Video), 10);
        manifest.state = RecordingState::Failed;
        DraftStore::new(recovery.root.clone())
            .create(&manifest)
            .unwrap();
        let identity = recovery.list().unwrap()[0].identity.clone().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = recovery.recover(&id, &identity, &CancelToken::default(), |_| {
                panic!("injected progress callback panic");
            });
        }));
        assert!(result.is_err());
        assert_eq!(recovery.list().unwrap()[0].session_id, id);
        assert!(lease(&recovery.root).is_ok());
    }

    #[cfg(unix)] // set_len creates a sparse file here; Windows may allocate 8 GiB.
    #[test]
    fn listing_does_not_stream_large_media() {
        let Some((_base, recovery, id)) = fixture(RecordingKind::Video) else {
            return;
        };
        let video = recovery.root.join(&id).join("segment-000.mp4");
        fs::OpenOptions::new()
            .write(true)
            .open(video)
            .unwrap()
            .set_len(8 * 1024 * 1024 * 1024)
            .unwrap();
        let start = std::time::Instant::now();
        let row = recovery
            .list()
            .unwrap()
            .into_iter()
            .find(|draft| draft.session_id == id)
            .unwrap();
        assert_eq!(row.status, "recoverable");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "listing read the recording media"
        );
    }

    #[test]
    fn recovers_asymmetric_segments_once_and_preserves_provenance() {
        for kind in [RecordingKind::Video, RecordingKind::Gif] {
            let Some((_base, recovery, id)) = fixture(kind) else {
                return;
            };
            let identity = identity(&recovery, &id);
            let mut stages = Vec::new();
            let saved = recovery
                .recover(&id, &identity, &CancelToken::default(), |stage| {
                    stages.push(stage)
                })
                .unwrap();
            assert_eq!(saved.status, "recovered");
            assert_eq!(
                saved.entry.kind,
                if kind == RecordingKind::Gif {
                    ArtifactKind::Gif
                } else {
                    ArtifactKind::Video
                }
            );
            assert_eq!(saved.entry.dropped_frames, 1);
            assert_eq!(saved.entry.has_system_audio, kind == RecordingKind::Video);
            assert!(!saved.entry.has_microphone_audio);
            assert!(saved.path.is_file());
            assert!(matches!(
                stages.as_slice(),
                [
                    RecoveryProgress::Scanning,
                    RecoveryProgress::Assembling,
                    RecoveryProgress::Poster,
                    RecoveryProgress::Publishing
                ]
            ));
            if kind == RecordingKind::Gif {
                let again = recovery
                    .recover(&id, &identity, &CancelToken::default(), |_| {})
                    .unwrap();
                assert_eq!(again.status, "already_recovered");
                assert_eq!(again.path, saved.path);
                assert!(recovery.root.join(&id).is_dir());
            } else {
                assert!(!recovery.root.join(&id).exists());
                assert!(
                    recovery
                        .recover(&id, &identity, &CancelToken::default(), |_| {})
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn surviving_microphone_does_not_invent_system_audio() {
        let Some((_base, recovery, id)) = fixture(RecordingKind::Video) else {
            return;
        };
        let bundle = recovery.root.join(&id);
        let store = DraftStore::new(recovery.root.clone());
        let mut manifest = store.load(&id).unwrap();
        for segment in &mut manifest.segments {
            let video = bundle.join(&segment.relative_path);
            let silent = bundle.join(format!("silent-{}.mp4", segment.index));
            assert!(
                Command::new(test_ffmpeg())
                    .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                    .arg(&video)
                    .args(["-c:v", "copy", "-an"])
                    .arg(&silent)
                    .status()
                    .unwrap()
                    .success()
            );
            fs::rename(silent, &video).unwrap();
            let mic = format!("mic-{}.wav", segment.index);
            assert!(
                Command::new(test_ffmpeg())
                    .args([
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=731:duration=0.5"
                    ])
                    .arg(bundle.join(&mic))
                    .status()
                    .unwrap()
                    .success()
            );
            segment.microphone_relative_path = Some(mic);
            segment.microphone_offset_ms = if segment.index == 0 { 120 } else { -80 };
        }
        store.save(&manifest).unwrap();
        let accepted = identity(&recovery, &id);
        let saved = recovery
            .recover(&id, &accepted, &CancelToken::default(), |_| {})
            .unwrap();
        assert!(!saved.entry.has_system_audio);
        assert!(saved.entry.has_microphone_audio);
        let probe = recovery.tools.probe(&saved.path).unwrap();
        assert!(probe.has_audio);
        assert_eq!((probe.metadata.width, probe.metadata.height), (64, 48));
    }

    #[test]
    fn incomplete_tail_is_preserved_while_playable_segment_is_published() {
        let Some((_base, recovery, id)) = fixture(RecordingKind::Gif) else {
            return;
        };
        let store = DraftStore::new(recovery.root.clone());
        let mut manifest = store.load(&id).unwrap();
        manifest.segments[1].complete = false;
        store.save(&manifest).unwrap();
        let tail = recovery
            .root
            .join(&id)
            .join(&manifest.segments[1].relative_path);
        fs::write(&tail, b"interrupted partial mp4").unwrap();
        let accepted = identity(&recovery, &id);
        let saved = recovery
            .recover(&id, &accepted, &CancelToken::default(), |_| {})
            .unwrap();
        assert_eq!(saved.entry.kind, ArtifactKind::Gif);
        assert_eq!(saved.entry.dropped_frames, 0);
        assert_eq!(fs::read(&tail).unwrap(), b"interrupted partial mp4");
        assert!(saved.path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_active_probe_and_assembly_without_publication() {
        use std::{
            os::unix::fs::PermissionsExt,
            sync::mpsc,
            time::{Duration, Instant},
        };
        for probe_phase in [true, false] {
            let Some((base, mut recovery, id)) = fixture(RecordingKind::Video) else {
                return;
            };
            let accepted = identity(&recovery, &id);
            let bundle = recovery.root.join(&id);
            let history = recovery.history_root.clone();
            let marker = base.path().join("entered-child");
            let wrapper = base.path().join("media-wrapper.sh");
            let real = if probe_phase {
                test_ffprobe()
            } else {
                test_ffmpeg()
            };
            let real = real.to_string_lossy().replace('\'', "'\\''");
            let marker_name = marker.to_string_lossy().replace('\'', "'\\''");
            let match_name = if probe_phase {
                "*segment-000.mp4*"
            } else {
                "*assembled.mp4*"
            };
            fs::write(&wrapper, format!("#!/bin/sh\nfor arg do\n case \"$arg\" in {match_name}) : > '{marker_name}'; for n in 1 2 3 4 5 6 7 8 9 10; do sleep 1; done; exit 42;; esac\ndone\nexec '{real}' \"$@\"\n")).unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
            if probe_phase {
                recovery.tools = MediaToolchain::new(test_ffmpeg().into(), wrapper);
            } else {
                recovery.tools = MediaToolchain::new(wrapper, test_ffprobe().into());
            }
            let cancel = CancelToken::default();
            let worker_cancel = cancel.clone();
            let (tx, rx) = mpsc::channel();
            let worker_id = id.clone();
            let worker_identity = accepted.clone();
            let worker = std::thread::spawn(move || {
                let result = recovery.recover(&worker_id, &worker_identity, &worker_cancel, |_| {});
                tx.send(result).unwrap();
            });
            let start = Instant::now();
            while !marker.exists() && start.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(20));
            }
            cancel.cancel();
            let result = rx
                .recv_timeout(Duration::from_secs(12))
                .expect("recovery worker must terminate");
            worker.join().unwrap();
            assert!(
                marker.exists(),
                "child did not enter gated phase: {result:?}"
            );
            assert!(result.is_err());
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "cancellation did not promptly kill child"
            );
            let listing =
                RecordingRecovery::new(history.clone(), MediaToolchain::from_command_names());
            assert_eq!(identity(&listing, &id), accepted);
            assert!(bundle.is_dir());
            assert!(!history.join(&id).exists());
            assert!(!fs::read_dir(bundle).unwrap().any(|item| {
                item.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".recovery-")
            }));
        }
    }

    #[test]
    fn refuses_stale_foreign_and_colliding_bundles_without_deleting_sources() {
        let Some((base, recovery, id)) = fixture(RecordingKind::Video) else {
            return;
        };
        let old = identity(&recovery, &id);
        let bundle = recovery.root.join(&id);
        fs::write(bundle.join("segment-000.mp4"), b"changed").unwrap();
        assert!(recovery.discard(&id, &old).is_err());
        assert!(
            recovery
                .recover(&id, &old, &CancelToken::default(), |_| {})
                .is_err()
        );
        assert!(bundle.is_dir());
        let fresh = identity(&recovery, &id);
        let collision = recovery.history_root.join(&id);
        fs::create_dir_all(&collision).unwrap();
        fs::write(
            collision.join(captures_history::HISTORY_METADATA_FILE),
            b"foreign",
        )
        .unwrap();
        assert!(
            recovery
                .recover(&id, &fresh, &CancelToken::default(), |_| {})
                .is_err()
        );
        assert_eq!(
            fs::read(collision.join(captures_history::HISTORY_METADATA_FILE)).unwrap(),
            b"foreign"
        );
        let unrelated = base.path().join("unrelated");
        fs::write(&unrelated, b"keep").unwrap();
        assert!(recovery.discard("../unrelated", &fresh).is_err());
        assert_eq!(fs::read(unrelated).unwrap(), b"keep");
    }

    #[test]
    fn partial_discard_failure_restores_visible_bundle_for_manual_recovery() {
        let Some((_base, recovery, id)) = fixture(RecordingKind::Video) else {
            return;
        };
        let accepted = identity(&recovery, &id);
        let bundle = recovery.root.join(&id);
        let failure = recovery.discard_with(&id, &accepted, |quarantine| {
            fs::remove_file(quarantine.join("segment-001.mp4"))?;
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected removal failure",
            ))
        });
        assert!(failure.unwrap_err().contains("injected removal failure"));
        assert!(bundle.join("manifest.json").is_file());
        assert!(bundle.join("segment-000.mp4").is_file());
        assert_eq!(
            recovery
                .list()
                .unwrap()
                .iter()
                .filter(|draft| draft.session_id == id)
                .count(),
            1
        );
        assert!(!fs::read_dir(&recovery.root).unwrap().any(|item| {
            item.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".discard-")
        }));
    }

    #[test]
    fn changed_history_metadata_cannot_be_claimed_as_previous_publication() {
        let Some((_base, recovery, id)) = fixture(RecordingKind::Gif) else {
            return;
        };
        let accepted = identity(&recovery, &id);
        let saved = recovery
            .recover(&id, &accepted, &CancelToken::default(), |_| {})
            .unwrap();
        let metadata = recovery
            .history_root
            .join(&id)
            .join(captures_history::HISTORY_METADATA_FILE);
        let mut entry: HistoryEntry =
            serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        entry.has_system_audio = true;
        fs::write(&metadata, serde_json::to_vec_pretty(&entry).unwrap()).unwrap();
        let before = fs::read(&metadata).unwrap();
        assert!(
            recovery
                .recover(&id, &accepted, &CancelToken::default(), |_| {})
                .is_err()
        );
        assert_eq!(fs::read(metadata).unwrap(), before);
        assert!(saved.path.is_file());
        assert!(recovery.root.join(id).is_dir());
    }

    #[test]
    fn publication_retry_finishes_cleanup_without_second_history_entry() {
        let Some((_base, recovery, id)) = fixture(RecordingKind::Gif) else {
            return;
        };
        let store = DraftStore::new(recovery.root.clone());
        let interrupted = store.load(&id).unwrap();
        let accepted = identity(&recovery, &id);
        let saved = recovery
            .recover(&id, &accepted, &CancelToken::default(), |_| {})
            .unwrap();
        let media = fs::read(&saved.path).unwrap();
        // Simulate a process exit after History publication but before the
        // recovery manifest's Ready transition was persisted.
        store.save(&interrupted).unwrap();
        let retry = recovery
            .recover(&id, &accepted, &CancelToken::default(), |_| {})
            .unwrap();
        assert_eq!(retry.status, "already_recovered");
        assert_eq!(retry.entry.id, saved.entry.id);
        assert_eq!(fs::read(retry.path).unwrap(), media);
        assert_eq!(store.load(&id).unwrap().state, RecordingState::Ready);
        assert_eq!(fs::read_dir(&recovery.history_root).unwrap().count(), 1);
    }

    #[test]
    fn cleanup_failure_is_success_warning_and_retry_does_not_publish_twice() {
        let Some((_base, recovery, id)) = fixture(RecordingKind::Video) else {
            return;
        };
        let accepted = identity(&recovery, &id);
        let saved = recovery
            .recover_with_cleanup(
                &id,
                &accepted,
                &CancelToken::default(),
                |_| {},
                |_store, _id| Err("injected post-publication cleanup failure".into()),
            )
            .unwrap();
        assert_eq!(saved.status, "recovered");
        assert!(
            saved
                .warning
                .as_deref()
                .unwrap()
                .contains("cleanup failure")
        );
        assert!(saved.path.is_file());
        assert!(recovery.root.join(&id).is_dir());
        let media = fs::read(&saved.path).unwrap();
        let retry = recovery
            .recover(&id, &accepted, &CancelToken::default(), |_| {})
            .unwrap();
        assert_eq!(retry.status, "already_recovered");
        assert_eq!(retry.entry.id, saved.entry.id);
        assert_eq!(fs::read(retry.path).unwrap(), media);
        assert!(!recovery.root.join(&id).exists());
        assert_eq!(fs::read_dir(&recovery.history_root).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn lists_corrupt_and_linked_bundles_as_unavailable_without_touching_targets() {
        use std::os::unix::fs::symlink;
        let Some((base, recovery, id)) = fixture(RecordingKind::Video) else {
            return;
        };
        let external = base.path().join("outside");
        fs::write(&external, b"keep").unwrap();
        let bundle = recovery.root.join(&id);
        let token = identity(&recovery, &id);
        symlink(&external, bundle.join("foreign-link")).unwrap();
        let linked = recovery
            .list()
            .unwrap()
            .into_iter()
            .find(|row| row.session_id == id)
            .unwrap();
        assert_eq!(linked.status, "unavailable");
        assert!(recovery.discard(&id, &token).is_err());
        fs::write(bundle.join("manifest.json"), b"corrupt").unwrap();
        let row = recovery
            .list()
            .unwrap()
            .into_iter()
            .find(|row| row.session_id == id)
            .unwrap();
        assert_eq!(row.status, "unavailable");
        assert!(row.identity.is_none());
        assert!(recovery.discard(&id, &token).is_err());
        assert_eq!(fs::read(external).unwrap(), b"keep");
    }

    #[cfg(windows)]
    #[test]
    fn windows_junction_bundle_is_unavailable_and_never_discarded() {
        let Some((base, recovery, id)) = fixture(RecordingKind::Video) else {
            return;
        };
        let bundle = recovery.root.join(&id);
        let accepted = identity(&recovery, &id);
        let external = base.path().join("external-owned-bundle");
        fs::rename(&bundle, &external).unwrap();
        assert!(
            Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&bundle)
                .arg(&external)
                .status()
                .unwrap()
                .success()
        );
        let row = recovery
            .list()
            .unwrap()
            .into_iter()
            .find(|row| row.session_id == id)
            .unwrap();
        assert_eq!(row.status, "unavailable");
        assert!(recovery.discard(&id, &accepted).is_err());
        assert!(external.join("manifest.json").is_file());
    }
}
