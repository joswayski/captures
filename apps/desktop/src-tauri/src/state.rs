use std::collections::HashMap;
use std::sync::{Arc, atomic::AtomicBool};
#[cfg(any(target_os = "linux", test))]
use std::time::{Duration, Instant};

use captures_capture::{DisplayDescriptor, WindowDescriptor, XcapBackend};
use parking_lot::{Mutex, RwLock};
use uuid::Uuid;

use crate::{
    models::{
        AppSettings, CaptureArtifact, CaptureSession, HistoryEntry, RecordingArtifactData,
        RecordingSelection,
    },
    recording::RecordingRuntime,
    storage,
};

#[derive(Default)]
pub struct ThumbnailVisibility {
    suppressed: bool,
    pending_artifact_id: Option<String>,
}

impl ThumbnailVisibility {
    pub fn begin_capture(&mut self) -> bool {
        if self.suppressed && self.pending_artifact_id.is_none() {
            return false;
        }
        self.suppressed = true;
        self.pending_artifact_id = None;
        true
    }

    pub fn wait_for_artifact(&mut self, artifact_id: String) {
        self.suppressed = true;
        self.pending_artifact_id = Some(artifact_id);
    }

    pub fn mark_artifact_ready(&mut self, artifact_id: &str) -> bool {
        if self.pending_artifact_id.as_deref() != Some(artifact_id) {
            return false;
        }
        self.pending_artifact_id = None;
        self.suppressed = false;
        true
    }

    pub fn restore(&mut self) {
        self.suppressed = false;
        self.pending_artifact_id = None;
    }

