use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use captures_capture::DisplayDescriptor;
use captures_media::{
    ByteRange, CancelToken, EditSpec, ExportFormat, ExportProgress, ExportSpec, MediaToolchain,
    RecordingAudioLayout, RecordingSegmentInput, TimelineSpriteSpec,
};
use captures_recording::{
    DraftStore, RecordingCoordinator, RecordingDraftManifest, RecordingKind, RecordingOptions,
    RecordingSegmentManifest, RecordingSessionSnapshot, RecordingState, RecordingTarget,
};
use captures_recording_macos::{MacRecordingSegment, SegmentInfo};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, window::Color};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::{
    AppError,
    models::{
        HistoryEntry, RecordingArtifact, RecordingArtifactData, RecordingSelection,
        RecordingSelectionSession, recording_media_url, recording_poster_url,
        recording_recovery_directory, recording_timeline_url,
    },
    state::AppState,
    storage,
};

#[cfg(target_os = "macos")]
use crate::models::recording_selection_url;
#[cfg(target_os = "macos")]
use tauri::LogicalSize;

const RECORDING_STATE_EVENT: &str = "recording-state-changed";
const RECORDING_COUNTDOWN_EVENT: &str = "recording-countdown";
const RECORDING_WARNING_EVENT: &str = "recording-warning";
const RECORDING_ARTIFACT_EVENT: &str = "recording-artifact-ready";
const RECORDING_COUNTDOWN_FADE_OUT_MS: u64 = 180;
const RECORDING_HUD_FULL_WIDTH: f64 = 430.0;
const RECORDING_HUD_HEIGHT: f64 = 126.0;
const RECORDING_HUD_BOTTOM_MARGIN: f64 = 20.0;
const GIF_SOURCE_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Default)]
pub struct RecordingRuntime {
    coordinator: RecordingCoordinator,
    session: Option<RuntimeSession>,
    generation: u64,
    exports: HashMap<String, CancelToken>,
}

struct RuntimeSession {
    id: String,
    options: RecordingOptions,
    directory: PathBuf,
    manifest: RecordingDraftManifest,
    active_segment: Option<MacRecordingSegment>,
    active_segment_started_at_ms: Option<u64>,
    poster_png: Vec<u8>,
    display: DisplayDescriptor,
}

pub fn screenshot_capture_is_blocked(state: &AppState) -> bool {
    let selection_is_active = state.recording_selection.lock().is_some();
    let recording_state = state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .map(|snapshot| snapshot.state);
    screenshot_capture_is_blocked_for(selection_is_active, recording_state)
}

const fn screenshot_capture_is_blocked_for(
    selection_is_active: bool,
    recording_state: Option<RecordingState>,
) -> bool {
    selection_is_active
        || matches!(
            recording_state,
            Some(
                RecordingState::Selecting
                    | RecordingState::Countdown
                    | RecordingState::Finalizing
                    | RecordingState::Editor
            )
        )
}

#[cfg(target_os = "macos")]
fn recording_session_is_active(state: &AppState) -> bool {
    state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .is_some_and(|snapshot| !snapshot.state.is_terminal())
}

#[cfg(target_os = "macos")]
pub(crate) fn recording_controls_are_available(state: &AppState) -> bool {
    state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .is_some_and(|snapshot| {
            matches!(
                snapshot.state,
                RecordingState::Recording | RecordingState::Paused
            )
        })
}

