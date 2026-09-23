//! Blocking recording lifecycle. Keep this owner on a worker, never the native
//! event loop. The host owns selector/countdown presentation and start cancellation.
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use captures_capture::DisplayDescriptor;
use captures_history::HistoryEntry;
use captures_media::{CancelToken, MediaToolchain, RecordingAssemblyKind, RecordingSegmentInput};
use captures_recording::{
    DraftStore, RecordingCoordinator, RecordingDraftManifest, RecordingKind, RecordingOptions,
    RecordingSegmentManifest, RecordingSessionSnapshot, RecordingState,
};
use serde::Serialize;

use crate::{NativeRecordingSegment, start_native_segment};

#[derive(Debug, Serialize)]
pub struct FinalizedRecording {
    pub entry: HistoryEntry,
    pub path: PathBuf,
    /// Publication succeeded with an engine/device or housekeeping warning.
    pub warning: Option<String>,
}

pub struct RecordingSession {
    recovery_lease: Option<File>,
    coordinator: RecordingCoordinator,
    store: DraftStore,
    manifest: RecordingDraftManifest,
    directory: PathBuf,
    display: DisplayDescriptor,
    active: Option<NativeRecordingSegment>,
    started_at_ms: Option<u64>,
}

impl RecordingSession {
    /// Creates a recovery bundle before countdown/start. This does not request
    /// permissions or start capture. Hosts must reject concurrent capture flows.
    pub fn prepare(
        recovery_root: PathBuf,
        options: RecordingOptions,
        display: DisplayDescriptor,
    ) -> Result<Self, String> {
        let now = now_ms();
        let mut coordinator = RecordingCoordinator::default();
        let initial = coordinator.begin(options.clone(), now).map_err(string)?;
        let mut manifest = RecordingDraftManifest::new(initial.id, options, now);
        let store = DraftStore::new(recovery_root);
        coordinator
            .transition(&manifest.session_id, RecordingState::Countdown, now)
            .map_err(string)?;
        manifest.state = RecordingState::Countdown;
        let recovery_lease = super::recovery::lease(store.root())?;
        let directory = store.create(&manifest).map_err(string)?;
        Ok(Self {
            recovery_lease: Some(recovery_lease),
            coordinator,
            store,
            manifest,
            directory,
            display,
            active: None,
            started_at_ms: None,
        })
    }

    pub fn snapshot(&self) -> RecordingSessionSnapshot {
        let mut snapshot = self
            .coordinator
            .snapshot(now_ms())
            .expect("session owns coordinator");
        // Engine/device warnings are not coordinator transitions. Surface them
        // while running and retain completed-segment warnings across pause/stop.
        snapshot.warning = self
            .active
            .as_ref()
            .and_then(NativeRecordingSegment::warning)
            .or_else(|| {
                self.manifest.segments.iter().rev().find_map(|segment| {
                    segment
                        .system_audio_warning
                        .clone()
                        .or_else(|| segment.microphone_warning.clone())
                })
            });
        snapshot
    }

