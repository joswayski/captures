//! Which capture the system clipboard still holds, shared by native hosts.
//!
//! Hosts record a clipboard revision (macOS change count, Windows sequence
//! number, or an application counter on Linux) plus a pixel fingerprint when
//! they copy a capture. A later revision means another app replaced the
//! clipboard. Linux has no system revision, so hosts periodically compare the
//! clipboard image with the recorded fingerprint instead.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClipboardFingerprint {
    pub width: u32,
    pub height: u32,
    pub checksum: u64,
}

impl ClipboardFingerprint {
    pub fn of_rgba(width: u32, height: u32, rgba: &[u8]) -> Self {
        let mut hasher = DefaultHasher::new();
        width.hash(&mut hasher);
        height.hash(&mut hasher);
        rgba.hash(&mut hasher);
        Self {
            width,
            height,
            checksum: hasher.finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClipboardVerification {
    pub revision: i64,
    pub fingerprint: ClipboardFingerprint,
}

#[derive(Default)]
pub struct ClipboardOwnership {
    revision: Option<i64>,
    artifact_id: Option<String>,
    fingerprint: Option<ClipboardFingerprint>,
    last_verification: Option<Instant>,
}

impl ClipboardOwnership {
    pub fn record(
        &mut self,
        revision: i64,
        artifact_id: String,
        fingerprint: ClipboardFingerprint,
    ) {
        self.revision = Some(revision);
        self.artifact_id = Some(artifact_id);
        self.fingerprint = Some(fingerprint);
        self.last_verification = None;
    }

    /// The owning artifact while the clipboard is still at the recorded
    /// revision; any other revision forgets ownership.
    pub fn current_artifact(&mut self, revision: i64) -> Option<String> {
        if self.revision != Some(revision) {
            self.clear();
        }
        self.artifact_id.clone()
    }

    /// A throttled request to compare the clipboard with the recorded pixels.
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

    /// Forget a verified-stale copy unless a newer copy has been recorded.
    pub fn clear_if_revision(&mut self, revision: i64) -> bool {
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
        self.last_verification = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: ClipboardFingerprint = ClipboardFingerprint {
        width: 2,
        height: 3,
        checksum: 41,
    };

    #[test]
    fn fingerprints_include_dimensions_and_pixels() {
        let original = ClipboardFingerprint::of_rgba(1, 1, &[1, 2, 3, 255]);
        assert_eq!(
            original,
            ClipboardFingerprint::of_rgba(1, 1, &[1, 2, 3, 255])
        );
        assert_ne!(
            original,
            ClipboardFingerprint::of_rgba(2, 1, &[1, 2, 3, 255])
        );
        assert_ne!(
            original,
            ClipboardFingerprint::of_rgba(1, 1, &[1, 2, 4, 255])
        );
    }

    #[test]
    fn ownership_tracks_one_artifact_until_the_clipboard_changes() {
        let mut ownership = ClipboardOwnership::default();
        ownership.record(41, "first".to_owned(), FINGERPRINT);
        assert_eq!(ownership.current_artifact(41).as_deref(), Some("first"));

        ownership.record(42, "second".to_owned(), FINGERPRINT);
        assert_eq!(ownership.current_artifact(42).as_deref(), Some("second"));
        assert!(ownership.current_artifact(43).is_none());
        assert!(ownership.current_artifact(42).is_none());
    }

    #[test]
    fn ownership_clears_only_the_replaced_artifact() {
        let mut ownership = ClipboardOwnership::default();
        ownership.record(41, "first".to_owned(), FINGERPRINT);
        assert!(!ownership.clear_if_artifact("second"));
        assert_eq!(ownership.current_artifact(41).as_deref(), Some("first"));
        assert!(ownership.clear_if_artifact("first"));
        assert!(ownership.current_artifact(41).is_none());
    }

    #[test]
    fn verification_is_throttled_and_cannot_clear_a_newer_copy() {
        let mut ownership = ClipboardOwnership::default();
        let now = Instant::now();
        ownership.record(41, "first".to_owned(), FINGERPRINT);
        assert_eq!(
            ownership.verification(now, Duration::from_secs(1)),
            Some(ClipboardVerification {
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