#[derive(Clone, Debug, Deserialize)]
pub struct StartRecordingRequest {
    pub selection_id: String,
    pub options: RecordingOptions,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingCountdown {
    pub session_id: String,
    pub remaining_seconds: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingWarning {
    pub session_id: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingAudioLevel {
    pub session_id: String,
    pub microphone_peak: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StartExportRequest {
    pub artifact_id: String,
    pub file_stem: String,
    #[serde(default)]
    pub destination_directory: Option<String>,
    #[serde(default)]
    pub overwrite_source: bool,
    pub edit: EditSpec,
    pub export: ExportSpec,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingExportProgress {
    pub export_id: String,
    pub progress: ExportProgress,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingExportComplete {
    pub export_id: String,
    pub artifact: RecordingArtifact,
    pub finder_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingExportFailed {
    pub export_id: String,
    pub message: String,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingTimelinePreview {
    pub url: String,
    pub frame_count: u16,
    pub frame_width: u32,
    pub frame_height: u32,
    pub sprite_width: u32,
    pub sprite_height: u32,
}

#[tauri::command]
pub async fn prepare_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<RecordingSelectionSession, String> {
    prepare_recording_inner(app, state.inner().clone())
        .await
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
pub async fn prepare_recording_inner(
    _app: AppHandle,
    _state: Arc<AppState>,
) -> Result<RecordingSelectionSession, AppError> {
    Err(AppError::Task(
        "screen recording is currently available on macOS only".to_owned(),
    ))
}

#[cfg(target_os = "macos")]
pub async fn prepare_recording_inner(
    app: AppHandle,
    state: Arc<AppState>,
) -> Result<RecordingSelectionSession, AppError> {
    if crate::updates::install_is_active(&app) {
        return Err(AppError::UpdateInstalling);
    }
    if !state.sessions.lock().is_empty() || recording_session_is_active(&state) {
        return Err(AppError::CaptureInProgress);
    }
    let pending_selection = state
        .recording_selection
        .lock()
        .as_ref()
        .map(|selection| selection.summary.clone());
    if let Some(summary) = pending_selection {
        crate::set_capture_huds_protected(&app, true);
        crate::hide_window(&app, "thumbnail");
        crate::hide_window(&app, "startup");
        crate::hide_window(&app, "update");
        if let Err(error) = prepare_recording_selector(&app, &summary).await {
            *state.recording_selection.lock() = None;
            restore_recording_ui(&app, &state);
            return Err(error);
        }
        return Ok(summary);
    }

    let request_permission = crate::mark_screen_permission_request(&state)?;
    if let Err(error) = state.backend.ensure_permission(request_permission) {
        if matches!(
            &error,
            captures_capture::CaptureError::PermissionRequestStarted
        ) {
            *state.screen_permission_requested_this_launch.lock() = true;
        }
        return Err(error.into());
    }

    crate::set_capture_huds_protected(&app, true);
    crate::hide_window(&app, "thumbnail");
    crate::hide_window(&app, "startup");
    crate::hide_window(&app, "update");
    let prepared = (|| {
        let display = crate::display_under_pointer(&state)?;
        let frame = state.backend.capture_display(&display.id)?;
        let snapshot_png = storage::encode_png(&frame.image)?;
        let windows = state
            .windows()?
            .into_iter()
            .filter(|window| crate::window_is_capturable(window, &display))
            .collect::<Vec<_>>();
        let id = Uuid::new_v4().to_string();
        let summary = RecordingSelectionSession {
            id: id.clone(),
            // Every capture starts from a high-quality video master. The editor
            // decides whether the final copy is video or GIF.
            kind: RecordingKind::Video,
            window_coordinate_scale: crate::window_coordinate_scale(&frame.descriptor),
            display: frame.descriptor,
            snapshot_url: recording_selection_url(&id),
            windows,
        };
        *state.recording_selection.lock() = Some(RecordingSelection {
            summary: summary.clone(),
            image: frame.image,
            snapshot_png,
        });
        Ok::<_, AppError>(summary)
    })();
    match prepared {
        Ok(summary) => {
            if let Err(error) = prepare_recording_selector(&app, &summary).await {
                *state.recording_selection.lock() = None;
                restore_recording_ui(&app, &state);
                return Err(error);
            }
            Ok(summary)
        }
        Err(error) => {
            *state.recording_selection.lock() = None;
            restore_recording_ui(&app, &state);
            Err(error)
        }
    }
}

#[tauri::command]
pub fn get_recording_selection(
    state: tauri::State<'_, Arc<AppState>>,
) -> Option<RecordingSelectionSession> {
    state
        .recording_selection
        .lock()
        .as_ref()
        .map(|selection| selection.summary.clone())
}

#[tauri::command]
pub fn cancel_recording_selection(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    selection_id: String,
) -> Result<(), String> {
    cancel_recording_selection_inner(&app, state.inner(), &selection_id)
        .map_err(|error| error.to_string())
}

fn cancel_recording_selection_inner(
    app: &AppHandle,
    state: &Arc<AppState>,
    selection_id: &str,
) -> Result<(), AppError> {
    let mut selection = state.recording_selection.lock();
    let matches = selection
        .as_ref()
        .is_some_and(|selection| selection.summary.id == selection_id);
    if !matches {
        return Err(AppError::SessionUnavailable);
    }
    *selection = None;
    drop(selection);
    destroy_recording_selector(app);
    crate::set_capture_huds_protected(app, false);
    crate::restore_thumbnail_stack(app, state);
    Ok(())
}

#[tauri::command]
pub fn list_recording_audio_devices() -> Vec<captures_recording::AudioDevice> {
    captures_recording_macos::microphone_devices()
}

#[tauri::command]
pub fn get_recording_snapshot(
    state: tauri::State<'_, Arc<AppState>>,
) -> Option<RecordingSessionSnapshot> {
    state.recording.lock().coordinator.snapshot(now_ms())
}

#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    request: StartRecordingRequest,
) -> Result<RecordingSessionSnapshot, String> {
    start_recording_inner(app, state.inner().clone(), request)
        .await
        .map_err(|error| error.to_string())
}

async fn start_recording_inner(
    app: AppHandle,
    state: Arc<AppState>,
    request: StartRecordingRequest,
) -> Result<RecordingSessionSnapshot, AppError> {
    request
        .options
        .validate()
        .map_err(|error| AppError::Task(error.to_owned()))?;
    let selection = {
        let mut selected = state.recording_selection.lock();
        let matches = selected
            .as_ref()
            .is_some_and(|selection| selection.summary.id == request.selection_id);
        if !matches {
            return Err(AppError::SessionUnavailable);
        }
        selected.take().ok_or(AppError::SessionUnavailable)?
    };
    destroy_recording_selector(&app);
    crate::set_capture_huds_protected(&app, false);

    let selected_display = selection.summary.display.clone();
    let initialized = (|| {
        validate_target(&selection.summary, &request.options.target)?;
        let poster_png = poster_for_selection(&selection, &request.options.target)?;
        initialize_recording_session(
            &state,
            request.options,
            poster_png,
            selected_display.clone(),
        )
    })();
    let (snapshot, generation) = match initialized {
        Ok(initialized) => initialized,
        Err(error) => {
            restore_recording_ui(&app, &state);
            return Err(error);
        }
    };
    emit_snapshot(&app, &snapshot);
    if let Err(error) = prepare_recording_hud(&app, &selected_display).and_then(|()| {
        if snapshot.options.countdown_seconds > 0 {
            show_recording_countdown(&app, &selected_display)
        } else {
            Ok(())
        }
    }) {
        fail_session(&app, &state, &snapshot.id, error.to_string());
        restore_recording_ui(&app, &state);
        return Err(error);
    }
    schedule_countdown(
        app,
        state,
        snapshot.id.clone(),
        generation,
        snapshot.options.countdown_seconds,
    );
    Ok(snapshot)
}

fn initialize_recording_session(
    state: &AppState,
    options: RecordingOptions,
    poster_png: Vec<u8>,
    display: DisplayDescriptor,
) -> Result<(RecordingSessionSnapshot, u64), AppError> {
    let now = now_ms();
    let mut runtime = state.recording.lock();
    let initial = runtime
        .coordinator
        .begin(options.clone(), now)
        .map_err(|error| AppError::Task(error.to_string()))?;
    let mut manifest = RecordingDraftManifest::new(initial.id.clone(), options.clone(), now);
    let store = DraftStore::new(recording_recovery_directory());
    let mut draft_created = false;
    let setup = (|| {
        let directory = store
            .create(&manifest)
            .map_err(|error| AppError::Task(error.to_string()))?;
        draft_created = true;
        fs::write(directory.join("poster.png"), &poster_png)?;
        let snapshot = runtime
            .coordinator
            .transition(&initial.id, RecordingState::Countdown, now)
            .map_err(|error| AppError::Task(error.to_string()))?;
        manifest.state = RecordingState::Countdown;
        manifest.updated_at_ms = now;
        store
            .save(&manifest)
            .map_err(|error| AppError::Task(error.to_string()))?;
        Ok::<_, AppError>((snapshot, directory))
    })();
    let (snapshot, directory) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            let message = error.to_string();
            let failed_at = now_ms();
            let _ = runtime
                .coordinator
                .fail(&initial.id, message.clone(), failed_at);
            if draft_created {
                manifest.state = RecordingState::Failed;
                manifest.updated_at_ms = failed_at;
                manifest.last_error = Some(message);
                let _ = store.save(&manifest);
            }
            return Err(error);
        }
    };
    runtime.generation = runtime.generation.wrapping_add(1);
    let generation = runtime.generation;
    runtime.session = Some(RuntimeSession {
        id: snapshot.id.clone(),
        options,
        directory,
        manifest,
        active_segment: None,
        active_segment_started_at_ms: None,
        poster_png,
        display,
    });
    Ok((snapshot, generation))
}

fn schedule_countdown(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: String,
    generation: u64,
    seconds: u8,
) {
    tauri::async_runtime::spawn(async move {
        for remaining in (1..=seconds).rev() {
            if !countdown_is_current(&state, &session_id, generation) {
                return;
            }
            let _ = app.emit(
                RECORDING_COUNTDOWN_EVENT,
                RecordingCountdown {
                    session_id: session_id.clone(),
                    remaining_seconds: remaining,
                },
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if let Err(error) = start_segment(app.clone(), state.clone(), &session_id, generation).await
        {
            fail_session(&app, &state, &session_id, error.to_string());
        }
    });
}

fn countdown_is_current(state: &AppState, session_id: &str, generation: u64) -> bool {
    let runtime = state.recording.lock();
    runtime.generation == generation
        && runtime
            .session
            .as_ref()
            .is_some_and(|session| session.id == session_id)
        && runtime
            .coordinator
            .snapshot(now_ms())
            .is_some_and(|snapshot| snapshot.state == RecordingState::Countdown)
}

fn recording_segment_is_current(state: &AppState, session_id: &str, generation: u64) -> bool {
    let runtime = state.recording.lock();
    runtime.generation == generation
        && runtime
            .session
            .as_ref()
            .is_some_and(|session| session.id == session_id)
        && runtime
            .coordinator
            .snapshot(now_ms())
            .is_some_and(|snapshot| snapshot.state == RecordingState::Recording)
}

async fn start_segment(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: &str,
    generation: u64,
) -> Result<(), AppError> {
    let (options, path) = {
        let runtime = state.recording.lock();
        if runtime.generation != generation {
            return Ok(());
        }
        let session = runtime
            .session
            .as_ref()
            .filter(|session| session.id == session_id)
            .ok_or(AppError::SessionUnavailable)?;
        let snapshot = runtime
            .coordinator
            .snapshot(now_ms())
            .ok_or(AppError::SessionUnavailable)?;
        if !matches!(
            snapshot.state,
            RecordingState::Countdown | RecordingState::Paused
        ) {
            return Ok(());
        }
        let index = session.manifest.segments.len();
        (
            session.options.clone(),
            session.directory.join(format!("segment-{index:03}.mp4")),
        )
    };

    let path_for_start = path.clone();
    let started = tauri::async_runtime::spawn_blocking(move || {
        MacRecordingSegment::start(&options, &path_for_start)
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))?;
    let segment = match started {
        Ok(segment) => segment,
        Err(error) => return Err(AppError::Task(error.to_string())),
    };

    let now = now_ms();
    let mut segment = Some(segment);
    let snapshot = {
        let mut runtime = state.recording.lock();
        let still_current = runtime.generation == generation
            && runtime
                .session
                .as_ref()
                .is_some_and(|session| session.id == session_id)
            && runtime.coordinator.snapshot(now).is_some_and(|snapshot| {
                matches!(
                    snapshot.state,
                    RecordingState::Countdown | RecordingState::Paused
                )
            });
        if !still_current {
            None
        } else {
            let started_from_countdown = runtime
                .coordinator
                .snapshot(now)
                .is_some_and(|snapshot| snapshot.state == RecordingState::Countdown);
            let dimensions = segment.as_ref().map(MacRecordingSegment::dimensions);
            let microphone_draft = segment
                .as_ref()
                .and_then(MacRecordingSegment::microphone_draft_info);
            let snapshot = runtime
                .coordinator
                .transition(session_id, RecordingState::Recording, now)
                .map_err(|error| AppError::Task(error.to_string()))?;
            let session = runtime
                .session
                .as_mut()
                .ok_or(AppError::SessionUnavailable)?;
            session.active_segment = segment.take();
            session.active_segment_started_at_ms = Some(now);
            let relative_path = path
                .strip_prefix(&session.directory)
                .map_err(|_| {
                    AppError::Task("recording segment escaped its recovery bundle".to_owned())
                })?
                .to_string_lossy()
                .into_owned();
            let (microphone_relative_path, microphone_offset_ms) = microphone_draft
                .map(|(microphone_path, offset_ms)| {
                    let relative_path = microphone_path
                        .strip_prefix(&session.directory)
                        .map_err(|_| {
                            AppError::Task(
                                "microphone segment escaped its recovery bundle".to_owned(),
                            )
                        })?
                        .to_string_lossy()
                        .into_owned();
                    Ok::<_, AppError>((Some(relative_path), offset_ms))
                })
                .transpose()?
                .unwrap_or((None, 0));
            let index = u32::try_from(session.manifest.segments.len())
                .map_err(|_| AppError::Task("recording has too many segments".to_owned()))?;
            let (width, height) = dimensions.unwrap_or_default();
            session.manifest.segments.push(RecordingSegmentManifest {
                index,
                relative_path,
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
            session.manifest.state = RecordingState::Recording;
            session.manifest.updated_at_ms = now;
            save_manifest(&session.manifest)?;
            Some((snapshot, started_from_countdown))
        }
    };
    let Some((snapshot, started_from_countdown)) = snapshot else {
        if let Some(segment) = segment {
            let _ = tauri::async_runtime::spawn_blocking(move || segment.discard()).await;
        }
        return Ok(());
    };
    emit_snapshot(&app, &snapshot);
    if started_from_countdown {
        captures_recording_macos::play_start_chime();
        tokio::time::sleep(Duration::from_millis(RECORDING_COUNTDOWN_FADE_OUT_MS)).await;
        if !recording_segment_is_current(&state, session_id, generation) {
            destroy_recording_countdown(&app);
            return Ok(());
        }
    }
    destroy_recording_countdown(&app);
    show_recording_hud(&app)?;
    schedule_segment_monitor(app, state, session_id.to_owned(), generation);
    Ok(())
}

fn schedule_segment_monitor(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: String,
    generation: u64,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let (level, warning, warning_snapshot) = {
                let mut runtime = state.recording.lock();
                if runtime.generation != generation {
                    return;
                }
                let Some((level, warning)) = runtime
                    .session
                    .as_ref()
                    .filter(|session| session.id == session_id)
                    .and_then(|session| session.active_segment.as_ref())
                    .map(|segment| (segment.microphone_level(), segment.warning()))
                else {
                    return;
                };
                let current_warning = runtime
                    .coordinator
                    .snapshot(now_ms())
                    .and_then(|snapshot| snapshot.warning);
                let warning_snapshot = warning
                    .as_ref()
                    .filter(|warning| current_warning.as_ref() != Some(*warning))
                    .and_then(|warning| {
                        runtime
                            .coordinator
                            .warn(&session_id, Some(warning.clone()), now_ms())
                            .ok()
                    });
                (level, warning, warning_snapshot)
            };
            let _ = app.emit(
                "recording-audio-level",
                RecordingAudioLevel {
                    session_id: session_id.clone(),
                    microphone_peak: level,
                },
            );
            if let (Some(warning), Some(snapshot)) = (warning, warning_snapshot) {
                emit_snapshot(&app, &snapshot);
                let _ = app.emit(
                    RECORDING_WARNING_EVENT,
                    RecordingWarning {
                        session_id: session_id.clone(),
                        message: warning,
                    },
                );
            }
        }
    });
}

#[tauri::command]
pub async fn pause_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<RecordingSessionSnapshot, String> {
    pause_recording_inner(&app, state.inner().clone(), &session_id)
        .await
        .map_err(|error| error.to_string())
}

async fn pause_recording_inner(
    app: &AppHandle,
    state: Arc<AppState>,
    session_id: &str,
) -> Result<RecordingSessionSnapshot, AppError> {
    let result: Result<RecordingSessionSnapshot, AppError> = async {
        let (segment, started_at_ms) =
            take_active_segment(&state, session_id, RecordingState::Recording)?;
        let info = stop_native_segment(segment).await?;
        let now = now_ms();
        let snapshot = {
            let mut runtime = state.recording.lock();
            let snapshot = runtime
                .coordinator
                .transition(session_id, RecordingState::Paused, now)
                .map_err(|error| AppError::Task(error.to_string()))?;
            let session = runtime
                .session
                .as_mut()
                .filter(|session| session.id == session_id)
                .ok_or(AppError::SessionUnavailable)?;
            append_segment(session, info, started_at_ms, now)?;
            session.manifest.state = RecordingState::Paused;
            save_manifest(&session.manifest)?;
            snapshot
        };
        emit_snapshot(app, &snapshot);
        Ok(snapshot)
    }
    .await;
    if let Err(error) = &result {
        let belongs_to_session = state
            .recording
            .lock()
            .coordinator
            .snapshot(now_ms())
            .is_some_and(|snapshot| snapshot.id == session_id && !snapshot.state.is_terminal());
        if belongs_to_session {
            fail_session(app, &state, session_id, error.to_string());
        }
    }
    result
}

#[tauri::command]
pub async fn resume_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<RecordingSessionSnapshot, String> {
    let (generation, snapshot) = {
        let runtime = state.recording.lock();
        let snapshot = runtime
            .coordinator
            .snapshot(now_ms())
            .filter(|snapshot| {
                snapshot.id == session_id && snapshot.state == RecordingState::Paused
            })
            .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
        (runtime.generation, snapshot)
    };
    start_segment(app, state.inner().clone(), &session_id, generation)
        .await
        .map_err(|error| error.to_string())?;
    Ok(state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .unwrap_or(snapshot))
}

#[tauri::command]
pub async fn restart_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<RecordingSessionSnapshot, String> {
    restart_recording_inner(app, state.inner().clone(), &session_id)
        .await
        .map_err(|error| error.to_string())
}

async fn restart_recording_inner(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: &str,
) -> Result<RecordingSessionSnapshot, AppError> {
    let (active, old_segments, countdown, generation, display, snapshot) = {
        let mut runtime = state.recording.lock();
        let now = now_ms();
        let snapshot = runtime
            .coordinator
            .transition(session_id, RecordingState::Countdown, now)
            .map_err(|error| AppError::Task(error.to_string()))?;
        runtime.generation = runtime.generation.wrapping_add(1);
        let generation = runtime.generation;
        let session = runtime
            .session
            .as_mut()
            .filter(|session| session.id == session_id)
            .ok_or(AppError::SessionUnavailable)?;
        let active = session.active_segment.take();
        session.active_segment_started_at_ms = None;
        let old_segments = session
            .manifest
            .segments
            .drain(..)
            .flat_map(|segment| {
                let video = session.directory.join(segment.relative_path);
                let microphone = segment
                    .microphone_relative_path
                    .map(|path| session.directory.join(path));
                [Some(video), microphone].into_iter().flatten()
            })
            .collect::<Vec<_>>();
        session.manifest.state = RecordingState::Countdown;
        session.manifest.updated_at_ms = now;
        session.manifest.last_error = None;
        save_manifest(&session.manifest)?;
        (
            active,
            old_segments,
            session.options.countdown_seconds,
            generation,
            session.display.clone(),
            snapshot,
        )
    };
    if let Some(active) = active {
        let _ = tauri::async_runtime::spawn_blocking(move || active.discard()).await;
    }
    for path in old_segments {
        let _ = fs::remove_file(path);
    }
    emit_snapshot(&app, &snapshot);
    crate::hide_window(&app, "recording-hud");
    if countdown > 0 {
        show_recording_countdown(&app, &display)?;
    }
    schedule_countdown(app, state, session_id.to_owned(), generation, countdown);
    Ok(snapshot)
}

#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<RecordingArtifact, String> {
    let inner = state.inner().clone();
    match stop_recording_inner(app.clone(), inner.clone(), &session_id).await {
        Ok(artifact) => Ok(artifact),
        Err(error) => {
            fail_session(&app, &inner, &session_id, error.to_string());
            Err(error.to_string())
        }
    }
}

async fn stop_recording_inner(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: &str,
) -> Result<RecordingArtifact, AppError> {
    let (active, started_at_ms, finalizing) = {
        let mut runtime = state.recording.lock();
        let now = now_ms();
        let current = runtime
            .coordinator
            .snapshot(now)
            .filter(|snapshot| snapshot.id == session_id)
            .ok_or(AppError::SessionUnavailable)?;
        if !matches!(
            current.state,
            RecordingState::Recording | RecordingState::Paused
        ) {
            return Err(AppError::Task(format!(
                "cannot stop a recording while it is {:?}",
                current.state
            )));
        }
        let finalizing = runtime
            .coordinator
            .transition(session_id, RecordingState::Finalizing, now)
            .map_err(|error| AppError::Task(error.to_string()))?;
        let session = runtime
            .session
            .as_mut()
            .ok_or(AppError::SessionUnavailable)?;
        session.manifest.state = RecordingState::Finalizing;
        session.manifest.updated_at_ms = now;
        save_manifest(&session.manifest)?;
        (
            session.active_segment.take(),
            session.active_segment_started_at_ms.take(),
            finalizing,
        )
    };
    emit_snapshot(&app, &finalizing);

    if let Some(segment) = active {
        let info = stop_native_segment(segment).await?;
        let now = now_ms();
        let mut runtime = state.recording.lock();
        let session = runtime
            .session
            .as_mut()
            .filter(|session| session.id == session_id)
            .ok_or(AppError::SessionUnavailable)?;
        append_segment(session, info, started_at_ms.unwrap_or(now), now)?;
        save_manifest(&session.manifest)?;
    }

    let (options, segments, directory, poster_png) = {
        let runtime = state.recording.lock();
        let session = runtime
            .session
            .as_ref()
            .filter(|session| session.id == session_id)
            .ok_or(AppError::SessionUnavailable)?;
        if session.manifest.segments.is_empty() {
            return Err(AppError::Task(
                "the recording did not contain any media".to_owned(),
            ));
        }
        (
            session.options.clone(),
            session
                .manifest
                .segments
                .iter()
                .map(|segment| RecordingSegmentInput {
                    video_path: session.directory.join(&segment.relative_path),
                    microphone_path: segment
                        .microphone_relative_path
                        .as_ref()
                        .map(|path| session.directory.join(path)),
                    microphone_offset_ms: segment.microphone_offset_ms,
                    duration_ms: segment.duration_ms,
                })
                .collect::<Vec<_>>(),
            session.directory.clone(),
            session.poster_png.clone(),
        )
    };
    let has_microphone_audio = options.kind == RecordingKind::Video
        && segments
            .iter()
            .any(|segment| segment.microphone_path.is_some());
    let settings = state.settings();
    let destination = storage::unique_media_path(
        Path::new(&settings.output_directory),
        if options.kind == RecordingKind::Video {
            "mp4"
        } else {
            "gif"
        },
    )?;
    let toolchain = media_toolchain(&app);
    let cancel = CancelToken::default();
    let destination_for_task = destination.clone();
    let options_for_task = options.clone();
    let directory_for_task = directory.clone();
    let probe = tauri::async_runtime::spawn_blocking(move || {
        if options_for_task.kind == RecordingKind::Video {
            toolchain.assemble_recording_segments(
                &segments,
                &destination_for_task,
                RecordingAudioLayout {
                    system_audio: options_for_task.audio.capture_system_audio,
                    microphone_audio: has_microphone_audio,
                },
                &cancel,
            )?;
        } else {
            let paths = segments
                .iter()
                .map(|segment| segment.video_path.clone())
                .collect::<Vec<_>>();
            let master = if paths.len() == 1 {
                paths[0].clone()
            } else {
                let master = directory_for_task.join("master.mp4");
                if !master.exists() {
                    toolchain.concatenate_segments(&paths, &master, &cancel)?;
                }
                master
            };
            toolchain.create_gif(
                &master,
                &destination_for_task,
                options_for_task.frames_per_second,
                options_for_task.gif.max_width,
                options_for_task.gif.max_colors,
                &cancel,
            )?;
        }
        toolchain.probe(&destination_for_task)
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))?
    .map_err(|error| AppError::Task(error.to_string()))?;

    let artifact_id = Uuid::new_v4().to_string();
    let dropped_frames = state
        .recording
        .lock()
        .session
        .as_ref()
        .filter(|session| session.id == session_id)
        .map(|session| {
            session
                .manifest
                .segments
                .iter()
                .map(|segment| segment.dropped_frames)
                .sum()
        })
        .unwrap_or(0);
    let artifact = RecordingArtifact {
        id: artifact_id.clone(),
        kind: options.kind,
        path: destination.to_string_lossy().into_owned(),
        media_url: recording_media_url(&artifact_id),
        poster_url: recording_poster_url(&artifact_id),
        mime_type: if options.kind == RecordingKind::Video {
            "video/mp4".to_owned()
        } else {
            "image/gif".to_owned()
        },
        duration_ms: probe.metadata.duration_ms.unwrap_or(0),
        width: probe.metadata.width,
        height: probe.metadata.height,
        size_bytes: probe.metadata.size_bytes,
        dropped_frames,
        has_system_audio: options.kind == RecordingKind::Video
            && options.audio.capture_system_audio,
        has_microphone_audio,
        created_at: chrono::Utc::now().to_rfc3339(),
        target: options.target.clone(),
        missing: false,
    };
    upsert_recording_artifact(&app, &state, artifact.clone(), poster_png.clone());
    let _ = fs::write(directory.join("poster.png"), poster_png);

    let now = now_ms();
    let ready = {
        let mut runtime = state.recording.lock();
        let ready = runtime
            .coordinator
            .transition(session_id, RecordingState::Ready, now)
            .map_err(|error| AppError::Task(error.to_string()))?;
        let session = runtime
            .session
            .as_mut()
            .filter(|session| session.id == session_id)
            .ok_or(AppError::SessionUnavailable)?;
        session.manifest.state = RecordingState::Ready;
        session.manifest.updated_at_ms = now;
        session.manifest.final_path = Some(artifact.path.clone());
        save_manifest(&session.manifest)?;
        ready
    };
    emit_snapshot(&app, &ready);
    let _ = app.emit(RECORDING_ARTIFACT_EVENT, &artifact);
    destroy_recording_countdown(&app);
    crate::hide_window(&app, "recording-hud");
    crate::restore_thumbnail_stack(&app, &state);

    if options.kind == RecordingKind::Video {
        let _ = DraftStore::new(recording_recovery_directory()).remove(session_id);
    }
    if settings.recording.open_editor_after_recording
        && let Err(error) = show_recording_editor(&app, &artifact.id)
    {
        eprintln!("recording was saved, but the editor could not open: {error}");
    }
    Ok(artifact)
}

#[tauri::command]
pub async fn discard_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<RecordingSessionSnapshot, String> {
    discard_recording_inner(app, state.inner().clone(), &session_id)
        .await
        .map_err(|error| error.to_string())
}

async fn discard_recording_inner(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: &str,
) -> Result<RecordingSessionSnapshot, AppError> {
    let (active, snapshot) = {
        let mut runtime = state.recording.lock();
        runtime.generation = runtime.generation.wrapping_add(1);
        let snapshot = runtime
            .coordinator
            .discard(session_id, now_ms())
            .map_err(|error| AppError::Task(error.to_string()))?;
        let active = runtime
            .session
            .as_mut()
            .filter(|session| session.id == session_id)
            .and_then(|session| session.active_segment.take());
        runtime.session = None;
        (active, snapshot)
    };
    if let Some(active) = active {
        let _ = tauri::async_runtime::spawn_blocking(move || active.discard()).await;
    }
    DraftStore::new(recording_recovery_directory())
        .remove(session_id)
        .map_err(|error| AppError::Task(error.to_string()))?;
    emit_snapshot(&app, &snapshot);
    destroy_recording_countdown(&app);
    crate::hide_window(&app, "recording-hud");
    crate::restore_thumbnail_stack(&app, &state);
    Ok(snapshot)
}

#[tauri::command]
pub async fn set_recording_microphone_muted(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
    muted: bool,
) -> Result<RecordingSessionSnapshot, String> {
    set_microphone_muted_inner(app, state.inner().clone(), &session_id, muted)
        .await
        .map_err(|error| error.to_string())
}

async fn set_microphone_muted_inner(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: &str,
    muted: bool,
) -> Result<RecordingSessionSnapshot, AppError> {
    let current = state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .filter(|snapshot| snapshot.id == session_id)
        .ok_or(AppError::SessionUnavailable)?;
    let was_recording = current.state == RecordingState::Recording;
    if was_recording {
        pause_recording_inner(&app, state.clone(), session_id).await?;
    } else if current.state != RecordingState::Paused {
        return Err(AppError::Task(
            "microphone mute can only change while recording or paused".to_owned(),
        ));
    }
    let snapshot = {
        let mut runtime = state.recording.lock();
        let session = runtime
            .session
            .as_mut()
            .filter(|session| session.id == session_id)
            .ok_or(AppError::SessionUnavailable)?;
        session.options.audio.microphone_muted = muted;
        session.manifest.options = session.options.clone();
        session.manifest.updated_at_ms = now_ms();
        save_manifest(&session.manifest)?;
        let options = session.options.clone();
        runtime
            .coordinator
            .update_options(session_id, options, now_ms())
            .map_err(|error| AppError::Task(error.to_string()))?
    };
    emit_snapshot(&app, &snapshot);
    if was_recording {
        let generation = state.recording.lock().generation;
        start_segment(app, state.clone(), session_id, generation).await?;
        return state
            .recording
            .lock()
            .coordinator
            .snapshot(now_ms())
            .filter(|snapshot| snapshot.id == session_id)
            .ok_or(AppError::SessionUnavailable);
    }
    Ok(snapshot)
}

#[tauri::command]
pub fn get_recording_artifacts(state: tauri::State<'_, Arc<AppState>>) -> Vec<RecordingArtifact> {
    state
        .recording_artifacts
        .lock()
        .iter_mut()
        .map(|artifact| {
            artifact.summary.missing = !Path::new(&artifact.summary.path).is_file();
            artifact.summary.clone()
        })
        .collect()
}

#[tauri::command]
pub fn get_recording_artifact(
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Option<RecordingArtifact> {
    state
        .recording_artifacts
        .lock()
        .iter_mut()
        .find(|artifact| artifact.summary.id == artifact_id)
        .map(|artifact| {
            artifact.summary.missing = !Path::new(&artifact.summary.path).is_file();
            artifact.summary.clone()
        })
}

#[tauri::command]
pub async fn prepare_recording_timeline_preview(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<RecordingTimelinePreview, String> {
    const FRAME_COUNT: u16 = 12;
    const FRAME_WIDTH: u32 = 160;
    const FRAME_HEIGHT: u32 = 90;
    let preview = || RecordingTimelinePreview {
        url: recording_timeline_url(&artifact_id),
        frame_count: FRAME_COUNT,
        frame_width: FRAME_WIDTH,
        frame_height: FRAME_HEIGHT,
        sprite_width: FRAME_WIDTH * u32::from(FRAME_COUNT),
        sprite_height: FRAME_HEIGHT,
    };
    if state
        .recording_timeline_sprites
        .lock()
        .contains_key(&artifact_id)
    {
        return Ok(preview());
    }
    let source = state
        .recording_artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.summary.id == artifact_id)
        .map(|artifact| artifact.summary.clone())
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    if !Path::new(&source.path).is_file() {
        return Err("the recording file is missing".to_owned());
    }
    let app_state = state.inner().clone();
    let cache_key = artifact_id.clone();
    let output = std::env::temp_dir().join(format!("captures-timeline-{}.png", Uuid::new_v4()));
    let output_for_task = output.clone();
    let input = PathBuf::from(source.path);
    let toolchain = media_toolchain(&app);
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        let cancel = CancelToken::default();
        toolchain.create_timeline_sprite(
            &input,
            &output_for_task,
            TimelineSpriteSpec {
                duration_ms: source.duration_ms.max(1),
                frame_count: FRAME_COUNT,
                frame_width: FRAME_WIDTH,
                frame_height: FRAME_HEIGHT,
            },
            &cancel,
        )?;
        let bytes = fs::read(&output_for_task)?;
        let _ = fs::remove_file(&output_for_task);
        Ok::<_, captures_media::MediaToolError>(bytes)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    let _ = fs::remove_file(output);
    app_state
        .recording_timeline_sprites
        .lock()
        .insert(cache_key, bytes);
    Ok(preview())
}

#[tauri::command]
pub fn start_recording_export(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    mut request: StartExportRequest,
) -> Result<String, String> {
    let (source, poster_png) = state
        .recording_artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.summary.id == request.artifact_id)
        .map(|artifact| (artifact.summary.clone(), artifact.poster_png.clone()))
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    if !Path::new(&source.path).is_file() {
        return Err("the recording file is missing".to_owned());
    }
    request.edit.audio.source_has_system_audio = source.has_system_audio;
    request.edit.audio.source_has_microphone_audio = source.has_microphone_audio;
    let extension = match request.export.format {
        ExportFormat::Mp4 => "mp4",
        ExportFormat::Gif => "gif",
        ExportFormat::WebM => "webm",
    };
    let source_path = PathBuf::from(&source.path);
    let source_extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if request.overwrite_source && !source_extension.eq_ignore_ascii_case(extension) {
        return Err("changing the file format requires saving a copy".to_owned());
    }
    let selected_directory = request
        .destination_directory
        .as_deref()
        .filter(|directory| !directory.is_empty())
        .map(Path::new);
    let final_destination = if request.overwrite_source {
        source_path.clone()
    } else {
        match selected_directory {
            Some(directory) => storage::recording_destination_path_in(
                &source_path,
                Some(directory),
                &request.file_stem,
                extension,
            ),
            None => {
                storage::recording_destination_path(&source_path, &request.file_stem, extension)
            }
        }
        .map_err(|error| error.to_string())?
    };
    let working_destination = if request.overwrite_source {
        replacement_working_path(&source_path, extension).map_err(|error| error.to_string())?
    } else {
        final_destination.clone()
    };
    let export_id = Uuid::new_v4().to_string();
    let cancel = CancelToken::default();
    state
        .recording
        .lock()
        .exports
        .insert(export_id.clone(), cancel.clone());

    let task_export_id = export_id.clone();
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let toolchain = media_toolchain(&app);
        let input = PathBuf::from(&source.path);
        let task_app = app.clone();
        let progress_export_id = task_export_id.clone();
        let edit = request.edit.clone();
        let export = request.export.clone();
        let overwrite_source = request.overwrite_source;
        let cleanup_destination = working_destination.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let mut outcome = toolchain.export(
                &input,
                &working_destination,
                &edit,
                &export,
                &cancel,
                |progress| {
                    let _ = task_app.emit(
                        "recording-export-progress",
                        RecordingExportProgress {
                            export_id: progress_export_id.clone(),
                            progress,
                        },
                    );
                },
            )?;
            let probe = toolchain.probe(&outcome.path)?;
            let generated_poster_path = outcome
                .path
                .with_file_name(format!(".captures-export-poster-{}.png", Uuid::new_v4()));
            let generated_poster = toolchain
                .create_poster(&outcome.path, &generated_poster_path, &cancel)
                .ok()
                .and_then(|()| fs::read(&generated_poster_path).ok());
            let _ = fs::remove_file(generated_poster_path);
            if overwrite_source {
                replace_recording_source(&input, &outcome.path)?;
                outcome.path = input;
                outcome.size_bytes = fs::metadata(&outcome.path)?.len();
            }
            Ok::<_, captures_media::MediaToolError>((outcome, probe, generated_poster))
        })
        .await;
        state.recording.lock().exports.remove(&task_export_id);
        match result {
            Ok(Ok((outcome, probe, generated_poster))) => {
                let artifact_id = if request.overwrite_source {
                    source.id.clone()
                } else {
                    Uuid::new_v4().to_string()
                };
                let keeps_system_audio = extension != "gif"
                    && source.has_system_audio
                    && !request.edit.audio.mute_system_audio;
                let keeps_microphone_audio = extension != "gif"
                    && source.has_microphone_audio
                    && !request.edit.audio.mute_microphone;
                let (has_system_audio, has_microphone_audio) =
                    match (keeps_system_audio, keeps_microphone_audio) {
                        (false, true) => (false, true),
                        (true, false) => (true, false),
                        // Two edited source tracks are mixed into one export track.
                        (true, true) => (true, false),
                        (false, false) => (false, false),
                    };
                let artifact = RecordingArtifact {
                    id: artifact_id.clone(),
                    kind: if extension == "gif" {
                        RecordingKind::Gif
                    } else {
                        RecordingKind::Video
                    },
                    path: outcome.path.to_string_lossy().into_owned(),
                    media_url: recording_media_url(&artifact_id),
                    poster_url: recording_poster_url(&artifact_id),
                    mime_type: match extension {
                        "gif" => "image/gif",
                        "webm" => "video/webm",
                        _ => "video/mp4",
                    }
                    .to_owned(),
                    duration_ms: probe.metadata.duration_ms.unwrap_or(0),
                    width: probe.metadata.width,
                    height: probe.metadata.height,
                    size_bytes: probe.metadata.size_bytes.max(outcome.size_bytes),
                    dropped_frames: source.dropped_frames,
                    has_system_audio,
                    has_microphone_audio,
                    created_at: if request.overwrite_source {
                        source.created_at
                    } else {
                        chrono::Utc::now().to_rfc3339()
                    },
                    target: source.target,
                    missing: false,
                };
                upsert_recording_artifact(
                    &app,
                    &state,
                    artifact.clone(),
                    generated_poster.unwrap_or(poster_png),
                );
                let finder_error = app
                    .opener()
                    .reveal_item_in_dir(PathBuf::from(&artifact.path))
                    .err()
                    .map(|error| error.to_string());
                let _ = app.emit(
                    "recording-export-complete",
                    RecordingExportComplete {
                        export_id: task_export_id,
                        artifact,
                        finder_error,
                    },
                );
            }
            Ok(Err(error)) => {
                if request.overwrite_source {
                    let _ = fs::remove_file(&cleanup_destination);
                }
                let cancelled = matches!(error, captures_media::MediaToolError::Cancelled);
                let _ = app.emit(
                    "recording-export-failed",
                    RecordingExportFailed {
                        export_id: task_export_id,
                        message: error.to_string(),
                        cancelled,
                    },
                );
            }
            Err(error) => {
                if request.overwrite_source {
                    let _ = fs::remove_file(&cleanup_destination);
                }
                let _ = app.emit(
                    "recording-export-failed",
                    RecordingExportFailed {
                        export_id: task_export_id,
                        message: error.to_string(),
                        cancelled: false,
                    },
                );
            }
        }
    });
    Ok(export_id)
}

#[tauri::command]
pub fn cancel_recording_export(
    state: tauri::State<'_, Arc<AppState>>,
    export_id: String,
) -> Result<(), String> {
    let token = state
        .recording
        .lock()
        .exports
        .get(&export_id)
        .cloned()
        .ok_or_else(|| "export is no longer active".to_owned())?;
    token.cancel();
    Ok(())
}

#[tauri::command]
pub fn reveal_recording_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<(), String> {
    let path = state
        .recording_artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.summary.id == artifact_id)
        .map(|artifact| artifact.summary.path.clone())
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    app.opener()
        .reveal_item_in_dir(PathBuf::from(path))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_recording_editor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<(), String> {
    let available = state.recording_artifacts.lock().iter().any(|artifact| {
        artifact.summary.id == artifact_id && Path::new(&artifact.summary.path).is_file()
    });
    if !available {
        return Err("the recording file is missing".to_owned());
    }
    show_recording_editor(&app, &artifact_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn trash_recording_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<(), String> {
    let path = state
        .recording_artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.summary.id == artifact_id)
        .map(|artifact| artifact.summary.path.clone())
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    if Path::new(&path).exists() {
        trash::delete(path).map_err(|error| error.to_string())?;
    }
    state
        .recording_artifacts
        .lock()
        .retain(|artifact| artifact.summary.id != artifact_id);
    state.recording_timeline_sprites.lock().remove(&artifact_id);
    let _ = storage::delete_history_capture(&artifact_id);
    state.history.lock().retain(|entry| entry.id != artifact_id);
    let _ = app.emit("capture-history-changed", ());
    let _ = app.emit("recording-artifact-removed", artifact_id);
    Ok(())
}

#[tauri::command]
pub fn get_recording_drafts() -> Vec<RecordingDraftManifest> {
    DraftStore::new(recording_recovery_directory())
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|manifest| {
            matches!(
                manifest.state,
                RecordingState::Recording
                    | RecordingState::Paused
                    | RecordingState::Finalizing
                    | RecordingState::Failed
            )
        })
        .collect()
}

#[tauri::command]
pub async fn recover_recording_draft(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<RecordingArtifact, String> {
    let state = state.inner().clone();
    match recover_recording_draft_inner(app.clone(), state.clone(), &session_id).await {
        Ok(artifact) => Ok(artifact),
        Err(error) => {
            let store = DraftStore::new(recording_recovery_directory());
            if let Ok(mut manifest) = store.load(&session_id) {
                manifest.state = RecordingState::Failed;
                manifest.last_error = Some(error.to_string());
                manifest.updated_at_ms = now_ms();
                let _ = store.save(&manifest);
            }
            let _ = app.emit(
                RECORDING_WARNING_EVENT,
                RecordingWarning {
                    session_id,
                    message: error.to_string(),
                },
            );
            Err(error.to_string())
        }
    }
}

async fn recover_recording_draft_inner(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: &str,
) -> Result<RecordingArtifact, AppError> {
    if !state.sessions.lock().is_empty()
        || state
            .recording
            .lock()
            .coordinator
            .snapshot(now_ms())
            .is_some_and(|snapshot| !snapshot.state.is_terminal())
    {
        return Err(AppError::CaptureInProgress);
    }

    let store = DraftStore::new(recording_recovery_directory());
    let mut manifest = store
        .load(session_id)
        .map_err(|error| AppError::Task(error.to_string()))?;
    if !matches!(
        manifest.state,
        RecordingState::Recording
            | RecordingState::Paused
            | RecordingState::Finalizing
            | RecordingState::Failed
    ) {
        return Err(AppError::Task(
            "this recording draft is not recoverable".to_owned(),
        ));
    }
    let directory = store
        .session_directory(session_id)
        .map_err(|error| AppError::Task(error.to_string()))?;
    manifest.state = RecordingState::Finalizing;
    manifest.last_error = None;
    manifest.updated_at_ms = now_ms();
    store
        .save(&manifest)
        .map_err(|error| AppError::Task(error.to_string()))?;

    let settings = state.settings();
    let extension = if manifest.options.kind == RecordingKind::Video {
        "mp4"
    } else {
        "gif"
    };
    let destination = storage::unique_media_path(Path::new(&settings.output_directory), extension)?;
    let task_app = app.clone();
    let mut task_manifest = manifest.clone();
    let task_directory = directory.clone();
    let destination_for_task = destination.clone();
    let (probe, poster_png, recovered_manifest) = tauri::async_runtime::spawn_blocking(move || {
        let toolchain = media_toolchain(&task_app);
        toolchain
            .verify()
            .map_err(|error| AppError::Task(error.to_string()))?;
        let mut segments = Vec::new();
        for segment in &mut task_manifest.segments {
            let video_path = recovery_child_path(&task_directory, &segment.relative_path)?;
            if !video_path.is_file() {
                if segment.complete {
                    return Err(AppError::Task(format!(
                        "completed recovery segment {} is missing",
                        segment.index
                    )));
                }
                continue;
            }
            if !segment.complete {
                let Ok(partial) = toolchain.probe(&video_path) else {
                    continue;
                };
                segment.complete = true;
                segment.width = partial.metadata.width;
                segment.height = partial.metadata.height;
                segment.duration_ms = partial.metadata.duration_ms.unwrap_or(0);
                segment.size_bytes = partial.metadata.size_bytes;
            }
            let microphone_path = segment
                .microphone_relative_path
                .as_deref()
                .map(|relative| recovery_child_path(&task_directory, relative))
                .transpose()?
                .filter(|path| path.is_file());
            segments.push(RecordingSegmentInput {
                video_path,
                microphone_path,
                microphone_offset_ms: segment.microphone_offset_ms,
                duration_ms: segment.duration_ms,
            });
        }
        task_manifest.segments.retain(|segment| segment.complete);
        if segments.is_empty() {
            return Err(AppError::Task(
                "no complete media segments could be recovered".to_owned(),
            ));
        }
        let has_microphone_audio = segments
            .iter()
            .any(|segment| segment.microphone_path.is_some());
        let cancel = CancelToken::default();
        if task_manifest.options.kind == RecordingKind::Video {
            toolchain
                .assemble_recording_segments(
                    &segments,
                    &destination_for_task,
                    RecordingAudioLayout {
                        system_audio: task_manifest.options.audio.capture_system_audio,
                        microphone_audio: has_microphone_audio,
                    },
                    &cancel,
                )
                .map_err(|error| AppError::Task(error.to_string()))?;
        } else {
            let paths = segments
                .iter()
                .map(|segment| segment.video_path.clone())
                .collect::<Vec<_>>();
            let master = if paths.len() == 1 {
                paths[0].clone()
            } else {
                let master = task_directory.join("master.mp4");
                if !master.exists() {
                    toolchain
                        .concatenate_segments(&paths, &master, &cancel)
                        .map_err(|error| AppError::Task(error.to_string()))?;
                }
                master
            };
            toolchain
                .create_gif(
                    &master,
                    &destination_for_task,
                    task_manifest.options.frames_per_second,
                    task_manifest.options.gif.max_width,
                    task_manifest.options.gif.max_colors,
                    &cancel,
                )
                .map_err(|error| AppError::Task(error.to_string()))?;
        }
        let probe = toolchain
            .probe(&destination_for_task)
            .map_err(|error| AppError::Task(error.to_string()))?;
        let poster_path = task_directory.join("poster.png");
        if !poster_path.is_file() {
            toolchain
                .create_poster(&destination_for_task, &poster_path, &cancel)
                .map_err(|error| AppError::Task(error.to_string()))?;
        }
        let poster_png = fs::read(poster_path)?;
        Ok::<_, AppError>((probe, poster_png, task_manifest))
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))??;

    let artifact_id = Uuid::new_v4().to_string();
    let artifact = RecordingArtifact {
        id: artifact_id.clone(),
        kind: manifest.options.kind,
        path: destination.to_string_lossy().into_owned(),
        media_url: recording_media_url(&artifact_id),
        poster_url: recording_poster_url(&artifact_id),
        mime_type: probe.metadata.mime_type,
        duration_ms: probe.metadata.duration_ms.unwrap_or(0),
        width: probe.metadata.width,
        height: probe.metadata.height,
        size_bytes: probe.metadata.size_bytes,
        dropped_frames: recovered_manifest
            .segments
            .iter()
            .map(|segment| segment.dropped_frames)
            .sum(),
        has_system_audio: manifest.options.audio.capture_system_audio,
        has_microphone_audio: recovered_manifest
            .segments
            .iter()
            .any(|segment| segment.microphone_relative_path.is_some()),
        created_at: chrono::Utc::now().to_rfc3339(),
        target: manifest.options.target.clone(),
        missing: false,
    };
    upsert_recording_artifact(&app, &state, artifact.clone(), poster_png);

    let mut recovered_manifest = recovered_manifest;
    recovered_manifest.state = RecordingState::Ready;
    recovered_manifest.updated_at_ms = now_ms();
    recovered_manifest.final_path = Some(artifact.path.clone());
    recovered_manifest.last_error = None;
    store
        .save(&recovered_manifest)
        .map_err(|error| AppError::Task(error.to_string()))?;
    if artifact.kind == RecordingKind::Video {
        let _ = store.remove(session_id);
    }
    let _ = app.emit(RECORDING_ARTIFACT_EVENT, &artifact);
    if settings.recording.open_editor_after_recording
        && let Err(error) = show_recording_editor(&app, &artifact.id)
    {
        eprintln!("recording was recovered, but the editor could not open: {error}");
    }
    Ok(artifact)
}

#[tauri::command]
pub fn discard_recording_draft(session_id: String) -> Result<(), String> {
    DraftStore::new(recording_recovery_directory())
        .remove(&session_id)
        .map_err(|error| error.to_string())
}

pub fn prune_expired_gif_sources() {
    let _ = DraftStore::new(recording_recovery_directory())
        .prune_gif_sources(now_ms(), GIF_SOURCE_RETENTION_MS);
}

fn recovery_child_path(directory: &Path, relative: &str) -> Result<PathBuf, AppError> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::Task(
            "recording recovery manifest contains an invalid media path".to_owned(),
        ));
    }
    Ok(directory.join(relative))
}

