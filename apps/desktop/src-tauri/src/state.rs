use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64},
};
#[cfg(any(target_os = "linux", test))]
use std::time::Duration;
use std::time::Instant;

use captures_capture::{DisplayDescriptor, WindowDescriptor, XcapBackend};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    models::{
        AppSettings, CaptureArtifact, CaptureSession, HistoryEntry, MiniPreviewPlacement,
        RecordingArtifactData, RecordingSelection,
    },
    recording::RecordingRuntime,
    storage,
};

/// Full-resolution file staged for a native OS file drag from a preview card.
#[derive(Clone, Debug)]
pub struct PreparedArtifactDrag {
    pub artifact_id: String,
    pub path: PathBuf,
    pub file_name: String,
}

/// Which edge of the visible pile stays put when the stack opens or closes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailStackAnchor {
    #[default]
    Bottom,
    Top,
}

impl ThumbnailStackAnchor {
    pub const fn is_top(self) -> bool {
        matches!(self, Self::Top)
    }
}

impl From<MiniPreviewPlacement> for ThumbnailStackAnchor {
    fn from(placement: MiniPreviewPlacement) -> Self {
        if placement.is_top() {
            Self::Top
        } else {
            Self::Bottom
        }
    }
}

/// Session-only: last user-dragged position of the mini-preview pile.
///
/// `edge` is the anchored edge of the visible pile in logical pixels: the
/// pile bottom when `anchor` is bottom, or the pile top when it is top.
#[derive(Clone, Copy, Debug)]
pub struct ThumbnailStackOrigin {
    pub x: f64,
    pub edge: f64,
    pub anchor: ThumbnailStackAnchor,
}

#[derive(Default)]
pub struct ThumbnailVisibility {
    next_capture_generation: u64,
    suppressed_capture_generation: Option<u64>,
    pending_artifact_id: Option<String>,
    capture_ui_suppressed: bool,
    /// Session-only: the user parked the stack behind the restore chip.
    user_collapsed: bool,
    stack_origin: Option<ThumbnailStackOrigin>,
}

impl ThumbnailVisibility {
    pub fn begin_capture(&mut self) -> Option<u64> {
        if self.suppressed_capture_generation.is_some() && self.pending_artifact_id.is_none() {
            return None;
        }
        self.next_capture_generation = self.next_capture_generation.wrapping_add(1);
        self.suppressed_capture_generation = Some(self.next_capture_generation);
        self.pending_artifact_id = None;
        Some(self.next_capture_generation)
    }

    pub fn wait_for_artifact(&mut self, capture_generation: u64, artifact_id: String) -> bool {
        if self.suppressed_capture_generation != Some(capture_generation) {
            return false;
        }
        self.pending_artifact_id = Some(artifact_id);
        true
    }

    pub fn mark_artifact_ready(&mut self, artifact_id: &str) -> bool {
        if self.pending_artifact_id.as_deref() != Some(artifact_id) {
            return false;
        }
        self.pending_artifact_id = None;
        self.suppressed_capture_generation = None;
        // Un-hide the stack so the new shot lands on the pile. Leave parking
        // alone: auto-expanding resized the window to the expanded bar while
        // the webview stayed collapsed, which pinned drag to that bar's top.
        true
    }

    pub fn restore_capture(&mut self, capture_generation: u64) -> bool {
        if self.suppressed_capture_generation != Some(capture_generation) {
            return false;
        }
        self.suppressed_capture_generation = None;
        self.pending_artifact_id = None;
        true
    }

    pub fn stop_waiting_for_artifact(&mut self) -> bool {
        if self.pending_artifact_id.is_none() {
            return false;
        }
        self.suppressed_capture_generation = None;
        self.pending_artifact_id = None;
        true
    }

    pub fn suppress_for_capture_ui(&mut self) {
        self.capture_ui_suppressed = true;
    }

    pub fn restore_capture_ui(&mut self) {
        self.capture_ui_suppressed = false;
    }

    pub fn collapse(&mut self) {
        self.user_collapsed = true;
    }

    pub fn expand(&mut self) {
        self.user_collapsed = false;
    }

    pub fn reset_session_placement(&mut self) {
        self.user_collapsed = false;
        self.stack_origin = None;
    }

    pub fn set_stack_origin(&mut self, origin: ThumbnailStackOrigin) {
        self.stack_origin = Some(origin);
    }

    pub fn stack_origin(&self) -> Option<ThumbnailStackOrigin> {
        self.stack_origin
    }

    pub fn clear_stack_origin(&mut self) {
        self.stack_origin = None;
    }

    pub fn is_collapsed(&self) -> bool {
        self.user_collapsed
    }

