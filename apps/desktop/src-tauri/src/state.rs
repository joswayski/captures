use std::collections::HashMap;
use std::sync::Arc;

use ces_capture::{DisplayDescriptor, WindowDescriptor, XcapBackend};
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

pub struct AppState {
    pub settings: RwLock<AppSettings>,
    pub sessions: Mutex<HashMap<Uuid, CaptureSession>>,
    pub artifacts: Mutex<Vec<CaptureArtifact>>,
    pub thumbnail_visibility: Mutex<ThumbnailVisibility>,
    pub backend: XcapBackend,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            settings: RwLock::new(storage::load_settings()),
            sessions: Mutex::new(HashMap::new()),
            artifacts: Mutex::new(Vec::new()),
            thumbnail_visibility: Mutex::new(ThumbnailVisibility::default()),
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
    use super::ThumbnailVisibility;

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
}