pub fn resolve_recording_asset(
    state: &AppState,
    category: &str,
    id: &str,
    range_header: Option<&str>,
) -> Option<ResolvedRecordingAsset> {
    Uuid::parse_str(id).ok()?;
    match category {
        "recording-selection" => state
            .recording_selection
            .lock()
            .as_ref()
            .filter(|selection| selection.summary.id == id)
            .map(|selection| ResolvedRecordingAsset {
                mime_type: "image/png".to_owned(),
                bytes: selection.snapshot_png.clone(),
                status: 200,
                total_length: None,
                content_range: None,
            }),
        "poster" => state
            .recording_artifacts
            .lock()
            .iter()
            .find(|artifact| artifact.summary.id == id)
            .map(|artifact| ResolvedRecordingAsset {
                mime_type: "image/png".to_owned(),
                bytes: artifact.poster_png.clone(),
                status: 200,
                total_length: None,
                content_range: None,
            }),
        "timeline" => state
            .recording_timeline_sprites
            .lock()
            .get(id)
            .map(|bytes| ResolvedRecordingAsset {
                mime_type: "image/png".to_owned(),
                bytes: bytes.clone(),
                status: 200,
                total_length: None,
                content_range: None,
            }),
        "media" => {
            let artifact = state
                .recording_artifacts
                .lock()
                .iter()
                .find(|artifact| artifact.summary.id == id)
                .map(|artifact| artifact.summary.clone())?;
            let mut file = fs::File::open(&artifact.path).ok()?;
            let total_length = file.metadata().ok()?.len();
            let range = match range_header {
                Some(value) => match ByteRange::parse(value, total_length) {
                    Ok(range) => Some(range),
                    Err(_) => {
                        return Some(ResolvedRecordingAsset {
                            mime_type: "text/plain".to_owned(),
                            bytes: Vec::new(),
                            status: 416,
                            total_length: Some(total_length),
                            content_range: Some(format!("bytes */{total_length}")),
                        });
                    }
                },
                None => None,
            };
            let (status, content_range, bytes) = if let Some(range) = range {
                file.seek(SeekFrom::Start(range.start)).ok()?;
                let length = usize::try_from(range.length()).ok()?;
                let mut bytes = vec![0; length];
                file.read_exact(&mut bytes).ok()?;
                (206, Some(range.content_range(total_length)), bytes)
            } else {
                let mut bytes = Vec::with_capacity(usize::try_from(total_length).ok()?);
                file.read_to_end(&mut bytes).ok()?;
                (200, None, bytes)
            };
            Some(ResolvedRecordingAsset {
                mime_type: artifact.mime_type,
                bytes,
                status,
                total_length: Some(total_length),
                content_range,
            })
        }
        _ => None,
    }
}