    pub fn manifest(&self) -> &RecordingDraftManifest {
        &self.manifest
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Called after countdown and hiding capture controls. The callback reads
    /// the host's capture-generation cancellation gate before and after the
    /// blocking engine start. A cancelled initial start discards its bundle;
    /// cancelled resume leaves completed segments paused for stop/finalization.
    pub fn start(
        &mut self,
        exclude_captures_app: bool,
        is_current: impl Fn() -> bool,
    ) -> Result<RecordingSessionSnapshot, String> {
        self.start_with(exclude_captures_app, is_current, start_native_segment)
    }

    fn start_with(
        &mut self,
        exclude_captures_app: bool,
        is_current: impl Fn() -> bool,
        open: impl FnOnce(
            &RecordingOptions,
            &Path,
            &DisplayDescriptor,
            bool,
        ) -> Result<NativeRecordingSegment, String>,
    ) -> Result<RecordingSessionSnapshot, String> {
        if !matches!(
            self.manifest.state,
            RecordingState::Countdown | RecordingState::Paused
        ) {
            return Err("Recording is not waiting to start".into());
        }
        if !is_current() {
            if self.manifest.state == RecordingState::Countdown {
                self.discard()?;
            }
            return Err("Recording cancelled".into());
        }
        let path = self
            .directory
            .join(format!("segment-{:03}.mp4", self.manifest.segments.len()));
        let segment = match open(
            &self.manifest.options,
            &path,
            &self.display,
            exclude_captures_app,
        ) {
            Ok(segment) => segment,
            Err(error) => return Err(self.fail(error)),
        };
        if !is_current() {
            if let Err(error) = segment.discard() {
                return Err(self.fail(error.to_string()));
            }
            if self.manifest.state == RecordingState::Countdown {
                self.discard()?;
            }
            return Err("Recording cancelled".into());
        }
        let now = now_ms();
        let (width, height) = segment.dimensions();
        let prepare = (|| {
            let relative = |sidecar: Option<(PathBuf, i64)>| -> Result<_, String> {
                sidecar
                    .map(|(path, offset)| {
                        path.strip_prefix(&self.directory)
                            .map(|path| (Some(path.to_string_lossy().into_owned()), offset))
                            .map_err(|_| "Recording audio escaped its recovery bundle".to_owned())
                    })
                    .transpose()
                    .map(|value| value.unwrap_or((None, 0)))
            };
            let (system_audio_relative_path, system_audio_offset_ms) =
                relative(segment.system_audio_draft_info())?;
            let (microphone_relative_path, microphone_offset_ms) =
                relative(segment.microphone_draft_info())?;
            let index = u32::try_from(self.manifest.segments.len()).map_err(string)?;
            self.manifest.segments.push(RecordingSegmentManifest {
                index,
                relative_path: format!("segment-{index:03}.mp4"),
                system_audio_relative_path,
                system_audio_offset_ms,
                system_audio_warning: None,
                microphone_relative_path,
                microphone_offset_ms,
                microphone_warning: None,
                started_at_ms: now,
                duration_ms: 0,
                width,
                height,
                size_bytes: 0,
                dropped_frames: 0,
                complete: false,
            });
            self.transition(RecordingState::Recording, now)
        })();
        if let Err(error) = prepare {
            let cleanup = segment.discard().err();
            return Err(self.fail(match cleanup {
                Some(cleanup) => format!("{error}; recording cleanup failed: {cleanup}"),
                None => error,
            }));
        }
        self.started_at_ms = Some(now);
        self.active = Some(segment);
        Ok(self.snapshot())
    }

    /// Change only the microphone's live mute state. A running take completes
    /// its current segment before the option is durably changed, then opens a
    /// new segment with the same target and all other options. Paused sessions
    /// persist the option without resuming. Completed media remains recoverable
    /// if cancellation or engine startup prevents the replacement segment.
    pub fn set_microphone_muted(
        &mut self,
        muted: bool,
        exclude_captures_app: bool,
        is_current: impl Fn() -> bool,
    ) -> Result<RecordingSessionSnapshot, String> {
        self.set_microphone_muted_with(
            muted,
            exclude_captures_app,
            is_current,
            start_native_segment,
        )
    }

    fn set_microphone_muted_with(
        &mut self,
        muted: bool,
        exclude_captures_app: bool,
        is_current: impl Fn() -> bool,
        open: impl FnOnce(
            &RecordingOptions,
            &Path,
            &DisplayDescriptor,
            bool,
        ) -> Result<NativeRecordingSegment, String>,
    ) -> Result<RecordingSessionSnapshot, String> {
        if !is_current() {
            return Err("Recording cancelled".into());
        }
        if !matches!(
            self.manifest.state,
            RecordingState::Recording | RecordingState::Paused
        ) {
            return Err("Microphone mute can only change while recording or paused".into());
        }
        if self.manifest.options.audio.microphone_device_id.is_none() {
            return Err("Recording does not have a selected microphone".into());
        }
        if self.manifest.options.audio.microphone_muted == muted {
            return Ok(self.snapshot());
        }

        let was_recording = self.manifest.state == RecordingState::Recording;
        if was_recording {
            self.complete_active()?;
            self.transition(RecordingState::Paused, now_ms())
                .map_err(|error| self.fail(error))?;
        }

        let mut options = self.manifest.options.clone();
        options.audio.microphone_muted = muted;
        self.persist_options(options)
            .map_err(|error| self.fail(error))?;

        if was_recording {
            self.start_with(exclude_captures_app, is_current, open)
        } else {
            Ok(self.snapshot())
        }
    }

    pub fn pause(&mut self) -> Result<RecordingSessionSnapshot, String> {
        if self.manifest.state != RecordingState::Recording {
            return Err("Recording is not running".into());
        }
        self.complete_active()?;
        self.transition(RecordingState::Paused, now_ms())
            .map_err(|error| self.fail(error))?;
        Ok(self.snapshot())
    }

    /// Replace the current take while retaining its target and options. Every
    /// path removed here belongs to this session's recovery bundle; unrelated
    /// bundles under the same root are never scanned or removed.
    pub fn restart(&mut self) -> Result<RecordingSessionSnapshot, String> {
        if !matches!(
            self.manifest.state,
            RecordingState::Recording | RecordingState::Paused | RecordingState::Failed
        ) {
            return Err("Recording is not running, paused, or failed".into());
        }
        if let Some(segment) = self.active.take()
            && let Err(error) = segment.discard()
        {
            return Err(self.fail(error.to_string()));
        }
        self.started_at_ms = None;

        let mut take_paths = self
            .manifest
            .segments
            .iter()
            .flat_map(|segment| {
                [
                    Some(self.directory.join(&segment.relative_path)),
                    segment
                        .system_audio_relative_path
                        .as_ref()
                        .map(|path| self.directory.join(path)),
                    segment
                        .microphone_relative_path
                        .as_ref()
                        .map(|path| self.directory.join(path)),
                ]
                .into_iter()
                .flatten()
            })
            .collect::<Vec<_>>();
        take_paths.extend(
            ["assembled.mp4", "assembled.gif", "poster.png"].map(|path| self.directory.join(path)),
        );
        for path in take_paths {
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(self.fail(format!(
                    "Could not remove replaced recording media {}: {error}",
                    path.display()
                )));
            }
        }

        self.manifest.segments.clear();
        self.manifest.final_path = None;
        self.manifest.last_error = None;
        self.transition(RecordingState::Countdown, now_ms())
            .map_err(|error| self.fail(error))?;
        Ok(self.snapshot())
    }

    /// Stop the engine and durably complete its segment before finalization.
    /// Recovery media stays owned by this session until publication succeeds.
    pub fn stop(&mut self) -> Result<RecordingSessionSnapshot, String> {
        if !matches!(
            self.manifest.state,
            RecordingState::Recording | RecordingState::Paused
        ) {
            return Err("Recording is not running or paused".into());
        }
        self.transition(RecordingState::Finalizing, now_ms())
            .map_err(|error| self.fail(error))?;
        self.complete_active()?;
        Ok(self.snapshot())
    }

