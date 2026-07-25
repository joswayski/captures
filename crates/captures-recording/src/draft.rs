use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{RecordingKind, RecordingOptions, RecordingState};

const MANIFEST_FILE: &str = "manifest.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingSegmentManifest {
    pub index: u32,
    pub relative_path: String,
    #[serde(default)]
    pub microphone_relative_path: Option<String>,
    #[serde(default)]
    pub microphone_offset_ms: i64,
    #[serde(default)]
    pub microphone_warning: Option<String>,
    pub started_at_ms: u64,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    #[serde(default)]
    pub dropped_frames: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingDraftManifest {
    pub schema_version: u16,
    pub session_id: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub state: RecordingState,
    pub options: RecordingOptions,
    #[serde(default)]
    pub segments: Vec<RecordingSegmentManifest>,
    #[serde(default)]
    pub final_path: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl RecordingDraftManifest {
    pub fn new(session_id: String, options: RecordingOptions, created_at_ms: u64) -> Self {
        Self {
            schema_version: 1,
            session_id,
            created_at_ms,
            updated_at_ms: created_at_ms,
            state: RecordingState::Selecting,
            options,
            segments: Vec::new(),
            final_path: None,
            last_error: None,
        }
    }

    pub fn retains_source_master(&self) -> bool {
        self.options.kind == RecordingKind::Gif
    }
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("recording recovery session ID is invalid")]
    InvalidSessionId,
    #[error("recording recovery manifest does not match its directory")]
    MismatchedSession,
}

#[derive(Clone, Debug)]
pub struct DraftStore {
    root: PathBuf,
}

impl DraftStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn session_directory(&self, session_id: &str) -> Result<PathBuf, RecoveryError> {
        validate_session_id(session_id)?;
        Ok(self.root.join(session_id))
    }

    pub fn create(&self, manifest: &RecordingDraftManifest) -> Result<PathBuf, RecoveryError> {
        let directory = self.session_directory(&manifest.session_id)?;
        fs::create_dir_all(&directory)?;
        self.save(manifest)?;
        Ok(directory)
    }

    pub fn save(&self, manifest: &RecordingDraftManifest) -> Result<(), RecoveryError> {
        let directory = self.session_directory(&manifest.session_id)?;
        fs::create_dir_all(&directory)?;
        let bytes = serde_json::to_vec_pretty(manifest)?;
        let mut temporary = tempfile_path(&directory);
        while temporary.exists() {
            temporary = tempfile_path(&directory);
        }
        let result = (|| {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, directory.join(MANIFEST_FILE))?;
            Ok::<(), RecoveryError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn load(&self, session_id: &str) -> Result<RecordingDraftManifest, RecoveryError> {
        let directory = self.session_directory(session_id)?;
        let manifest: RecordingDraftManifest =
            serde_json::from_slice(&fs::read(directory.join(MANIFEST_FILE))?)?;
        if manifest.session_id != session_id {
            return Err(RecoveryError::MismatchedSession);
        }
        Ok(manifest)
    }

    pub fn list(&self) -> Result<Vec<RecordingDraftManifest>, RecoveryError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut manifests = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let id = entry.file_name().to_string_lossy().into_owned();
                self.load(&id).ok()
            })
            .collect::<Vec<_>>();
        manifests.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        Ok(manifests)
    }

    pub fn remove(&self, session_id: &str) -> Result<(), RecoveryError> {
        let directory = self.session_directory(session_id)?;
        match fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn prune_gif_sources(
        &self,
        now_ms: u64,
        retention_ms: u64,
    ) -> Result<Vec<String>, RecoveryError> {
        let mut removed = Vec::new();
        for manifest in self.list()? {
            let expired = now_ms.saturating_sub(manifest.updated_at_ms) > retention_ms;
            if expired && manifest.retains_source_master() && manifest.state.is_terminal() {
                self.remove(&manifest.session_id)?;
                removed.push(manifest.session_id);
            }
        }
        Ok(removed)
    }
}

fn validate_session_id(session_id: &str) -> Result<(), RecoveryError> {
    Uuid::parse_str(session_id)
        .map(|_| ())
        .map_err(|_| RecoveryError::InvalidSessionId)
}

fn tempfile_path(directory: &Path) -> PathBuf {
    directory.join(format!(".{MANIFEST_FILE}.{}.tmp", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use uuid::Uuid;

    use crate::{
        AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingKind, RecordingOptions,
        RecordingState, RecordingTarget,
    };

    use super::{DraftStore, RecordingDraftManifest, RecordingSegmentManifest, RecoveryError};

    fn options(kind: RecordingKind) -> RecordingOptions {
        RecordingOptions {
            kind,
            target: RecordingTarget::Region {
                display_id: "1".to_owned(),
                rect: CaptureRect {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 600,
                },
            },
            frames_per_second: 30,
            max_resolution: MaxResolution::P1080,
            countdown_seconds: 3,
            show_cursor: true,
            highlight_clicks: false,
            show_keystrokes: false,
            audio: AudioOptions::default(),
            gif: GifOptions::default(),
        }
    }

    #[test]
    fn atomically_round_trips_and_orders_manifests() {
        let directory = tempdir().expect("temporary directory");
        let store = DraftStore::new(directory.path().to_path_buf());
        let first_id = Uuid::new_v4().to_string();
        let second_id = Uuid::new_v4().to_string();
        let mut first =
            RecordingDraftManifest::new(first_id.clone(), options(RecordingKind::Video), 10);
        first.updated_at_ms = 20;
        first.segments.push(RecordingSegmentManifest {
            index: 0,
            relative_path: "segment-000.mp4".to_owned(),
            microphone_relative_path: Some("segment-000.mic.wav".to_owned()),
            microphone_offset_ms: 12,
            microphone_warning: None,
            started_at_ms: 10,
            duration_ms: 1_000,
            width: 800,
            height: 600,
            size_bytes: 42,
            dropped_frames: 0,
            complete: true,
        });
        let second =
            RecordingDraftManifest::new(second_id.clone(), options(RecordingKind::Gif), 30);

        store.create(&first).expect("first manifest saved");
        store.create(&second).expect("second manifest saved");

        assert_eq!(store.load(&first_id).expect("first manifest loaded"), first);
        let listed = store.list().expect("manifests listed");
        assert_eq!(listed[0].session_id, second_id);
        assert_eq!(listed[1].session_id, first_id);
    }

    #[test]
    fn prunes_only_terminal_expired_gif_sources() {
        let directory = tempdir().expect("temporary directory");
        let store = DraftStore::new(directory.path().to_path_buf());
        let gif_id = Uuid::new_v4().to_string();
        let video_id = Uuid::new_v4().to_string();
        let mut gif = RecordingDraftManifest::new(gif_id.clone(), options(RecordingKind::Gif), 0);
        gif.state = RecordingState::Ready;
        let mut video =
            RecordingDraftManifest::new(video_id.clone(), options(RecordingKind::Video), 0);
        video.state = RecordingState::Ready;
        store.create(&gif).expect("gif saved");
        store.create(&video).expect("video saved");

        assert_eq!(
            store.prune_gif_sources(101, 100).expect("pruned"),
            vec![gif_id]
        );
        assert!(store.load(&video_id).is_ok());
    }

    #[test]
    fn rejects_paths_that_are_not_session_ids() {
        let directory = tempdir().expect("temporary directory");
        let store = DraftStore::new(directory.path().to_path_buf());
        assert!(matches!(
            store.session_directory("../escape"),
            Err(RecoveryError::InvalidSessionId)
        ));
    }
}