pub struct ResolvedRecordingAsset {
    pub mime_type: String,
    pub bytes: Vec<u8>,
    pub status: u16,
    pub total_length: Option<u64>,
    pub content_range: Option<String>,
}

fn validate_target(
    selection: &RecordingSelectionSession,
    target: &RecordingTarget,
) -> Result<(), AppError> {
    match target {
        RecordingTarget::Display { display_id } if display_id == &selection.display.id => Ok(()),
        RecordingTarget::Region { display_id, rect }
            if display_id == &selection.display.id
                && rect.is_valid()
                && rect.x >= 0
                && rect.y >= 0
                && u32::try_from(rect.x)
                    .ok()
                    .and_then(|x| x.checked_add(rect.width))
                    .is_some_and(|right| right <= selection.display.width)
                && u32::try_from(rect.y)
                    .ok()
                    .and_then(|y| y.checked_add(rect.height))
                    .is_some_and(|bottom| bottom <= selection.display.height) =>
        {
            Ok(())
        }
        RecordingTarget::Window { window_id }
            if selection
                .windows
                .iter()
                .any(|window| &window.id == window_id) =>
        {
            Ok(())
        }
        _ => Err(AppError::InvalidSelection),
    }
}

fn poster_for_selection(
    selection: &RecordingSelection,
    target: &RecordingTarget,
) -> Result<Vec<u8>, AppError> {
    let image = match target {
        RecordingTarget::Display { .. } => selection.image.clone(),
        RecordingTarget::Region { rect, .. } => {
            let scale_x = f64::from(selection.image.width())
                / f64::from(selection.summary.display.width.max(1));
            let scale_y = f64::from(selection.image.height())
                / f64::from(selection.summary.display.height.max(1));
            let x = (f64::from(rect.x) * scale_x).round().max(0.0) as u32;
            let y = (f64::from(rect.y) * scale_y).round().max(0.0) as u32;
            let width = (f64::from(rect.width) * scale_x).round().max(1.0) as u32;
            let height = (f64::from(rect.height) * scale_y).round().max(1.0) as u32;
            image::imageops::crop_imm(
                &selection.image,
                x.min(selection.image.width().saturating_sub(1)),
                y.min(selection.image.height().saturating_sub(1)),
                width.min(selection.image.width().saturating_sub(x)),
                height.min(selection.image.height().saturating_sub(y)),
            )
            .to_image()
        }
        RecordingTarget::Window { window_id } => {
            let window = selection
                .summary
                .windows
                .iter()
                .find(|window| &window.id == window_id)
                .ok_or(AppError::InvalidSelection)?;
            let display = &selection.summary.display;
            let scale_x = f64::from(selection.image.width()) / f64::from(display.width.max(1));
            let scale_y = f64::from(selection.image.height()) / f64::from(display.height.max(1));
            let x = (f64::from(window.x - display.x) * scale_x).round().max(0.0) as u32;
            let y = (f64::from(window.y - display.y) * scale_y).round().max(0.0) as u32;
            let width = (f64::from(window.width) * scale_x).round().max(1.0) as u32;
            let height = (f64::from(window.height) * scale_y).round().max(1.0) as u32;
            image::imageops::crop_imm(
                &selection.image,
                x.min(selection.image.width().saturating_sub(1)),
                y.min(selection.image.height().saturating_sub(1)),
                width.min(selection.image.width().saturating_sub(x)),
                height.min(selection.image.height().saturating_sub(y)),
            )
            .to_image()
        }
    };
    storage::encode_thumbnail_png(&image)
}