    /// Assemble and publish into private History, not the user's output folder.
    /// Failed/cancelled assembly or publication retains the source bundle.
    /// Once publication succeeds, subsequent housekeeping errors are warnings:
    /// the returned history artifact is valid and must not be published again.
    pub fn finish(
        &mut self,
        history_root: &Path,
        tools: &MediaToolchain,
        cancel: &CancelToken,
    ) -> Result<FinalizedRecording, String> {
        if self.manifest.state != RecordingState::Finalizing {
            return Err("Recording is not finalizing".into());
        }
        let result = self.publish(history_root, tools, cancel);
        let (entry, path) = result.map_err(|error| self.fail(error))?;
        self.manifest.final_path = Some(path.to_string_lossy().into_owned());
        let warning = match self.transition(RecordingState::Ready, now_ms()) {
            Err(error) => Some(format!(
                "Recording saved to {}; could not save recovery state: {error}",
                path.display()
            )),
            Ok(()) if self.manifest.options.kind == RecordingKind::Video => self
                .store
                .remove(&self.manifest.session_id)
                .err()
                .map(|error| format!("Recording saved; could not remove source bundle: {error}")),
            Ok(()) => None, // GIF source media remains available for editing.
        };
        self.recovery_lease.take();
        Ok(FinalizedRecording {
            entry,
            path,
            warning: warning.or_else(|| self.snapshot().warning),
        })
    }

    fn publish(
        &self,
        history_root: &Path,
        tools: &MediaToolchain,
        cancel: &CancelToken,
    ) -> Result<(HistoryEntry, PathBuf), String> {
        if self.manifest.segments.is_empty()
            || self
                .manifest
                .segments
                .iter()
                .any(|segment| !segment.complete)
        {
            return Err("Recording does not contain complete media".into());
        }
        let options = &self.manifest.options;
        let segments = self
            .manifest
            .segments
            .iter()
            .map(|segment| RecordingSegmentInput {
                video_path: self.directory.join(&segment.relative_path),
                system_audio_path: segment
                    .system_audio_relative_path
                    .as_ref()
                    .map(|path| self.directory.join(path)),
                system_audio_offset_ms: segment.system_audio_offset_ms,
                microphone_path: segment
                    .microphone_relative_path
                    .as_ref()
                    .map(|path| self.directory.join(path)),
                microphone_offset_ms: segment.microphone_offset_ms,
                duration_ms: segment.duration_ms,
            })
            .collect::<Vec<_>>();
        let (kind, extension, mime_type) = match options.kind {
            RecordingKind::Video => (
                RecordingAssemblyKind::Video {
                    capture_system_audio: options.audio.capture_system_audio,
                },
                "mp4",
                "video/mp4",
            ),
            RecordingKind::Gif => (
                RecordingAssemblyKind::Gif {
                    frames_per_second: options.frames_per_second,
                    max_width: options.gif.max_width,
                    max_colors: options.gif.max_colors,
                },
                "gif",
                "image/gif",
            ),
        };
        let assembled = self.directory.join(format!("assembled.{extension}"));
        let outcome = tools
            .assemble_recording(&segments, &assembled, &self.directory, kind, cancel)
            .map_err(string)?;
        let poster = self.directory.join("poster.png");
        tools
            .create_poster(&assembled, &poster, cancel)
            .map_err(string)?;
        let poster = std::fs::read(poster).map_err(string)?;
        if cancel.is_cancelled() {
            return Err("Recording finalization cancelled".into());
        }
        let metadata = outcome.probe.metadata;
        let entry = HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            kind: options.kind.into(),
            // Hosts resolve native media/poster paths; no web protocol required.
            preview_url: String::new(),
            full_url: String::new(),
            width: metadata.width,
            height: metadata.height,
            size_bytes: metadata.size_bytes,
            created_at: chrono::Utc::now().to_rfc3339(),
            mode: None,
            saved_path: None,
            mime_type: Some(mime_type.into()),
            duration_ms: metadata.duration_ms,
            target: Some(options.target.clone()),
            has_system_audio: options.kind == RecordingKind::Video
                && options.audio.capture_system_audio,
            has_microphone_audio: outcome.has_microphone_audio,
            dropped_frames: self
                .manifest
                .segments
                .iter()
                .map(|s| s.dropped_frames)
                .sum(),
        };
        let path = captures_history::save_recording(history_root, &entry, &poster, &assembled)
            .map_err(string)?;
        Ok((entry, path))
    }

    pub fn discard(&mut self) -> Result<RecordingSessionSnapshot, String> {
        if !matches!(
            self.manifest.state,
            RecordingState::Countdown
                | RecordingState::Recording
                | RecordingState::Paused
                | RecordingState::Failed
        ) {
            return Err("Recording cannot be discarded after finalization or discard".into());
        }
        if let Some(segment) = self.active.take() {
            segment
                .discard()
                .map_err(|error| self.fail(error.to_string()))?;
        }
        let now = now_ms();
        self.coordinator
            .discard(&self.manifest.session_id, now)
            .map_err(string)?;
        self.manifest.state = RecordingState::Discarded;
        self.manifest.updated_at_ms = now;
        self.store.save(&self.manifest).map_err(string)?;
        self.store
            .remove(&self.manifest.session_id)
            .map_err(string)?;
        self.started_at_ms = None;
        self.recovery_lease.take();
        Ok(self.snapshot())
    }

    fn complete_active(&mut self) -> Result<(), String> {
        if let Some(segment) = self.active.take() {
            let info = segment
                .stop()
                .map_err(|error| self.fail(error.to_string()))?;
            let now = now_ms();
            self.manifest
                .complete_segment(
                    &self.directory,
                    info,
                    self.started_at_ms.take().unwrap_or(now),
                    now,
                )
                .map_err(|error| self.fail(error.to_string()))?;
            self.store
                .save(&self.manifest)
                .map_err(|error| self.fail(error.to_string()))?;
        }
        Ok(())
    }

    fn transition(&mut self, state: RecordingState, now: u64) -> Result<(), String> {
        self.coordinator
            .transition(&self.manifest.session_id, state, now)
            .map_err(string)?;
        self.manifest.state = state;
        self.manifest.updated_at_ms = now;
        self.store.save(&self.manifest).map_err(string)
    }

    fn persist_options(&mut self, options: RecordingOptions) -> Result<(), String> {
        let now = now_ms();
        self.coordinator
            .update_options(&self.manifest.session_id, options.clone(), now)
            .map_err(string)?;
        self.manifest.options = options;
        self.manifest.updated_at_ms = now;
        self.store.save(&self.manifest).map_err(string)
    }

    fn fail(&mut self, message: String) -> String {
        let now = now_ms();
        // A metadata write can fail while the engine is still running. Stop it
        // without deleting its media, then make one best-effort recovery write.
        let stopped = self
            .active
            .take()
            .map(|segment| {
                let info = segment.stop().map_err(string)?;
                self.manifest
                    .complete_segment(
                        &self.directory,
                        info,
                        self.started_at_ms.take().unwrap_or(now),
                        now,
                    )
                    .map_err(string)
            })
            .transpose();
        let message = match stopped {
            Ok(_) => message,
            Err(error) => format!("{message}; could not complete recording segment: {error}"),
        };
        let _ = self
            .coordinator
            .fail(&self.manifest.session_id, message.clone(), now);
        self.manifest.state = RecordingState::Failed;
        self.manifest.last_error = Some(message.clone());
        self.manifest.updated_at_ms = now;
        match self.store.save(&self.manifest) {
            Ok(()) => message,
            Err(error) => format!("{message}; could not save recording recovery state: {error}"),
        }
    }
}