    pub fn is_suppressed(&self) -> bool {
        self.suppressed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClipboardFingerprint {
    pub width: u32,
    pub height: u32,
    pub checksum: u64,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardVerification {
    pub revision: isize,
    pub fingerprint: ClipboardFingerprint,
}

#[derive(Default)]
pub struct ClipboardOwnership {
    revision: Option<isize>,
    artifact_id: Option<String>,
    fingerprint: Option<ClipboardFingerprint>,
    #[cfg(any(target_os = "linux", test))]
    last_verification: Option<Instant>,
}

impl ClipboardOwnership {
    pub fn record(
        &mut self,
        revision: isize,
        artifact_id: String,
        fingerprint: ClipboardFingerprint,
    ) {
        self.revision = Some(revision);
        self.artifact_id = Some(artifact_id);
        self.fingerprint = Some(fingerprint);
        #[cfg(any(target_os = "linux", test))]
        {
            self.last_verification = None;
        }
    }

    pub fn current_artifact(&mut self, revision: isize) -> Option<String> {
        if self.revision != Some(revision) {
            self.clear();
        }
        self.artifact_id.clone()
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn verification(
        &mut self,
        now: Instant,
        minimum_interval: Duration,
    ) -> Option<ClipboardVerification> {
        if self
            .last_verification
            .is_some_and(|last| now.saturating_duration_since(last) < minimum_interval)
        {
            return None;
        }
        let verification = ClipboardVerification {
            revision: self.revision?,
            fingerprint: self.fingerprint?,
        };
        self.last_verification = Some(now);
        Some(verification)
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn clear_if_revision(&mut self, revision: isize) -> bool {
        if self.revision != Some(revision) {
            return false;
        }
        self.clear();
        true
    }

    fn clear(&mut self) {
        self.revision = None;
        self.artifact_id = None;
        self.fingerprint = None;
        #[cfg(any(target_os = "linux", test))]
        {
            self.last_verification = None;
        }
    }
}

#[derive(Default)]
pub struct ScreenshotCountdownRuntime {
    pub generation: u64,
    pub active: bool,
}

pub struct AppState {
    pub settings: RwLock<AppSettings>,
    pub sessions: Mutex<HashMap<Uuid, CaptureSession>>,
    pub artifacts: Mutex<Vec<CaptureArtifact>>,
    pub recording_artifacts: Mutex<Vec<RecordingArtifactData>>,
    pub recording_timeline_sprites: Mutex<HashMap<String, Vec<u8>>>,
    pub recording_selection: Mutex<Option<RecordingSelection>>,
    pub recording: Mutex<RecordingRuntime>,
    pub screenshot_countdown: Mutex<ScreenshotCountdownRuntime>,
    pub history: Mutex<Vec<HistoryEntry>>,
    pub clipboard_ownership: Mutex<ClipboardOwnership>,
    pub thumbnail_visibility: Mutex<ThumbnailVisibility>,
    pub screen_permission_requested_this_launch: Mutex<bool>,
    pub shortcut_capture_suppressed: AtomicBool,
    pub backend: XcapBackend,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        if let Err(error) = storage::clear_drag_exports() {
            eprintln!("failed to clear temporary drag exports: {error}");
        }
        let history = storage::load_capture_history().unwrap_or_else(|error| {
            eprintln!("failed to load capture history: {error}");
            Vec::new()
        });
        let recording_artifacts = history
            .iter()
            .filter_map(storage::load_recording_artifact)
            .collect();
        Arc::new(Self {
            settings: RwLock::new(storage::load_settings()),
            sessions: Mutex::new(HashMap::new()),
            artifacts: Mutex::new(Vec::new()),
            recording_artifacts: Mutex::new(recording_artifacts),
            recording_timeline_sprites: Mutex::new(HashMap::new()),
            recording_selection: Mutex::new(None),
            recording: Mutex::new(RecordingRuntime::default()),
            screenshot_countdown: Mutex::new(ScreenshotCountdownRuntime::default()),
            history: Mutex::new(history),
            clipboard_ownership: Mutex::new(ClipboardOwnership::default()),
            thumbnail_visibility: Mutex::new(ThumbnailVisibility::default()),
            screen_permission_requested_this_launch: Mutex::new(false),
            shortcut_capture_suppressed: AtomicBool::new(false),
            backend: XcapBackend,
        })
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.read().clone()
    }

    pub fn monitors(&self) -> Result<Vec<DisplayDescriptor>, crate::AppError> {
        self.backend.displays().map_err(Into::into)
    }

    pub fn windows(&self) -> Result<Vec<WindowDescriptor>, crate::AppError> {
        self.backend.windows().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{ClipboardFingerprint, ClipboardOwnership, ThumbnailVisibility};

    const FINGERPRINT: ClipboardFingerprint = ClipboardFingerprint {
        width: 2,
        height: 3,
        checksum: 41,
    };

    #[test]
    fn blocks_overlapping_capture_preparation() {
        let mut visibility = ThumbnailVisibility::default();

        assert!(visibility.begin_capture());
        assert!(!visibility.begin_capture());
        assert!(visibility.is_suppressed());

        visibility.restore();
        assert!(!visibility.is_suppressed());
        assert!(visibility.begin_capture());
    }

    #[test]
    fn ignores_a_stale_image_ready_event_after_the_next_capture_starts() {
        let mut visibility = ThumbnailVisibility::default();

        assert!(visibility.begin_capture());
        visibility.wait_for_artifact("first".to_owned());
        assert!(visibility.begin_capture());
        visibility.wait_for_artifact("second".to_owned());

        assert!(!visibility.mark_artifact_ready("first"));
        assert!(visibility.is_suppressed());
        assert!(visibility.mark_artifact_ready("second"));
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn clipboard_ownership_tracks_one_artifact_until_the_pasteboard_changes() {
        let mut ownership = ClipboardOwnership::default();

        ownership.record(41, "first".to_owned(), FINGERPRINT);
        assert_eq!(ownership.current_artifact(41).as_deref(), Some("first"));

        ownership.record(42, "second".to_owned(), FINGERPRINT);
        assert_eq!(ownership.current_artifact(42).as_deref(), Some("second"));
        assert!(ownership.current_artifact(43).is_none());
        assert!(ownership.current_artifact(42).is_none());
    }

    #[test]
    fn clipboard_verification_is_throttled_and_cannot_clear_a_newer_copy() {
        let mut ownership = ClipboardOwnership::default();
        let now = Instant::now();
        ownership.record(41, "first".to_owned(), FINGERPRINT);

        assert_eq!(
            ownership.verification(now, Duration::from_secs(1)),
            Some(super::ClipboardVerification {
                revision: 41,
                fingerprint: FINGERPRINT,
            })
        );
        assert!(
            ownership
                .verification(now + Duration::from_millis(500), Duration::from_secs(1))
                .is_none()
        );

        ownership.record(42, "second".to_owned(), FINGERPRINT);
        assert!(!ownership.clear_if_revision(41));
        assert_eq!(ownership.current_artifact(42).as_deref(), Some("second"));
        assert!(ownership.clear_if_revision(42));
        assert!(ownership.current_artifact(42).is_none());
    }
}