fn take_active_segment(
    state: &AppState,
    session_id: &str,
    expected_state: RecordingState,
) -> Result<(MacRecordingSegment, u64), AppError> {
    let mut runtime = state.recording.lock();
    let current = runtime
        .coordinator
        .snapshot(now_ms())
        .filter(|snapshot| snapshot.id == session_id && snapshot.state == expected_state)
        .ok_or(AppError::SessionUnavailable)?;
    let _ = current;
    let session = runtime
        .session
        .as_mut()
        .filter(|session| session.id == session_id)
        .ok_or(AppError::SessionUnavailable)?;
    let segment = session
        .active_segment
        .take()
        .ok_or_else(|| AppError::Task("the active recording segment is unavailable".to_owned()))?;
    let started_at_ms = session
        .active_segment_started_at_ms
        .take()
        .unwrap_or_else(now_ms);
    Ok((segment, started_at_ms))
}

async fn stop_native_segment(segment: MacRecordingSegment) -> Result<SegmentInfo, AppError> {
    tauri::async_runtime::spawn_blocking(move || segment.stop())
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
        .map_err(|error| AppError::Task(error.to_string()))
}

fn append_segment(
    session: &mut RuntimeSession,
    info: SegmentInfo,
    started_at_ms: u64,
    now: u64,
) -> Result<(), AppError> {
    let relative_path = info
        .path
        .strip_prefix(&session.directory)
        .map_err(|_| AppError::Task("recording segment escaped its recovery bundle".to_owned()))?
        .to_string_lossy()
        .into_owned();
    let microphone_relative_path = info
        .microphone_path
        .as_ref()
        .map(|path| {
            path.strip_prefix(&session.directory)
                .map(|relative| relative.to_string_lossy().into_owned())
                .map_err(|_| {
                    AppError::Task("microphone segment escaped its recovery bundle".to_owned())
                })
        })
        .transpose()?;
    let segment = RecordingSegmentManifest {
        index: u32::try_from(session.manifest.segments.len())
            .map_err(|_| AppError::Task("recording has too many segments".to_owned()))?,
        relative_path,
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
    if let Some(pending) = session
        .manifest
        .segments
        .iter_mut()
        .rev()
        .find(|pending| !pending.complete && pending.relative_path == segment.relative_path)
    {
        let index = pending.index;
        *pending = RecordingSegmentManifest { index, ..segment };
    } else {
        session.manifest.segments.push(segment);
    }
    session.manifest.updated_at_ms = now;
    Ok(())
}

fn fail_session(app: &AppHandle, state: &AppState, session_id: &str, message: String) {
    let snapshot = {
        let mut runtime = state.recording.lock();
        let now = now_ms();
        let snapshot = runtime
            .coordinator
            .fail(session_id, message.clone(), now)
            .ok();
        if let Some(session) = runtime
            .session
            .as_mut()
            .filter(|session| session.id == session_id)
        {
            session.manifest.state = RecordingState::Failed;
            session.manifest.last_error = Some(message.clone());
            session.manifest.updated_at_ms = now;
            let _ = save_manifest(&session.manifest);
        }
        snapshot
    };
    if let Some(snapshot) = snapshot {
        emit_snapshot(app, &snapshot);
    }
    let _ = app.emit(
        RECORDING_WARNING_EVENT,
        RecordingWarning {
            session_id: session_id.to_owned(),
            message,
        },
    );
    destroy_recording_countdown(app);
}

fn restore_recording_ui(app: &AppHandle, state: &Arc<AppState>) {
    destroy_recording_selector(app);
    destroy_recording_countdown(app);
    crate::hide_window(app, "recording-hud");
    crate::set_capture_huds_protected(app, false);
    crate::restore_thumbnail_stack(app, state);
}

#[cfg(target_os = "macos")]
fn focus_recording_window(app: &AppHandle, label: &'static str) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(label) else {
            return;
        };
        if let Err(error) = captures_macos_window::focus_window(&window) {
            eprintln!("failed to activate {label}: {error}");
        }
    }) {
        eprintln!("failed to schedule {label} activation: {error}");
    }
}

