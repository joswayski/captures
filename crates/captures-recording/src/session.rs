use thiserror::Error;
use uuid::Uuid;

use crate::{RecordingOptions, RecordingSessionSnapshot, RecordingState};

pub type RecordingResult<T> = Result<T, RecordingError>;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecordingError {
    #[error("a recording session is already active")]
    AlreadyActive,
    #[error("the recording session is no longer available")]
    SessionUnavailable,
    #[error("invalid recording options: {0}")]
    InvalidOptions(String),
    #[error("cannot transition recording from {from:?} to {to:?}")]
    InvalidTransition {
        from: RecordingState,
        to: RecordingState,
    },
}

#[derive(Debug)]
struct RecordingSession {
    id: Uuid,
    options: RecordingOptions,
    state: RecordingState,
    accumulated_ms: u64,
    active_started_at_ms: Option<u64>,
    countdown_ends_at_ms: Option<u64>,
    warning: Option<String>,
    error: Option<String>,
}

impl RecordingSession {
    fn new(options: RecordingOptions) -> RecordingResult<Self> {
        options
            .validate()
            .map_err(|error| RecordingError::InvalidOptions(error.to_owned()))?;
        Ok(Self {
            id: Uuid::new_v4(),
            options,
            state: RecordingState::Selecting,
            accumulated_ms: 0,
            active_started_at_ms: None,
            countdown_ends_at_ms: None,
            warning: None,
            error: None,
        })
    }

    fn transition(&mut self, next: RecordingState, now_ms: u64) -> RecordingResult<()> {
        let valid = matches!(
            (self.state, next),
            (RecordingState::Selecting, RecordingState::Countdown)
                | (RecordingState::Selecting, RecordingState::Recording)
                | (RecordingState::Countdown, RecordingState::Recording)
                | (RecordingState::Countdown, RecordingState::Discarded)
                | (RecordingState::Recording, RecordingState::Paused)
                | (RecordingState::Recording, RecordingState::Finalizing)
                | (RecordingState::Recording, RecordingState::Countdown)
                | (RecordingState::Paused, RecordingState::Recording)
                | (RecordingState::Paused, RecordingState::Finalizing)
                | (RecordingState::Paused, RecordingState::Countdown)
                | (RecordingState::Finalizing, RecordingState::Ready)
                | (RecordingState::Ready, RecordingState::Editor)
                | (RecordingState::Editor, RecordingState::Ready)
        );
        if !valid {
            return Err(RecordingError::InvalidTransition {
                from: self.state,
                to: next,
            });
        }

        if self.state == RecordingState::Recording {
            self.accumulated_ms = self
                .accumulated_ms
                .saturating_add(now_ms.saturating_sub(self.active_started_at_ms.unwrap_or(now_ms)));
            self.active_started_at_ms = None;
        }
        if next == RecordingState::Recording {
            self.active_started_at_ms = Some(now_ms);
        }
        self.countdown_ends_at_ms = (next == RecordingState::Countdown)
            .then(|| now_ms.saturating_add(u64::from(self.options.countdown_seconds) * 1_000));
        if matches!(next, RecordingState::Countdown)
            && matches!(
                self.state,
                RecordingState::Recording | RecordingState::Paused
            )
        {
            self.accumulated_ms = 0;
            self.active_started_at_ms = None;
            self.warning = None;
            self.error = None;
        }
        self.state = next;
        Ok(())
    }

    fn fail(&mut self, message: String, now_ms: u64) {
        if self.state == RecordingState::Recording {
            self.accumulated_ms = self
                .accumulated_ms
                .saturating_add(now_ms.saturating_sub(self.active_started_at_ms.unwrap_or(now_ms)));
        }
        self.active_started_at_ms = None;
        self.countdown_ends_at_ms = None;
        self.state = RecordingState::Failed;
        self.error = Some(message);
    }

    fn discard(&mut self, now_ms: u64) {
        if self.state == RecordingState::Recording {
            self.accumulated_ms = self
                .accumulated_ms
                .saturating_add(now_ms.saturating_sub(self.active_started_at_ms.unwrap_or(now_ms)));
        }
        self.active_started_at_ms = None;
        self.countdown_ends_at_ms = None;
        self.state = RecordingState::Discarded;
    }

