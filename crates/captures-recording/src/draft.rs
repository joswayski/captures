use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{RecordingKind, RecordingOptions, RecordingSegmentInfo, RecordingState};

const MANIFEST_FILE: &str = "manifest.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingSegmentManifest {
    pub index: u32,
    pub relative_path: String,
    #[serde(default)]
    pub system_audio_relative_path: Option<String>,
    #[serde(default)]
    pub system_audio_offset_ms: i64,
    #[serde(default)]
    pub system_audio_warning: Option<String>,
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

    pub fn complete_segment(
        &mut self,
        session_directory: &Path,
        info: RecordingSegmentInfo,
        started_at_ms: u64,
        updated_at_ms: u64,
    ) -> Result<(), DraftSegmentError> {
        let relative_path = info
            .path
            .strip_prefix(session_directory)
            .map_err(|_| DraftSegmentError::VideoOutsideBundle)?
            .to_string_lossy()
            .into_owned();
        let microphone_relative_path = info
            .microphone_path
            .as_ref()
            .map(|path| {
                path.strip_prefix(session_directory)
                    .map(|relative| relative.to_string_lossy().into_owned())
                    .map_err(|_| DraftSegmentError::MicrophoneOutsideBundle)
            })
            .transpose()?;
        let system_audio_relative_path = info
            .system_audio_path
            .as_ref()
            .map(|path| {
                path.strip_prefix(session_directory)
                    .map(|relative| relative.to_string_lossy().into_owned())
                    .map_err(|_| DraftSegmentError::SystemAudioOutsideBundle)
            })
            .transpose()?;
        let segment = RecordingSegmentManifest {
            index: u32::try_from(self.segments.len())
                .map_err(|_| DraftSegmentError::TooManySegments)?,
            relative_path,
            system_audio_relative_path,
            system_audio_offset_ms: info.system_audio_offset_ms,
            system_audio_warning: info.system_audio_warning,
            microphone_relative_path,
            microphone_offset_ms: info.microphone_offset_ms,
            microphone_warning: info.microphone_warning,
            started_at_ms,
            duration_ms: info.duration_ms,
            width: info.width,
            height: info.height,
            size_bytes: info.size_bytes,
            dropped_frames: info.dropped_frames,
            complete: true,
        };
        if let Some(pending) = self
            .segments
            .iter_mut()
            .rev()
            .find(|pending| !pending.complete && pending.relative_path == segment.relative_path)
        {
            let index = pending.index;
            *pending = RecordingSegmentManifest { index, ..segment };
        } else {
            self.segments.push(segment);
        }
        self.updated_at_ms = updated_at_ms;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DraftSegmentError {
    #[error("recording segment escaped its recovery bundle")]
    VideoOutsideBundle,
    #[error("microphone segment escaped its recovery bundle")]
    MicrophoneOutsideBundle,
    #[error("desktop audio segment escaped its recovery bundle")]
    SystemAudioOutsideBundle,
    #[error("recording has too many segments")]
    TooManySegments,
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
    use std::path::Path;

    use tempfile::tempdir;
    use uuid::Uuid;

    use crate::{
        AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingKind, RecordingOptions,
        RecordingState, RecordingTarget,
    };

    use super::{
        DraftSegmentError, DraftStore, RecordingDraftManifest, RecordingSegmentManifest,
        RecoveryError,
    };
    use crate::RecordingSegmentInfo;

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

    fn pending_segment(index: u32, relative_path: &str) -> RecordingSegmentManifest {
        RecordingSegmentManifest {
            index,
            relative_path: relative_path.to_owned(),
            system_audio_relative_path: None,
            system_audio_offset_ms: 0,
            system_audio_warning: None,
            microphone_relative_path: None,
            microphone_offset_ms: 0,
            microphone_warning: None,
            started_at_ms: 1,
            duration_ms: 0,
            width: 640,
            height: 360,
            size_bytes: 0,
            dropped_frames: 0,
            complete: false,
        }
    }

    fn segment_info(directory: &Path) -> RecordingSegmentInfo {
        RecordingSegmentInfo {
            path: directory.join("segment-001.mp4"),
            system_audio_path: Some(directory.join("segment-001.system.wav")),
            system_audio_offset_ms: -17,
            system_audio_warning: Some("desktop audio drift".to_owned()),
            microphone_path: Some(directory.join("segment-001.mic.wav")),
            microphone_offset_ms: 23,
            microphone_warning: Some("microphone started late".to_owned()),
            width: 1_920,
            height: 1_080,
            duration_ms: 4_321,
            size_bytes: 98_765,
            dropped_frames: 12,
        }
    }

    #[test]
    fn completes_the_last_matching_pending_segment_without_reordering() {
        let directory = tempdir().expect("temporary directory");
        let mut manifest = RecordingDraftManifest::new(
            Uuid::new_v4().to_string(),
            options(RecordingKind::Video),
            10,
        );
        manifest.segments = vec![
            pending_segment(4, "segment-001.mp4"),
            pending_segment(9, "segment-001.mp4"),
        ];

        manifest
            .complete_segment(directory.path(), segment_info(directory.path()), 123, 456)
            .expect("segment completed");

        assert_eq!(manifest.segments.len(), 2);
        assert_eq!(manifest.segments[0], pending_segment(4, "segment-001.mp4"));
        assert_eq!(
            manifest.segments[1],
            RecordingSegmentManifest {
                index: 9,
                relative_path: "segment-001.mp4".to_owned(),
                system_audio_relative_path: Some("segment-001.system.wav".to_owned()),
                system_audio_offset_ms: -17,
                system_audio_warning: Some("desktop audio drift".to_owned()),
                microphone_relative_path: Some("segment-001.mic.wav".to_owned()),
                microphone_offset_ms: 23,
                microphone_warning: Some("microphone started late".to_owned()),
                started_at_ms: 123,
                duration_ms: 4_321,
                width: 1_920,
                height: 1_080,
                size_bytes: 98_765,
                dropped_frames: 12,
                complete: true,
            }
        );
        assert_eq!(manifest.updated_at_ms, 456);
    }

    #[test]
    fn appends_a_nonmatching_segment_at_the_current_length() {
        let directory = tempdir().expect("temporary directory");
        let mut manifest = RecordingDraftManifest::new(
            Uuid::new_v4().to_string(),
            options(RecordingKind::Video),
            10,
        );
        manifest.segments = vec![pending_segment(27, "segment-000.mp4")];

        manifest
            .complete_segment(directory.path(), segment_info(directory.path()), 123, 456)
            .expect("segment appended");

        assert_eq!(manifest.segments.len(), 2);
        assert_eq!(manifest.segments[0], pending_segment(27, "segment-000.mp4"));
        assert_eq!(manifest.segments[1].index, 1);
        assert_eq!(manifest.segments[1].relative_path, "segment-001.mp4");
        assert!(manifest.segments[1].complete);
        assert_eq!(manifest.updated_at_ms, 456);
    }

    #[test]
    fn out_of_bundle_segment_paths_leave_the_manifest_unchanged() {
        let directory = tempdir().expect("temporary directory");
        let outside = tempdir().expect("outside directory");
        let manifest = RecordingDraftManifest::new(
            Uuid::new_v4().to_string(),
            options(RecordingKind::Video),
            10,
        );
        let cases = [
            (
                DraftSegmentError::VideoOutsideBundle,
                "recording segment escaped its recovery bundle",
                RecordingSegmentInfo {
                    path: outside.path().join("segment.mp4"),
                    ..segment_info(directory.path())
                },
            ),
            (
                DraftSegmentError::MicrophoneOutsideBundle,
                "microphone segment escaped its recovery bundle",
                RecordingSegmentInfo {
                    microphone_path: Some(outside.path().join("segment.mic.wav")),
                    ..segment_info(directory.path())
                },
            ),
            (
                DraftSegmentError::SystemAudioOutsideBundle,
                "desktop audio segment escaped its recovery bundle",
                RecordingSegmentInfo {
                    system_audio_path: Some(outside.path().join("segment.system.wav")),
                    ..segment_info(directory.path())
                },
            ),
        ];

        for (expected, message, info) in cases {
            let mut actual = manifest.clone();
            let error = actual
                .complete_segment(directory.path(), info, 123, 456)
                .expect_err("outside path rejected");
            assert_eq!(error, expected);
            assert_eq!(error.to_string(), message);
            assert_eq!(actual, manifest);
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
            system_audio_relative_path: Some("segment-000.system.wav".to_owned()),
            system_audio_offset_ms: 4,
            system_audio_warning: None,
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