fn destroy_recording_selector(app: &AppHandle) {
    let Some(window) = app.get_webview_window("recording-selector") else {
        return;
    };
    if let Err(error) = window.set_ignore_cursor_events(true) {
        eprintln!("failed to disable recording selector pointer events: {error}");
    }
    // Hidden WKWebViews can suspend before processing the next selection.
    // Recreate this short-lived surface so every selection gets a live event
    // bridge and cannot inherit stale region or window state.
    if let Err(error) = window.destroy() {
        eprintln!("failed to destroy recording selector: {error}");
    }
}

fn replacement_working_path(source: &Path, extension: &str) -> io::Result<PathBuf> {
    let directory = source
        .parent()
        .filter(|directory| directory.is_dir())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "save folder is unavailable"))?;
    Ok(directory.join(format!(".captures-save-{}.{}", Uuid::new_v4(), extension)))
}

fn replace_recording_source(source: &Path, replacement: &Path) -> io::Result<()> {
    let directory = source
        .parent()
        .filter(|directory| directory.is_dir())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "save folder is unavailable"))?;
    let source_extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("media");
    let backup = directory.join(format!(
        ".captures-backup-{}.{}",
        Uuid::new_v4(),
        source_extension
    ));

    fs::rename(source, &backup)?;
    match fs::rename(replacement, source) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(replace_error) => match fs::rename(&backup, source) {
            Ok(()) => Err(replace_error),
            Err(restore_error) => Err(io::Error::new(
                replace_error.kind(),
                format!(
                    "the edited recording could not replace the original ({replace_error}), and the backup could not be restored automatically ({restore_error})"
                ),
            )),
        },
    }
}