    pub fn is_suppressed(&self) -> bool {
        self.suppressed_capture_generation.is_some() || self.capture_ui_suppressed
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

    pub fn clear_if_artifact(&mut self, artifact_id: &str) -> bool {
        if self.artifact_id.as_deref() != Some(artifact_id) {
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
    pub thumbnail_capture_generation: Option<u64>,
    pub remaining_seconds: u8,
}

pub struct AppState {
    pub settings: RwLock<AppSettings>,
    pub sessions: Mutex<HashMap<Uuid, CaptureSession>>,
    pub artifacts: Mutex<Vec<CaptureArtifact>>,
    /// Latest folder export from each open screenshot editor, keyed by the
    /// editor window's capture id. Available for reveal, overwrite, and asset
    /// URLs, but not shown in the mini-preview stack. At most one full image is
    /// retained per editor; the slot is dropped when that window closes.
    pub editor_artifacts: Mutex<HashMap<String, CaptureArtifact>>,
    pub recording_artifacts: Mutex<Vec<RecordingArtifactData>>,
    pub recording_timeline_sprites: Mutex<HashMap<String, Vec<u8>>>,
    pub recording_selection: Mutex<Option<RecordingSelection>>,
    pub recording: Mutex<RecordingRuntime>,
    pub screenshot_countdown: Mutex<ScreenshotCountdownRuntime>,
    pub history: Mutex<Vec<HistoryEntry>>,
    pub clipboard_ownership: Mutex<ClipboardOwnership>,
    pub thumbnail_visibility: Mutex<ThumbnailVisibility>,
    /// Last full-resolution PNG prepared for a preview file drag.
    pub prepared_artifact_drag: Mutex<Option<PreparedArtifactDrag>>,
    /// Timestamp of the most recent in-app file drop (e.g. screenshot editor).
    pub last_internal_file_drop: Mutex<Option<Instant>>,
    pub screen_permission_requested_this_launch: Mutex<bool>,
    pub shortcut_capture_suppressed: AtomicBool,
    /// Invalidates stale recording-saved notice timers when the same reusable
    /// notice window is shown again.
    pub recording_saved_notice_generation: AtomicU64,
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
            editor_artifacts: Mutex::new(HashMap::new()),
            recording_artifacts: Mutex::new(recording_artifacts),
            recording_timeline_sprites: Mutex::new(HashMap::new()),
            recording_selection: Mutex::new(None),
            recording: Mutex::new(RecordingRuntime::default()),
            screenshot_countdown: Mutex::new(ScreenshotCountdownRuntime::default()),
            history: Mutex::new(history),
            clipboard_ownership: Mutex::new(ClipboardOwnership::default()),
            thumbnail_visibility: Mutex::new(ThumbnailVisibility::default()),
            prepared_artifact_drag: Mutex::new(None),
            last_internal_file_drop: Mutex::new(None),
            screen_permission_requested_this_launch: Mutex::new(false),
            shortcut_capture_suppressed: AtomicBool::new(false),
            recording_saved_notice_generation: AtomicU64::new(0),
            backend: XcapBackend,
        })
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.read().clone()
    }

    pub fn find_artifact(&self, id: &str) -> Option<CaptureArtifact> {
        self.artifacts
            .lock()
            .iter()
            .find(|artifact| artifact.id == id)
            .cloned()
            .or_else(|| {
                self.editor_artifacts
                    .lock()
                    .values()
                    .find(|artifact| artifact.id == id)
                    .cloned()
            })
    }

    /// Keep this editor's latest folder export without adding a mini preview.
    ///
    /// `source_id` is the capture the editor is saving from. Repeated "Save as
    /// new file" in the same window replaces the previous retained export.
    pub fn store_editor_artifact(&self, source_id: &str, artifact: CaptureArtifact) {
        let mut retained = self.editor_artifacts.lock();
        let owner_id = retained
            .iter()
            .find(|(_, entry)| entry.id == source_id)
            .map(|(owner, _)| owner.clone())
            .unwrap_or_else(|| source_id.to_owned());
        retained.insert(owner_id, artifact);
    }

    pub fn drop_editor_artifacts_for_owner(&self, owner_id: &str) {
        self.editor_artifacts.lock().remove(owner_id);
    }

    /// Drop retained exports whose owner or artifact id is in `ids`, except
    /// slots still needed by an open editor (`keep_owners`).
    pub fn forget_editor_artifacts_for_ids(&self, ids: &[String], keep_owners: &[String]) {
        self.editor_artifacts.lock().retain(|owner, entry| {
            keep_owners.iter().any(|keep| keep == owner)
                || !ids.iter().any(|id| id == owner || id == &entry.id)
        });
    }