    fn snapshot(&self, now_ms: u64) -> RecordingSessionSnapshot {
        let elapsed_ms = if self.state == RecordingState::Recording {
            self.accumulated_ms
                .saturating_add(now_ms.saturating_sub(self.active_started_at_ms.unwrap_or(now_ms)))
        } else {
            self.accumulated_ms
        };
        let countdown_remaining_seconds = self.countdown_ends_at_ms.map(|ends_at_ms| {
            let remaining_ms = ends_at_ms.saturating_sub(now_ms);
            u8::try_from(remaining_ms.saturating_add(999) / 1_000).unwrap_or(u8::MAX)
        });
        RecordingSessionSnapshot {
            id: self.id.to_string(),
            state: self.state,
            options: self.options.clone(),
            elapsed_ms,
            countdown_remaining_seconds,
            warning: self.warning.clone(),
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub struct RecordingCoordinator {
    active: Option<RecordingSession>,
}

impl RecordingCoordinator {
    pub fn begin(
        &mut self,
        options: RecordingOptions,
        now_ms: u64,
    ) -> RecordingResult<RecordingSessionSnapshot> {
        if self
            .active
            .as_ref()
            .is_some_and(|session| !session.state.is_terminal())
        {
            return Err(RecordingError::AlreadyActive);
        }
        let session = RecordingSession::new(options)?;
        let snapshot = session.snapshot(now_ms);
        self.active = Some(session);
        Ok(snapshot)
    }

    pub fn snapshot(&self, now_ms: u64) -> Option<RecordingSessionSnapshot> {
        self.active.as_ref().map(|session| session.snapshot(now_ms))
    }

    pub fn transition(
        &mut self,
        session_id: &str,
        next: RecordingState,
        now_ms: u64,
    ) -> RecordingResult<RecordingSessionSnapshot> {
        let session = self.session_mut(session_id)?;
        session.transition(next, now_ms)?;
        Ok(session.snapshot(now_ms))
    }

    pub fn warn(
        &mut self,
        session_id: &str,
        warning: Option<String>,
        now_ms: u64,
    ) -> RecordingResult<RecordingSessionSnapshot> {
        let session = self.session_mut(session_id)?;
        session.warning = warning;
        Ok(session.snapshot(now_ms))
    }

    pub fn update_options(
        &mut self,
        session_id: &str,
        options: RecordingOptions,
        now_ms: u64,
    ) -> RecordingResult<RecordingSessionSnapshot> {
        options
            .validate()
            .map_err(|error| RecordingError::InvalidOptions(error.to_owned()))?;
        let session = self.session_mut(session_id)?;
        session.options = options;
        Ok(session.snapshot(now_ms))
    }

    pub fn fail(
        &mut self,
        session_id: &str,
        message: String,
        now_ms: u64,
    ) -> RecordingResult<RecordingSessionSnapshot> {
        let session = self.session_mut(session_id)?;
        session.fail(message, now_ms);
        Ok(session.snapshot(now_ms))
    }

    pub fn discard(
        &mut self,
        session_id: &str,
        now_ms: u64,
    ) -> RecordingResult<RecordingSessionSnapshot> {
        let session = self.session_mut(session_id)?;
        session.discard(now_ms);
        Ok(session.snapshot(now_ms))
    }

    fn session_mut(&mut self, session_id: &str) -> RecordingResult<&mut RecordingSession> {
        self.active
            .as_mut()
            .filter(|session| session.id.to_string() == session_id)
            .ok_or(RecordingError::SessionUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingKind, RecordingOptions,
        RecordingState, RecordingTarget,
    };

    use super::{RecordingCoordinator, RecordingError};

    fn options() -> RecordingOptions {
        RecordingOptions {
            kind: RecordingKind::Video,
            target: RecordingTarget::Region {
                display_id: "display".to_owned(),
                rect: CaptureRect {
                    x: 0,
                    y: 0,
                    width: 1_920,
                    height: 1_080,
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
    fn tracks_elapsed_time_only_while_recording() {
        let mut coordinator = RecordingCoordinator::default();
        let session = coordinator.begin(options(), 0).expect("session begins");
        let id = session.id;
        coordinator
            .transition(&id, RecordingState::Countdown, 10)
            .expect("countdown starts");
        assert_eq!(
            coordinator
                .snapshot(1_010)
                .expect("countdown snapshot")
                .countdown_remaining_seconds,
            Some(2)
        );
        coordinator
            .transition(&id, RecordingState::Recording, 3_010)
            .expect("recording starts");
        assert_eq!(
            coordinator
                .snapshot(3_010)
                .expect("recording snapshot")
                .countdown_remaining_seconds,
            None
        );
        assert_eq!(
            coordinator.snapshot(4_010).expect("snapshot").elapsed_ms,
            1_000
        );

        coordinator
            .transition(&id, RecordingState::Paused, 5_010)
            .expect("recording pauses");
        assert_eq!(
            coordinator.snapshot(9_010).expect("snapshot").elapsed_ms,
            2_000
        );

        coordinator
            .transition(&id, RecordingState::Recording, 10_010)
            .expect("recording resumes");
        coordinator
            .transition(&id, RecordingState::Finalizing, 11_010)
            .expect("recording stops");
        assert_eq!(
            coordinator.snapshot(20_000).expect("snapshot").elapsed_ms,
            3_000
        );
    }

    #[test]
    fn restart_resets_elapsed_time_and_reuses_the_session() {
        let mut coordinator = RecordingCoordinator::default();
        let id = coordinator.begin(options(), 0).expect("session begins").id;
        coordinator
            .transition(&id, RecordingState::Recording, 100)
            .expect("recording starts");
        let restarted = coordinator
            .transition(&id, RecordingState::Countdown, 2_100)
            .expect("recording restarts");
        assert_eq!(restarted.elapsed_ms, 0);
        assert_eq!(restarted.state, RecordingState::Countdown);
    }

    #[test]
    fn rejects_stale_sessions_and_overlapping_recordings() {
        let mut coordinator = RecordingCoordinator::default();
        let session = coordinator.begin(options(), 0).expect("session begins");
        assert_eq!(
            coordinator.begin(options(), 0),
            Err(RecordingError::AlreadyActive)
        );
        assert_eq!(
            coordinator.transition("stale", RecordingState::Recording, 0),
            Err(RecordingError::SessionUnavailable)
        );
        coordinator
            .discard(&session.id, 0)
            .expect("session discards");
        assert!(coordinator.begin(options(), 0).is_ok());
    }

    #[test]
    fn rejects_invalid_lifecycle_transitions() {
        let mut coordinator = RecordingCoordinator::default();
        let session = coordinator.begin(options(), 0).expect("session begins");
        assert_eq!(
            coordinator.transition(&session.id, RecordingState::Paused, 0),
            Err(RecordingError::InvalidTransition {
                from: RecordingState::Selecting,
                to: RecordingState::Paused,
            })
        );
    }
}