fn upsert_recording_artifact(
    app: &AppHandle,
    state: &AppState,
    artifact: RecordingArtifact,
    poster_png: Vec<u8>,
) {
    let history_entry = HistoryEntry::from_recording(&artifact);
    let history_saved = match storage::save_history_recording(&history_entry, &poster_png) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("failed to save recording history: {error}");
            false
        }
    };
    let artifact_id = artifact.id.clone();
    let artifact_data = RecordingArtifactData {
        summary: artifact,
        poster_png,
    };
    let mut recording_artifacts = state.recording_artifacts.lock();
    if let Some(existing) = recording_artifacts
        .iter_mut()
        .find(|existing| existing.summary.id == artifact_id)
    {
        *existing = artifact_data;
    } else {
        recording_artifacts.push(artifact_data);
    }
    drop(recording_artifacts);
    state.recording_timeline_sprites.lock().remove(&artifact_id);
    if history_saved {
        let mut history = state.history.lock();
        if let Some(existing) = history.iter_mut().find(|entry| entry.id == artifact_id) {
            *existing = history_entry;
        } else {
            history.insert(0, history_entry);
        }
        let _ = app.emit("capture-history-changed", ());
    }
}

fn save_manifest(manifest: &RecordingDraftManifest) -> Result<(), AppError> {
    DraftStore::new(recording_recovery_directory())
        .save(manifest)
        .map_err(|error| AppError::Task(error.to_string()))
}

fn emit_snapshot(app: &AppHandle, snapshot: &RecordingSessionSnapshot) {
    if let Err(error) = app.emit(RECORDING_STATE_EVENT, snapshot) {
        eprintln!("failed to emit recording state: {error}");
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn media_toolchain(app: &AppHandle) -> MediaToolchain {
    let executable_directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let resource_directory = app.path().resource_dir().ok();
    let target_suffix = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let find = |name: &str| {
        let suffixed_name = format!("{name}-{target_suffix}");
        executable_directory
            .as_ref()
            .map(|directory| directory.join(name))
            .filter(|path| path.is_file())
            .or_else(|| {
                executable_directory
                    .as_ref()
                    .map(|directory| directory.join(&suffixed_name))
                    .filter(|path| path.is_file())
            })
            .or_else(|| {
                resource_directory
                    .as_ref()
                    .map(|directory| directory.join("binaries").join(name))
                    .filter(|path| path.is_file())
            })
            .or_else(|| {
                resource_directory
                    .as_ref()
                    .map(|directory| directory.join("binaries").join(&suffixed_name))
                    .filter(|path| path.is_file())
            })
            .or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("binaries")
                    .join(&suffixed_name)
                    .is_file()
                    .then(|| {
                        Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("binaries")
                            .join(&suffixed_name)
                    })
            })
            .unwrap_or_else(|| PathBuf::from(name))
    };
    MediaToolchain::new(find("ffmpeg"), find("ffprobe"))
}

#[cfg(target_os = "macos")]
fn create_recording_selector_window(app: &AppHandle) -> Result<(), AppError> {
    if app.get_webview_window("recording-selector").is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        app,
        "recording-selector",
        WebviewUrl::App("index.html?view=recording-selector".into()),
    )
    .title("Captures Recorder")
    .inner_size(1.0, 1.0)
    .position(-10_000.0, -10_000.0)
    .decorations(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .focused(false)
    .visible(true)
    .build()?;
    window.set_content_protected(false)?;
    Ok(())
}