fn string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_recording::{
        AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingKind,
        RecordingSegmentManifest, RecordingTarget,
    };

    fn display() -> DisplayDescriptor {
        DisplayDescriptor {
            id: "fixture".into(),
            name: "Fixture".into(),
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            scale_factor: 1.,
            is_primary: true,
        }
    }

    fn options(display: &DisplayDescriptor) -> RecordingOptions {
        RecordingOptions {
            kind: RecordingKind::Video,
            target: RecordingTarget::Region {
                display_id: display.id.clone(),
                rect: CaptureRect {
                    x: 40,
                    y: 50,
                    width: 310,
                    height: 170,
                },
            },
            frames_per_second: 15,
            max_resolution: MaxResolution::Original,
            countdown_seconds: 0,
            show_cursor: false,
            highlight_clicks: false,
            show_keystrokes: false,
            audio: AudioOptions::default(),
            gif: GifOptions::default(),
        }
    }

    #[test]
    fn cancelled_start_never_opens_engine_and_removes_only_its_recovery_bundle() {
        let root = tempfile::tempdir().unwrap();
        let sentinel = root.path().join("other-recording");
        std::fs::write(&sentinel, b"keep").unwrap();
        let display = display();
        let mut session =
            RecordingSession::prepare(root.path().into(), options(&display), display).unwrap();
        let directory = session.directory().to_owned();
        assert_eq!(session.snapshot().state, RecordingState::Countdown);
        assert!(session.pause().is_err());
        assert!(session.stop().is_err());
        assert!(
            session
                .finish(
                    root.path(),
                    &MediaToolchain::from_command_names(),
                    &CancelToken::default()
                )
                .is_err()
        );
        assert_eq!(
            session.start(false, || false).unwrap_err(),
            "Recording cancelled"
        );
        assert_eq!(session.snapshot().state, RecordingState::Discarded);
        assert!(!directory.exists());
        assert_eq!(std::fs::read(sentinel).unwrap(), b"keep");
        assert!(session.active.is_none());
        assert!(
            session.discard().is_err(),
            "retained terminal owner must not write after lease release"
        );
    }

    #[test]
    fn failed_owner_holds_lease_for_restart_or_discard_until_host_retires_it() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recording-recovery");
        let history = base.path().join("history");
        let display = display();
        let mut failed =
            RecordingSession::prepare(root.clone(), options(&display), display.clone()).unwrap();
        let id = failed.manifest().session_id.clone();
        failed.fail("injected engine failure".into());
        let recovery = crate::RecordingRecovery::new(history, MediaToolchain::from_command_names());
        assert!(recovery.list().is_err());
        assert!(
            RecordingSession::prepare(root.clone(), options(&display), display.clone()).is_err()
        );
        drop(failed); // Worker must retire its failed owner before a new Prepare or recovery.
        let rows = recovery.list().unwrap();
        assert_eq!(
            rows.iter().find(|row| row.session_id == id).unwrap().status,
            "recoverable"
        );
        let mut next = RecordingSession::prepare(root, options(&display), display).unwrap();
        next.discard().unwrap();
    }

    #[test]
    fn cancelled_resume_before_engine_open_preserves_paused_bundle() {
        let root = tempfile::tempdir().unwrap();
        let display = display();
        let mut session =
            RecordingSession::prepare(root.path().into(), options(&display), display).unwrap();
        // Exercise the state gate without opening an engine on the test runner.
        session
            .transition(RecordingState::Recording, now_ms())
            .unwrap();
        session.pause().unwrap();
        let before = session.manifest.clone();
        let sentinel = session.directory.join("completed-media");
        std::fs::write(&sentinel, b"preserve prior take").unwrap();
        assert_eq!(
            session.start(false, || false).unwrap_err(),
            "Recording cancelled"
        );
        assert_eq!(session.snapshot().state, RecordingState::Paused);
        assert_eq!(session.manifest, before);
        assert_eq!(session.store.load(&before.session_id).unwrap(), before);
        assert_eq!(std::fs::read(sentinel).unwrap(), b"preserve prior take");
        assert!(session.active.is_none());
    }

    #[test]
    fn paused_microphone_mute_is_durable_and_survives_resume_cancellation_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let display = display();
        let mut options = options(&display);
        options.audio.microphone_device_id = Some("fixture-microphone".into());
        let mut session = RecordingSession::prepare(root.path().into(), options, display).unwrap();
        session
            .transition(RecordingState::Recording, now_ms().saturating_sub(1_000))
            .unwrap();
        session.pause().unwrap();
        let elapsed = session.snapshot().elapsed_ms;

        let muted = session.set_microphone_muted(true, false, || true).unwrap();
        assert_eq!(muted.state, RecordingState::Paused);
        assert!(muted.options.audio.microphone_muted);
        assert_eq!(muted.elapsed_ms, elapsed);
        assert_eq!(session.manifest.options, muted.options);
        assert_eq!(
            session.store.load(&session.manifest.session_id).unwrap(),
            session.manifest
        );

        let before = session.manifest.clone();
        let unchanged = session.set_microphone_muted(true, false, || true).unwrap();
        assert_eq!(unchanged, muted);
        assert_eq!(
            session.manifest, before,
            "unchanged mute must not rotate media"
        );

        assert_eq!(
            session.start(false, || false).unwrap_err(),
            "Recording cancelled"
        );
        assert_eq!(session.manifest, before);
        assert!(session.snapshot().options.audio.microphone_muted);

        let restarted = session.restart().unwrap();
        assert_eq!(restarted.state, RecordingState::Countdown);
        assert!(restarted.options.audio.microphone_muted);
        assert_eq!(
            restarted.options.audio.microphone_device_id.as_deref(),
            Some("fixture-microphone")
        );
    }

    #[test]
    fn microphone_mute_rejects_stale_invalid_and_micless_sessions_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let display = display();
        let mut selected = options(&display);
        selected.audio.microphone_device_id = Some("fixture-microphone".into());
        let mut session =
            RecordingSession::prepare(root.path().join("selected"), selected, display.clone())
                .unwrap();
        let before = session.manifest.clone();
        assert_eq!(
            session
                .set_microphone_muted(true, false, || false)
                .unwrap_err(),
            "Recording cancelled"
        );
        assert_eq!(session.manifest, before);
        assert_eq!(
            session
                .set_microphone_muted(true, false, || true)
                .unwrap_err(),
            "Microphone mute can only change while recording or paused"
        );
        assert_eq!(session.manifest, before);

        let mut micless =
            RecordingSession::prepare(root.path().join("micless"), options(&display), display)
                .unwrap();
        micless
            .transition(RecordingState::Recording, now_ms())
            .unwrap();
        micless.pause().unwrap();
        let before = micless.manifest.clone();
        assert_eq!(
            micless
                .set_microphone_muted(true, false, || true)
                .unwrap_err(),
            "Recording does not have a selected microphone"
        );
        assert_eq!(micless.manifest, before);
    }

    #[test]
    fn unchanged_running_mute_does_not_complete_or_reopen_a_segment() {
        let root = tempfile::tempdir().unwrap();
        let display = display();
        let mut options = options(&display);
        options.audio.microphone_device_id = Some("fixture-microphone".into());
        let mut session = RecordingSession::prepare(root.path().into(), options, display).unwrap();
        session
            .transition(RecordingState::Recording, now_ms())
            .unwrap();

        let snapshot = session
            .set_microphone_muted_with(
                false,
                false,
                || true,
                |_, _, _, _| panic!("unchanged mute opened a recording engine"),
            )
            .unwrap();

        assert_eq!(snapshot.state, RecordingState::Recording);
        assert!(session.manifest.segments.is_empty());
    }

    #[test]
    fn running_mute_preserves_completed_media_when_reopening_fails() {
        let root = tempfile::tempdir().unwrap();
        let display = display();
        let mut options = options(&display);
        options.audio.microphone_device_id = Some("fixture-microphone".into());
        let mut session = RecordingSession::prepare(root.path().into(), options, display).unwrap();
        let media = session.directory.join("segment-000.mp4");
        std::fs::write(&media, b"accepted media").unwrap();
        session.manifest.segments.push(RecordingSegmentManifest {
            index: 0,
            relative_path: "segment-000.mp4".into(),
            system_audio_relative_path: None,
            system_audio_offset_ms: 0,
            system_audio_warning: None,
            microphone_relative_path: Some("segment-000-microphone.wav".into()),
            microphone_offset_ms: 0,
            microphone_warning: None,
            started_at_ms: 1,
            duration_ms: 500,
            width: 310,
            height: 170,
            size_bytes: 14,
            dropped_frames: 0,
            complete: true,
        });
        session.store.save(&session.manifest).unwrap();
        session
            .transition(RecordingState::Recording, now_ms().saturating_sub(500))
            .unwrap();

        let error = session
            .set_microphone_muted_with(
                true,
                false,
                || true,
                |_, _, _, _| Err("replacement microphone could not open".into()),
            )
            .unwrap_err();

        assert_eq!(error, "replacement microphone could not open");
        assert_eq!(session.snapshot().state, RecordingState::Failed);
        assert!(session.snapshot().options.audio.microphone_muted);
        assert_eq!(session.manifest.segments.len(), 1);
        assert_eq!(std::fs::read(media).unwrap(), b"accepted media");
        assert_eq!(
            session.store.load(&session.manifest.session_id).unwrap(),
            session.manifest
        );
    }

    #[test]
    fn paused_restart_replaces_only_its_take_and_resets_elapsed_time() {
        let root = tempfile::tempdir().unwrap();
        let unrelated = root.path().join("unrelated-recovery-bundle");
        std::fs::create_dir(&unrelated).unwrap();
        std::fs::write(unrelated.join("segment-000.mp4"), b"other take").unwrap();
        let display = display();
        let mut session =
            RecordingSession::prepare(root.path().into(), options(&display), display).unwrap();
        session
            .transition(RecordingState::Recording, now_ms().saturating_sub(2_000))
            .unwrap();
        session.pause().unwrap();
        let expected_options = session.manifest.options.clone();
        for (path, bytes) in [
            ("segment-000.mp4", b"old video".as_slice()),
            ("segment-000-system.wav", b"old system audio".as_slice()),
            ("segment-000-microphone.wav", b"old microphone".as_slice()),
            ("assembled.mp4", b"old assembly".as_slice()),
            ("poster.png", b"old poster".as_slice()),
        ] {
            std::fs::write(session.directory.join(path), bytes).unwrap();
        }
        session.manifest.segments.push(RecordingSegmentManifest {
            index: 0,
            relative_path: "segment-000.mp4".into(),
            system_audio_relative_path: Some("segment-000-system.wav".into()),
            system_audio_offset_ms: 0,
            system_audio_warning: Some("old warning".into()),
            microphone_relative_path: Some("segment-000-microphone.wav".into()),
            microphone_offset_ms: 0,
            microphone_warning: None,
            started_at_ms: 1,
            duration_ms: 2_000,
            width: 310,
            height: 170,
            size_bytes: 9,
            dropped_frames: 0,
            complete: true,
        });
        session.store.save(&session.manifest).unwrap();

        let restarted = session.restart().unwrap();

        assert_eq!(restarted.state, RecordingState::Countdown);
        assert_eq!(restarted.elapsed_ms, 0);
        assert_eq!(restarted.options, expected_options);
        assert!(session.manifest.segments.is_empty());
        assert_eq!(session.manifest.last_error, None);
        for path in [
            "segment-000.mp4",
            "segment-000-system.wav",
            "segment-000-microphone.wav",
            "assembled.mp4",
            "poster.png",
        ] {
            assert!(
                !session.directory.join(path).exists(),
                "{path} was retained"
            );
        }
        assert_eq!(
            std::fs::read(unrelated.join("segment-000.mp4")).unwrap(),
            b"other take"
        );
        assert_eq!(
            session.store.load(&session.manifest.session_id).unwrap(),
            session.manifest
        );
    }

    #[test]
    fn restart_reports_cleanup_failure_without_forgetting_old_media() {
        let root = tempfile::tempdir().unwrap();
        let display = display();
        let mut session =
            RecordingSession::prepare(root.path().into(), options(&display), display).unwrap();
        session
            .transition(RecordingState::Recording, now_ms().saturating_sub(500))
            .unwrap();
        session.pause().unwrap();
        session.manifest.segments.push(RecordingSegmentManifest {
            index: 0,
            relative_path: "segment-000.mp4".into(),
            system_audio_relative_path: None,
            system_audio_offset_ms: 0,
            system_audio_warning: None,
            microphone_relative_path: None,
            microphone_offset_ms: 0,
            microphone_warning: None,
            started_at_ms: 1,
            duration_ms: 500,
            width: 310,
            height: 170,
            size_bytes: 1,
            dropped_frames: 0,
            complete: true,
        });
        let old_media = session.directory.join("segment-000.mp4");
        std::fs::create_dir(&old_media).unwrap();
        session.store.save(&session.manifest).unwrap();

        let error = session.restart().unwrap_err();

        assert!(error.contains("Could not remove replaced recording media"));
        assert_eq!(session.manifest.state, RecordingState::Failed);
        assert_eq!(session.manifest.segments.len(), 1);
        assert!(old_media.is_dir());
        assert_eq!(
            session.store.load(&session.manifest.session_id).unwrap(),
            session.manifest
        );
    }

    #[test]
    fn invalid_options_do_not_create_a_recovery_bundle() {
        let root = tempfile::tempdir().unwrap();
        let display = display();
        let mut options = options(&display);
        options.frames_per_second = 17;
        assert!(RecordingSession::prepare(root.path().join("absent"), options, display).is_err());
        assert!(!root.path().join("absent").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires private Xvfb, hsetroot, ffmpeg; records only the disposable X11 display"]
    fn private_x11_records_pause_resume_pixels_and_cancelled_start() {
        use std::{cell::Cell, process::Command, thread, time::Duration};

        assert_eq!(
            std::env::var("CAPTURES_TEST_PRIVATE_X11").as_deref(),
            Ok("1")
        );
        assert_eq!(std::env::var("XDG_SESSION_TYPE").as_deref(), Ok("x11"));
        assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
        let display = captures_capture::XcapBackend.displays().unwrap().remove(0);
        let root = tempfile::tempdir().unwrap();
        let set_background = |color: &str| {
            assert!(
                Command::new("hsetroot")
                    .args(["-solid", color])
                    .status()
                    .unwrap()
                    .success()
            );
        };
        let first_pixel = |path: &Path| {
            let frame = Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(path)
                .args([
                    "-frames:v",
                    "1",
                    "-vf",
                    "crop=2:2:40:40",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "-",
                ])
                .output()
                .unwrap();
            assert!(
                frame.status.success(),
                "{}",
                String::from_utf8_lossy(&frame.stderr)
            );
            assert_eq!(frame.stdout.len(), 12);
            [frame.stdout[0], frame.stdout[1], frame.stdout[2]]
        };

        // Restarting from both Paused and Recording must remove the replaced
        // pixels, reset elapsed time, and reuse the original target/options.
        let mut restarted =
            RecordingSession::prepare(root.path().into(), options(&display), display.clone())
                .unwrap();
        let replaced_path = restarted.directory.join("segment-000.mp4");
        set_background("#c02040");
        restarted.start(false, || true).unwrap();
        thread::sleep(Duration::from_millis(350));
        restarted.pause().unwrap();
        assert!(restarted.snapshot().elapsed_ms > 0);
        assert!(replaced_path.is_file());
        let paused_restart = restarted.restart().unwrap();
        assert_eq!(paused_restart.state, RecordingState::Countdown);
        assert_eq!(paused_restart.elapsed_ms, 0);
        assert!(!replaced_path.exists());

        set_background("#2070c0");
        restarted.start(false, || true).unwrap();
        thread::sleep(Duration::from_millis(350));
        assert!(restarted.snapshot().elapsed_ms > 0);
        let running_restart = restarted.restart().unwrap();
        assert_eq!(running_restart.state, RecordingState::Countdown);
        assert_eq!(running_restart.elapsed_ms, 0);
        assert!(!replaced_path.exists());

        set_background("#20a050");
        restarted.start(false, || true).unwrap();
        thread::sleep(Duration::from_millis(350));
        restarted.pause().unwrap();
        let pixel = first_pixel(&replaced_path);
        for (actual, expected) in pixel.into_iter().zip([32u8, 160, 80]) {
            assert!(actual.abs_diff(expected) <= 6, "encoded pixel {pixel:?}");
        }
        restarted.discard().unwrap();

        let mut session =
            RecordingSession::prepare(root.path().into(), options(&display), display.clone())
                .unwrap();
        for (index, color) in ["#c02040", "#2070c0"].into_iter().enumerate() {
            set_background(color);
            assert_eq!(
                session.start(false, || true).unwrap().state,
                RecordingState::Recording
            );
            assert!(
                session.start(false, || true).is_err(),
                "duplicate Start must not open another engine"
            );
            thread::sleep(Duration::from_millis(350));
            assert_eq!(session.pause().unwrap().state, RecordingState::Paused);
            assert!(session.active.is_none());
            let segment = &session.manifest.segments[index];
            assert!(segment.complete && segment.duration_ms > 0 && segment.size_bytes > 0);
            assert_eq!((segment.width, segment.height), (310, 170));
            let pixel = first_pixel(&session.directory.join(&segment.relative_path));
            let expected = if index == 0 {
                [192u8, 32, 64]
            } else {
                [32u8, 112, 192]
            };
            for (actual, expected) in pixel.into_iter().zip(expected) {
                assert!(
                    actual.abs_diff(expected) <= 6,
                    "encoded pixel {pixel:?}, expected {expected}"
                );
            }
            assert_eq!(
                session.store.load(&session.manifest.session_id).unwrap(),
                session.manifest
            );
        }
        // Losing the selector generation during Resume must discard only the
        // newly opened engine, never either completed segment of this take.
        for after_open in [false, true] {
            let before = session.manifest.clone();
            let media: Vec<_> = before
                .segments
                .iter()
                .map(|segment| {
                    std::fs::read(session.directory.join(&segment.relative_path)).unwrap()
                })
                .collect();
            let calls = Cell::new(0);
            assert_eq!(
                session
                    .start(false, || {
                        calls.set(calls.get() + 1);
                        after_open && calls.get() == 1
                    })
                    .unwrap_err(),
                "Recording cancelled"
            );
            assert_eq!(calls.get(), if after_open { 2 } else { 1 });
            assert_eq!(session.snapshot().state, RecordingState::Paused);
            assert!(session.active.is_none());
            assert_eq!(session.manifest, before);
            assert_eq!(session.store.load(&before.session_id).unwrap(), before);
            for (segment, bytes) in before.segments.iter().zip(media) {
                assert_eq!(
                    std::fs::read(session.directory.join(&segment.relative_path)).unwrap(),
                    bytes
                );
            }
            assert!(!session.directory.join("segment-002.mp4").exists());
        }
        // A later healthy segment must not hide an earlier audio failure.
        session.manifest.segments[0].microphone_warning = Some("Microphone disconnected".into());
        assert_eq!(
            session.snapshot().warning.as_deref(),
            Some("Microphone disconnected")
        );
        session.manifest.segments[0].microphone_warning = None;
        assert!(session.snapshot().warning.is_none());
        assert_eq!(session.stop().unwrap().state, RecordingState::Finalizing);
        assert_eq!(session.manifest.segments.len(), 2);
        assert!(
            session.discard().is_err(),
            "finalizing media must remain recoverable"
        );
        assert!(session.directory.join("segment-000.mp4").is_file());
        assert!(session.directory.join("segment-001.mp4").is_file());

        // Reuse the real captured segments to distinguish assembly cancellation,
        // publication failure, and housekeeping failure after a successful save.
        let copy_for_finalization = |kind| {
            let mut options = options(&display);
            options.kind = kind;
            // Independent finalization fixtures cannot share the live take's
            // recovery-root lease; they own disposable bundles in separate roots.
            let copy_root = root.path().join(format!("copy-{}", uuid::Uuid::new_v4()));
            let mut copy = RecordingSession::prepare(copy_root, options, display.clone()).unwrap();
            copy.manifest.segments = session.manifest.segments.clone();
            for segment in &copy.manifest.segments {
                std::fs::copy(
                    session.directory.join(&segment.relative_path),
                    copy.directory.join(&segment.relative_path),
                )
                .unwrap();
            }
            copy.transition(RecordingState::Recording, now_ms())
                .unwrap();
            copy.stop().unwrap();
            copy
        };
        let tools = MediaToolchain::from_command_names();
        let mut interrupted = copy_for_finalization(RecordingKind::Video);
        let token = CancelToken::default();
        token.cancel();
        let absent_history = root.path().join("cancelled-history");
        assert!(
            interrupted
                .finish(&absent_history, &tools, &token)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(!absent_history.exists());
        assert_eq!(interrupted.snapshot().state, RecordingState::Failed);
        assert!(interrupted.directory.join("segment-000.mp4").is_file());
        assert!(interrupted.directory.join("segment-001.mp4").is_file());
        assert_eq!(
            interrupted
                .store
                .load(&interrupted.manifest.session_id)
                .unwrap(),
            interrupted.manifest
        );

        let mut unpublished = copy_for_finalization(RecordingKind::Video);
        let blocked_history = root.path().join("blocked-history");
        std::fs::write(&blocked_history, b"keep").unwrap();
        assert!(
            unpublished
                .finish(&blocked_history, &tools, &CancelToken::default())
                .is_err()
        );
        assert_eq!(unpublished.snapshot().state, RecordingState::Failed);
        assert!(unpublished.manifest.final_path.is_none());
        assert!(unpublished.directory.join("segment-000.mp4").is_file());
        assert_eq!(std::fs::read(&blocked_history).unwrap(), b"keep");

        let mut published = copy_for_finalization(RecordingKind::Video);
        let manifest_path = published.directory.join("manifest.json");
        std::fs::remove_file(&manifest_path).unwrap();
        std::fs::create_dir(&manifest_path).unwrap();
        let result = published
            .finish(
                &root.path().join("warning-history"),
                &tools,
                &CancelToken::default(),
            )
            .unwrap();
        assert!(result.path.is_file());
        assert!(
            result
                .warning
                .unwrap()
                .contains("could not save recovery state")
        );
        assert_eq!(published.snapshot().state, RecordingState::Ready);
        assert!(
            published.directory.join("segment-000.mp4").is_file(),
            "failed recovery metadata write must not delete sources"
        );

        let mut gif = copy_for_finalization(RecordingKind::Gif);
        let result = gif
            .finish(
                &root.path().join("gif-history"),
                &tools,
                &CancelToken::default(),
            )
            .unwrap();
        assert!(result.warning.is_none());
        assert_eq!(result.entry.mime_type.as_deref(), Some("image/gif"));
        assert_eq!((result.entry.width, result.entry.height), (310, 170));
        assert!(!result.entry.has_system_audio && !result.entry.has_microphone_audio);
        assert_eq!(gif.snapshot().state, RecordingState::Ready);
        assert!(
            gif.directory.join("master.mp4").is_file(),
            "GIF keeps editable master"
        );
        assert!(gif.directory.join("segment-000.mp4").is_file());

        let history = root.path().join("history");
        let saved = session
            .finish(&history, &tools, &CancelToken::default())
            .unwrap();
        assert_eq!(session.snapshot().state, RecordingState::Ready);
        assert!(saved.warning.is_none());
        assert!(
            !session.directory.exists(),
            "video sources removed only after publication"
        );
        assert_eq!((saved.entry.width, saved.entry.height), (310, 170));
        assert!(saved.entry.duration_ms.unwrap() >= 400);
        assert!(!saved.entry.has_system_audio && !saved.entry.has_microphone_audio);
        assert_eq!(saved.entry.target, Some(options(&display).target));
        assert_eq!(
            saved.entry.saved_path, None,
            "private History is not permanent Save"
        );
        assert_eq!(saved.path, history.join(&saved.entry.id).join("media.mp4"));
        assert!(history.join(&saved.entry.id).join("preview.png").is_file());
        assert_eq!(
            captures_history::load(&history, chrono::Utc::now()).unwrap()[0].id,
            saved.entry.id
        );
        assert_eq!(
            tools.probe(&saved.path).unwrap().metadata.size_bytes,
            saved.entry.size_bytes
        );
        let frames = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&saved.path)
            .args([
                "-vf",
                "crop=2:2:40:40",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-",
            ])
            .output()
            .unwrap();
        assert!(
            frames.status.success(),
            "{}",
            String::from_utf8_lossy(&frames.stderr)
        );
        assert!(frames.stdout.len() >= 24);
        // Both segments must survive in order, not just produce a valid MP4.
        for (pixel, expected) in [
            (&frames.stdout[..3], [192u8, 32, 64]),
            (&frames.stdout[frames.stdout.len() - 3..], [32u8, 112, 192]),
        ] {
            for (actual, expected) in pixel.iter().zip(expected) {
                assert!(
                    actual.abs_diff(expected) <= 6,
                    "published pixel {pixel:?} expected {expected}"
                );
            }
        }
        assert!(
            session
                .finish(&history, &tools, &CancelToken::default())
                .is_err()
        );

        // A failed Finalizing manifest write must still stop the running engine
        // and leave completed media, rather than record invisibly or delete it.
        let mut failing =
            RecordingSession::prepare(root.path().into(), options(&display), display.clone())
                .unwrap();
        failing.start(false, || true).unwrap();
        thread::sleep(Duration::from_millis(200));
        let manifest_path = failing.directory.join("manifest.json");
        std::fs::remove_file(&manifest_path).unwrap();
        std::fs::create_dir(&manifest_path).unwrap();
        assert!(
            failing
                .stop()
                .unwrap_err()
                .contains("could not save recording recovery state")
        );
        assert_eq!(failing.snapshot().state, RecordingState::Failed);
        assert!(failing.active.is_none());
        assert!(failing.manifest.segments[0].complete);
        assert!(failing.directory.join("segment-000.mp4").is_file());

        let mut cancelled = RecordingSession::prepare(
            root.path().join("cancelled-recovery-root"),
            options(&display),
            display,
        )
        .unwrap();
        let cancelled_directory = cancelled.directory.clone();
        let calls = Cell::new(0);
        assert_eq!(
            cancelled
                .start(false, || {
                    calls.set(calls.get() + 1);
                    calls.get() == 1
                })
                .unwrap_err(),
            "Recording cancelled"
        );
        assert_eq!(calls.get(), 2, "cancel after the real engine opened");
        assert!(!cancelled_directory.exists());
        assert!(cancelled.active.is_none());
        assert_eq!(cancelled.snapshot().state, RecordingState::Discarded);
    }
}