    /// Replace a capture already on the mini-preview stack or retained from an
    /// editor export. Returns false when neither store has this id.
    pub fn replace_artifact(&self, artifact: CaptureArtifact) -> bool {
        {
            let mut artifacts = self.artifacts.lock();
            if let Some(existing) = artifacts
                .iter_mut()
                .find(|existing| existing.id == artifact.id)
            {
                *existing = artifact;
                return true;
            }
        }
        let mut editor_artifacts = self.editor_artifacts.lock();
        if let Some(existing) = editor_artifacts
            .values_mut()
            .find(|existing| existing.id == artifact.id)
        {
            *existing = artifact;
            return true;
        }
        false
    }

    pub fn monitors(&self) -> Result<Vec<DisplayDescriptor>, crate::AppError> {
        self.backend.displays().map_err(Into::into)
    }

    pub fn windows(&self) -> Result<Vec<WindowDescriptor>, crate::AppError> {
        let mut windows = self.backend.windows()?;
        crate::apply_native_window_frames(&mut windows);
        Ok(windows)
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

        let first = visibility
            .begin_capture()
            .expect("first capture should start");
        assert!(visibility.begin_capture().is_none());
        assert!(visibility.is_suppressed());

        assert!(visibility.restore_capture(first));
        assert!(!visibility.is_suppressed());
        assert!(visibility.begin_capture().is_some());
    }

    #[test]
    fn ignores_stale_restore_and_image_ready_events_after_the_next_capture_starts() {
        let mut visibility = ThumbnailVisibility::default();

        let first = visibility
            .begin_capture()
            .expect("first capture should start");
        assert!(visibility.wait_for_artifact(first, "first".to_owned()));
        let second = visibility
            .begin_capture()
            .expect("second capture should start");

        assert!(!visibility.restore_capture(first));
        assert!(visibility.is_suppressed());
        assert!(visibility.wait_for_artifact(second, "second".to_owned()));

        assert!(!visibility.mark_artifact_ready("first"));
        assert!(visibility.is_suppressed());
        assert!(visibility.mark_artifact_ready("second"));
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn disabling_previews_releases_only_an_artifact_wait() {
        let mut visibility = ThumbnailVisibility::default();

        let capture = visibility.begin_capture().expect("capture should start");
        assert!(!visibility.stop_waiting_for_artifact());
        assert!(visibility.is_suppressed());

        assert!(visibility.wait_for_artifact(capture, "artifact".to_owned()));
        assert!(visibility.stop_waiting_for_artifact());
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn capture_ui_suppression_stays_active_across_a_screenshot_preview() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.suppress_for_capture_ui();

        let capture = visibility.begin_capture().expect("capture should start");
        assert!(visibility.wait_for_artifact(capture, "artifact".to_owned()));
        assert!(visibility.mark_artifact_ready("artifact"));
        assert!(visibility.is_suppressed());

        visibility.restore_capture_ui();
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn stack_origin_survives_expand_and_clears_with_session_placement() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.set_stack_origin(super::ThumbnailStackOrigin {
            x: 120.0,
            edge: 640.0,
            anchor: super::ThumbnailStackAnchor::Bottom,
        });
        visibility.collapse();
        visibility.expand();
        assert!(!visibility.is_collapsed());
        assert_eq!(visibility.stack_origin().unwrap().x, 120.0);
        assert_eq!(visibility.stack_origin().unwrap().edge, 640.0);
        assert_eq!(
            visibility.stack_origin().unwrap().anchor,
            super::ThumbnailStackAnchor::Bottom
        );

        visibility.reset_session_placement();
        assert!(visibility.stack_origin().is_none());
        assert!(!visibility.is_collapsed());
    }

    #[test]
    fn a_new_preview_keeps_the_stack_collapsed() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.collapse();
        assert!(visibility.is_collapsed());

        visibility.expand();
        assert!(!visibility.is_collapsed());

        visibility.collapse();
        let capture = visibility.begin_capture().expect("capture should start");
        assert!(visibility.is_collapsed());
        assert!(visibility.wait_for_artifact(capture, "artifact".to_owned()));
        assert!(visibility.mark_artifact_ready("artifact"));
        assert!(visibility.is_collapsed());
    }