#[cfg(target_os = "macos")]
async fn prepare_recording_selector(
    app: &AppHandle,
    selection: &RecordingSelectionSession,
) -> Result<(), AppError> {
    create_recording_selector_window(app)?;
    let handle = app.clone();
    let selection = selection.clone();
    let wake_selection = selection.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            let display = &selection.display;
            let window = handle
                .get_webview_window("recording-selector")
                .ok_or_else(|| "recording selector is unavailable".to_owned())?;
            window
                .set_size(LogicalSize::new(
                    f64::from(display.width),
                    f64::from(display.height),
                ))
                .map_err(|error| error.to_string())?;
            // A hidden borderless NSWindow grows from its bottom-left anchor.
            // Position it after resizing so the final top-left edge matches the
            // selected display instead of landing one full screen above it.
            window
                .set_position(tauri::LogicalPosition::new(
                    f64::from(display.x),
                    f64::from(display.y),
                ))
                .map_err(|error| error.to_string())?;
            window
                .set_content_protected(false)
                .map_err(|error| error.to_string())?;
            // A hidden or zero-alpha WKWebView can be suspended before React
            // installs its recording-selection listener. Wake it at a tiny,
            // imperceptible alpha while pointer events still pass through.
            // React reveals the window only after the new snapshot has painted,
            // so a cached region or window highlight can never flash onscreen.
            window
                .set_ignore_cursor_events(true)
                .map_err(|error| error.to_string())?;
            captures_macos_window::prime_window_reveal(&window).map_err(str::to_owned)?;
            window.show().map_err(|error| error.to_string())?;
            handle
                .emit("recording-selection-ready", &selection)
                .map_err(|error| error.to_string())?;
            Ok(())
        })();
        let _ = sender.send(result);
    })?;
    receiver
        .await
        .map_err(|_| AppError::Task("recording selector setup was interrupted".to_owned()))?
        .map_err(AppError::Task)?;
    schedule_recording_selector_webview_wake(app, wake_selection);
    Ok(())
}

#[cfg(target_os = "macos")]
fn schedule_recording_selector_webview_wake(app: &AppHandle, selection: RecordingSelectionSession) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A hidden or near-transparent WKWebView can suspend JavaScript,
        // including the frontend's animation-frame and timer fallbacks. Wake
        // the native window independently while it is still click-through.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let selection_id = selection.id.clone();
        let still_pending = app
            .state::<Arc<AppState>>()
            .recording_selection
            .lock()
            .as_ref()
            .is_some_and(|selection| selection.summary.id == selection_id);
        if !still_pending {
            return;
        }
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            let Some(window) = handle.get_webview_window("recording-selector") else {
                return;
            };
            if let Err(error) = captures_macos_window::reveal_window(&window) {
                eprintln!("failed to wake recording selector WebView: {error}");
            }
            if let Err(error) = window.show() {
                eprintln!("failed to show recording selector while waking: {error}");
            }
        }) {
            eprintln!("failed to schedule recording selector WebView wake: {error}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let still_pending = app
            .state::<Arc<AppState>>()
            .recording_selection
            .lock()
            .as_ref()
            .is_some_and(|pending| pending.summary.id == selection.id);
        if !still_pending {
            return;
        }
        if let Some(window) = app.get_webview_window("recording-selector")
            && let Err(error) = window.emit("recording-selection-ready", &selection)
        {
            eprintln!("failed to redeliver recording selector state after wake: {error}");
        }
    });
}

#[tauri::command]
pub fn show_recording_selector(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    selection_id: String,
) -> Result<(), String> {
    let available = state
        .recording_selection
        .lock()
        .as_ref()
        .is_some_and(|selection| selection.summary.id == selection_id);
    if !available {
        return Err(AppError::SessionUnavailable.to_string());
    }
    let window = app
        .get_webview_window("recording-selector")
        .ok_or_else(|| "recording selector is unavailable".to_owned())?;
    window
        .set_ignore_cursor_events(true)
        .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn reveal_recording_selector(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    selection_id: String,
) -> Result<(), String> {
    let available = state
        .recording_selection
        .lock()
        .as_ref()
        .is_some_and(|selection| selection.summary.id == selection_id);
    if !available {
        return Err(AppError::SessionUnavailable.to_string());
    }
    let window = app
        .get_webview_window("recording-selector")
        .ok_or_else(|| "recording selector is unavailable".to_owned())?;
    #[cfg(target_os = "macos")]
    captures_macos_window::reveal_window(&window).map_err(str::to_owned)?;
    window
        .set_ignore_cursor_events(false)
        .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    focus_recording_window(&app, "recording-selector");
    // Focus is helpful for Escape-key handling, but macOS can temporarily
    // reject it for an accessory app. The selector is already visible and
    // interactive, so do not turn that harmless failure into a hidden window.
    if let Err(error) = window.set_focus() {
        eprintln!("failed to focus recording selector: {error}");
    }
    Ok(())
}

fn prepare_recording_hud(app: &AppHandle, display: &DisplayDescriptor) -> Result<(), AppError> {
    let x = f64::from(display.x) + (f64::from(display.width) - RECORDING_HUD_FULL_WIDTH) / 2.0;
    let y = f64::from(display.y) + f64::from(display.height)
        - RECORDING_HUD_HEIGHT
        - RECORDING_HUD_BOTTOM_MARGIN;
    if app.get_webview_window("recording-hud").is_none() {
        WebviewWindowBuilder::new(
            app,
            "recording-hud",
            WebviewUrl::App("index.html?view=recording-hud".into()),
        )
        .title("Captures Recording Controls")
        .inner_size(RECORDING_HUD_FULL_WIDTH, RECORDING_HUD_HEIGHT)
        .position(x, y)
        .decorations(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .transparent(true)
        .background_color(Color(0, 0, 0, 0))
        .visible(false)
        .build()?;
    }
    let window = app
        .get_webview_window("recording-hud")
        .ok_or_else(|| AppError::Task("recording controls are unavailable".to_owned()))?;
    window.set_size(tauri::LogicalSize::new(
        RECORDING_HUD_FULL_WIDTH,
        RECORDING_HUD_HEIGHT,
    ))?;
    window.set_position(tauri::LogicalPosition::new(x, y))?;
    window.set_content_protected(false)?;
    window.hide()?;
    Ok(())
}

#[tauri::command]
pub fn hide_recording_hud(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    let available = state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .is_some_and(|snapshot| snapshot.id == session_id && !snapshot.state.is_terminal());
    if !available {
        return Err(AppError::SessionUnavailable.to_string());
    }
    let window = app.get_webview_window("recording-hud").ok_or_else(|| {
        AppError::Task("recording controls are unavailable".to_owned()).to_string()
    })?;
    window.hide().map_err(|error| error.to_string())
}

fn show_recording_hud(app: &AppHandle) -> Result<(), AppError> {
    let window = app
        .get_webview_window("recording-hud")
        .ok_or_else(|| AppError::Task("recording controls are unavailable".to_owned()))?;
    window.set_content_protected(false)?;
    window.show()?;
    Ok(())
}

fn show_recording_countdown(app: &AppHandle, display: &DisplayDescriptor) -> Result<(), AppError> {
    if app.get_webview_window("recording-countdown").is_none() {
        WebviewWindowBuilder::new(
            app,
            "recording-countdown",
            WebviewUrl::App("index.html?view=recording-countdown".into()),
        )
        .title("Captures Recording Countdown")
        .inner_size(f64::from(display.width), f64::from(display.height))
        .position(f64::from(display.x), f64::from(display.y))
        .decorations(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .transparent(true)
        .background_color(Color(0, 0, 0, 0))
        .focused(true)
        .visible(false)
        .build()?;
    }
    let window = app
        .get_webview_window("recording-countdown")
        .ok_or_else(|| AppError::Task("recording countdown is unavailable".to_owned()))?;
    window.set_size(tauri::LogicalSize::new(
        f64::from(display.width),
        f64::from(display.height),
    ))?;
    window.set_position(tauri::LogicalPosition::new(
        f64::from(display.x),
        f64::from(display.y),
    ))?;
    window.set_content_protected(false)?;
    window.show()?;
    #[cfg(target_os = "macos")]
    focus_recording_window(app, "recording-countdown");
    if let Err(error) = window.set_focus() {
        eprintln!("failed to focus recording countdown: {error}");
    }
    Ok(())
}

fn destroy_recording_countdown(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("recording-countdown")
        && let Err(error) = window.destroy()
    {
        eprintln!("failed to close recording countdown: {error}");
    }
}

fn show_recording_editor(app: &AppHandle, artifact_id: &str) -> Result<(), AppError> {
    let label = format!("recording-editor-{artifact_id}");
    if let Some(window) = app.get_webview_window(&label) {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App(
            format!("index.html?view=recording-editor&artifact_id={artifact_id}").into(),
        ),
    )
    .title("Captures Editor")
    .inner_size(1_100.0, 760.0)
    .min_inner_size(760.0, 560.0)
    .center()
    .resizable(true)
    .background_color(Color(24, 25, 29, 255))
    .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use captures_recording::RecordingState;
    use tempfile::tempdir;

    use super::{
        replace_recording_source, replacement_working_path, screenshot_capture_is_blocked_for,
    };

    #[test]
    fn permits_screenshots_while_recording_or_paused() {
        assert!(!screenshot_capture_is_blocked_for(
            false,
            Some(RecordingState::Recording)
        ));
        assert!(!screenshot_capture_is_blocked_for(
            false,
            Some(RecordingState::Paused)
        ));
        assert!(!screenshot_capture_is_blocked_for(
            false,
            Some(RecordingState::Failed)
        ));
    }

    #[test]
    fn blocks_screenshots_during_recording_setup_and_finalization() {
        assert!(screenshot_capture_is_blocked_for(true, None));
        assert!(screenshot_capture_is_blocked_for(
            false,
            Some(RecordingState::Countdown)
        ));
        assert!(screenshot_capture_is_blocked_for(
            false,
            Some(RecordingState::Finalizing)
        ));
    }

    #[test]
    fn replaces_a_recording_only_after_the_new_file_exists() {
        let directory = tempdir().expect("temporary directory");
        let source = directory.path().join("recording.mp4");
        let replacement = replacement_working_path(&source, "mp4").expect("working path");
        std::fs::write(&source, b"original").expect("source");
        std::fs::write(&replacement, b"edited").expect("replacement");

        replace_recording_source(&source, &replacement).expect("source replaced");

        assert_eq!(std::fs::read(&source).expect("saved source"), b"edited");
        assert!(!replacement.exists());
    }

    #[test]
    fn restores_the_original_when_replacement_fails() {
        let directory = tempdir().expect("temporary directory");
        let source = directory.path().join("recording.mp4");
        let missing_replacement = directory.path().join("missing.mp4");
        std::fs::write(&source, b"original").expect("source");

        assert!(replace_recording_source(&source, &missing_replacement).is_err());
        assert_eq!(
            std::fs::read(&source).expect("restored source"),
            b"original"
        );
    }
}
