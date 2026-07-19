use std::collections::HashMap;
use std::sync::Arc;

use captures_capture::{DisplayDescriptor, WindowDescriptor, XcapBackend};
use parking_lot::{Mutex, RwLock};
use uuid::Uuid;

use crate::{
    models::{AppSettings, CaptureArtifact, CaptureSession},
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

#[derive(Default)]
pub struct ClipboardOwnership {
    revision: Option<isize>,
    artifact_id: Option<String>,
}

impl ClipboardOwnership {
    pub fn record(&mut self, revision: isize, artifact_id: String) {
        self.revision = Some(revision);
        self.artifact_id = Some(artifact_id);
    }

    pub fn current_artifact(&mut self, revision: isize) -> Option<String> {
        if self.revision != Some(revision) {
            self.revision = None;
            self.artifact_id = None;
        }
        self.artifact_id.clone()
    }
}

pub struct AppState {
    pub settings: RwLock<AppSettings>,
    pub sessions: Mutex<HashMap<Uuid, CaptureSession>>,
    pub artifacts: Mutex<Vec<CaptureArtifact>>,
    pub clipboard_ownership: Mutex<ClipboardOwnership>,
    pub thumbnail_visibility: Mutex<ThumbnailVisibility>,
    pub screen_permission_requested_this_launch: Mutex<bool>,
    pub backend: XcapBackend,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            settings: RwLock::new(storage::load_settings()),
            sessions: Mutex::new(HashMap::new()),
            artifacts: Mutex::new(Vec::new()),
            clipboard_ownership: Mutex::new(ClipboardOwnership::default()),
            thumbnail_visibility: Mutex::new(ThumbnailVisibility::default()),
            screen_permission_requested_this_launch: Mutex::new(false),
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
    use super::{ClipboardOwnership, ThumbnailVisibility};

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

        ownership.record(41, "first".to_owned());
        assert_eq!(ownership.current_artifact(41).as_deref(), Some("first"));

        ownership.record(42, "second".to_owned());
        assert_eq!(ownership.current_artifact(42).as_deref(), Some("second"));
        assert!(ownership.current_artifact(43).is_none());
        assert!(ownership.current_artifact(42).is_none());
    }
}