    #[test]
    fn cancelling_a_capture_keeps_the_stack_collapsed() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.collapse();
        let capture = visibility.begin_capture().expect("capture should start");
        assert!(visibility.restore_capture(capture));
        assert!(visibility.is_collapsed());
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
    fn clipboard_ownership_clears_only_the_replaced_artifact() {
        let mut ownership = ClipboardOwnership::default();
        ownership.record(41, "first".to_owned(), FINGERPRINT);

        assert!(!ownership.clear_if_artifact("second"));
        assert_eq!(ownership.current_artifact(41).as_deref(), Some("first"));
        assert!(ownership.clear_if_artifact("first"));
        assert!(ownership.current_artifact(41).is_none());
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

    #[test]
    fn editor_exports_are_findable_without_joining_the_preview_stack() {
        use captures_capture::CaptureMode;

        use crate::models::{CaptureArtifact, ClipboardCopyStatus};

        use super::AppState;

        let state = AppState::new();
        let original = CaptureArtifact {
            id: "capture-1".to_owned(),
            path: None,
            preview_url: String::new(),
            full_url: String::new(),
            width: 8,
            height: 8,
            size_bytes: 12,
            created_at: "2026-08-29T00:00:00Z".to_owned(),
            mode: CaptureMode::Region,
            history_saved: true,
            clipboard_copy_status: ClipboardCopyStatus::Skipped,
            image_png: vec![1],
            preview_png: vec![2],
        };
        let saved = CaptureArtifact {
            id: "saved-1".to_owned(),
            path: Some("/tmp/saved.png".to_owned()),
            ..original.clone()
        };
        state.artifacts.lock().push(original);
        state.store_editor_artifact("capture-1", saved);

        assert_eq!(state.artifacts.lock().len(), 1);
        assert_eq!(state.artifacts.lock()[0].id, "capture-1");
        assert_eq!(state.editor_artifacts.lock().len(), 1);
        assert_eq!(
            state
                .find_artifact("saved-1")
                .and_then(|artifact| artifact.path),
            Some("/tmp/saved.png".to_owned())
        );

        let saved_again = CaptureArtifact {
            id: "saved-2".to_owned(),
            path: Some("/tmp/saved-2.png".to_owned()),
            preview_url: String::new(),
            full_url: String::new(),
            width: 8,
            height: 8,
            size_bytes: 12,
            created_at: "2026-08-29T00:00:00Z".to_owned(),
            mode: CaptureMode::Region,
            history_saved: true,
            clipboard_copy_status: ClipboardCopyStatus::Skipped,
            image_png: vec![5],
            preview_png: vec![6],
        };
        state.store_editor_artifact("saved-1", saved_again);
        assert_eq!(state.editor_artifacts.lock().len(), 1);
        assert!(state.find_artifact("saved-1").is_none());
        assert_eq!(
            state
                .find_artifact("saved-2")
                .and_then(|artifact| artifact.path),
            Some("/tmp/saved-2.png".to_owned())
        );

        let other = CaptureArtifact {
            id: "saved-other".to_owned(),
            path: Some("/tmp/other.png".to_owned()),
            preview_url: String::new(),
            full_url: String::new(),
            width: 8,
            height: 8,
            size_bytes: 12,
            created_at: "2026-08-29T00:00:00Z".to_owned(),
            mode: CaptureMode::Region,
            history_saved: true,
            clipboard_copy_status: ClipboardCopyStatus::Skipped,
            image_png: vec![7],
            preview_png: vec![8],
        };
        state.store_editor_artifact("capture-2", other);
        assert_eq!(state.editor_artifacts.lock().len(), 2);

        state.drop_editor_artifacts_for_owner("capture-1");
        assert!(state.find_artifact("saved-2").is_none());
        assert!(state.find_artifact("saved-other").is_some());

        state.forget_editor_artifacts_for_ids(&["saved-other".to_owned()], &[]);
        assert!(state.find_artifact("saved-other").is_none());
        assert!(state.editor_artifacts.lock().is_empty());

        state.store_editor_artifact(
            "capture-1",
            CaptureArtifact {
                id: "saved-keep".to_owned(),
                path: Some("/tmp/keep.png".to_owned()),
                preview_url: String::new(),
                full_url: String::new(),
                width: 8,
                height: 8,
                size_bytes: 12,
                created_at: "2026-08-29T00:00:00Z".to_owned(),
                mode: CaptureMode::Region,
                history_saved: true,
                clipboard_copy_status: ClipboardCopyStatus::Skipped,
                image_png: vec![9],
                preview_png: vec![10],
            },
        );
        state
            .forget_editor_artifacts_for_ids(&["saved-keep".to_owned()], &["capture-1".to_owned()]);
        assert!(state.find_artifact("saved-keep").is_some());

        let overwritten = CaptureArtifact {
            id: "saved-keep".to_owned(),
            path: Some("/tmp/keep-again.png".to_owned()),
            preview_url: String::new(),
            full_url: String::new(),
            width: 8,
            height: 8,
            size_bytes: 12,
            created_at: "2026-08-29T00:00:00Z".to_owned(),
            mode: CaptureMode::Region,
            history_saved: true,
            clipboard_copy_status: ClipboardCopyStatus::Skipped,
            image_png: vec![3],
            preview_png: vec![4],
        };
        assert!(state.replace_artifact(overwritten));
        assert_eq!(state.artifacts.lock().len(), 1);
        assert_eq!(
            state
                .find_artifact("saved-keep")
                .and_then(|artifact| artifact.path),
            Some("/tmp/keep-again.png".to_owned())
        );
    }
}
