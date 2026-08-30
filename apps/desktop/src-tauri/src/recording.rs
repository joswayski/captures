use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use captures_capture::{CaptureMode, DisplayDescriptor};
use captures_media::{
    ByteRange, CancelToken, EditSpec, ExportFormat, ExportProgress, ExportSpec, MediaToolError,
    MediaToolchain, QualityPreset, RecordingAudioLayout, RecordingSegmentInput, TimelineSpriteSpec,
    estimate_sample_windows, export_preserves_source_bytes, extrapolate_sampled_size,
    visual_edit_is_identity,
};
use captures_recording::{
    DraftStore, RecordingCoordinator, RecordingDraftManifest, RecordingKind, RecordingOptions,
    RecordingSegmentInfo, RecordingSegmentManifest, RecordingSessionSnapshot, RecordingState,
    RecordingTarget,
};
#[cfg(target_os = "macos")]
use captures_recording_macos::MacRecordingSegment as NativeRecordingSegment;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use captures_recording_xcap::XcapRecordingSegment as NativeRecordingSegment;
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, CursorIcon, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    window::Color,
};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::{
    AppError,
    models::{
        CaptureArtifact, CaptureSelectorMode, HistoryEntry, RecordingArtifact,
        RecordingArtifactData, RecordingCapabilities, RecordingSelection,
        RecordingSelectionSession,
        recording_controls_are_excluded as controls_excluded_for_preference, recording_media_url,
        recording_poster_url, recording_recovery_directory, recording_selection_url,
        recording_timeline_url,
    },
    state::AppState,
    storage,
};

const RECORDING_STATE_EVENT: &str = "recording-state-changed";
const RECORDING_COUNTDOWN_EVENT: &str = "recording-countdown";
const RECORDING_WARNING_EVENT: &str = "recording-warning";
const RECORDING_ARTIFACT_EVENT: &str = "recording-artifact-ready";
pub(crate) const RECORDING_REGION_INDICATOR_LABEL: &str = "recording-region-indicator";
pub(crate) const RECORDING_REGION_INDICATOR_TITLE: &str = "Captures Recording Region";
#[cfg(target_os = "macos")]
const RECORDING_COUNTDOWN_FADE_OUT_MS: u64 = 180;
const RECORDING_HUD_FULL_WIDTH: f64 = 430.0;
const RECORDING_HUD_HEIGHT: f64 = 102.0;
const RECORDING_HUD_BOTTOM_MARGIN: f64 = 20.0;
/// Cap un-ranged `captures-capture://media/...` reads so a missing Range header
/// cannot pull an entire recording into the desktop process.
const DEFAULT_UNRANGED_MEDIA_BYTES: u64 = 2 * 1024 * 1024;
const GIF_SOURCE_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Default)]
pub struct RecordingRuntime {
    coordinator: RecordingCoordinator,
    session: Option<RuntimeSession>,
    generation: u64,
    exports: HashMap<String, CancelToken>,
    /// In-flight export size estimates, keyed by artifact ID. A newer request
    /// cancels and replaces the previous one for the same artifact.
    estimates: HashMap<String, CancelToken>,
    /// In-flight before/after frame previews, keyed by artifact ID.
    previews: HashMap<String, CancelToken>,
}

struct RuntimeSession {
    id: String,
    options: RecordingOptions,
    directory: PathBuf,
    manifest: RecordingDraftManifest,
    active_segment: Option<NativeRecordingSegment>,
    active_segment_started_at_ms: Option<u64>,
    poster_png: Vec<u8>,
    display: DisplayDescriptor,
}

pub fn screenshot_capture_is_blocked(state: &AppState) -> bool {
    let recording_state = state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .map(|snapshot| snapshot.state);
    screenshot_capture_is_blocked_for(recording_state)
}

const fn screenshot_capture_is_blocked_for(recording_state: Option<RecordingState>) -> bool {
    matches!(
        recording_state,
        Some(
            RecordingState::Selecting
                | RecordingState::Countdown
                | RecordingState::Finalizing
                | RecordingState::Editor
        )
    )
}

const fn recording_in_progress_for(recording_state: Option<RecordingState>) -> bool {
    matches!(
        recording_state,
        Some(
            RecordingState::Countdown
                | RecordingState::Recording
                | RecordingState::Paused
                | RecordingState::Finalizing
        )
    )
}

pub(crate) fn recording_session_is_active(state: &AppState) -> bool {
    state
        .recording
        .lock()
        .coordinator
        .snapshot(now_ms())
        .is_some_and(|snapshot| !snapshot.state.is_terminal())
}

pub(crate) fn recording_in_progress(state: &AppState) -> bool {
    recording_in_progress_for(
        state
            .recording
            .lock()
            .coordinator
            .snapshot(now_ms())
            .map(|snapshot| snapshot.state),
    )
}

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

#[derive(Clone, Debug, Deserialize)]
pub struct CaptureSelectionScreenshotRequest {
    pub selection_id: String,
    pub target: RecordingTarget,
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
    pub reveal_error: Option<String>,
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

pub async fn prepare_recording_inner(
    app: AppHandle,
    state: Arc<AppState>,
) -> Result<RecordingSelectionSession, AppError> {
    prepare_capture_selector_inner(
        app,
        state,
        CaptureSelectorMode::Recording,
        CaptureMode::Region,
    )
    .await
}

pub(crate) async fn prepare_capture_selector_inner(
    app: AppHandle,
    state: Arc<AppState>,
    initial_mode: CaptureSelectorMode,
    initial_target: CaptureMode,
) -> Result<RecordingSelectionSession, AppError> {
    crate::ensure_capture_session_available()?;
    if crate::updates::install_is_active(&app) {
        return Err(AppError::UpdateInstalling);
    }
    if !state.sessions.lock().is_empty()
        || recording_session_is_active(&state)
        || crate::screenshot_countdown_is_active(&state)
    {
        return Err(AppError::CaptureInProgress);
    }
    let pending_selection = {
        let mut pending = state.recording_selection.lock();
        pending.as_mut().map(|selection| {
            selection.summary.initial_mode = initial_mode;
            selection.summary.initial_target = initial_target;
            selection.summary.clone()
        })
    };
    if let Some(summary) = pending_selection {
        let include_mini_previews = state.settings().include_mini_previews_in_captures;
        crate::suppress_thumbnail_capture_ui(&state);
        crate::set_capture_huds_protected(&app, true);
        if !include_mini_previews {
            crate::hide_window(&app, "thumbnail");
        }
        crate::hide_window(&app, "mini-previews-hidden");
        crate::hide_window(&app, "startup");
        crate::hide_recording_saved_notices(&app);
        if crate::updates::should_hide_update_notice_for_capture(&app) {
            crate::updates::defer_visible_notice(&app);
            crate::hide_window(&app, "update");
        }
        if let Err(error) = prepare_recording_selector(&app, &summary, true).await {
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

    crate::suppress_thumbnail_capture_ui(&state);
    crate::hide_capture_huds_before_snapshot(&app).await;
    let freeze_screen = state.settings().freeze_screen;
    let prepared = (|| {
        crate::ensure_capture_session_available()?;
        let id = Uuid::new_v4().to_string();
        let (display, snapshot_png, image, displays, windows) = if freeze_screen {
            let (frame, snapshot_png, mut displays, mut windows) = std::thread::scope(|scope| {
                // The selector always needs window targets, but their enumeration is
                // independent of freezing and encoding the display background.
                let windows = scope.spawn(|| state.windows());
                let frame = state
                    .backend
                    .capture_display_at_point(crate::pointer_position())?;
                let snapshot_png = storage::encode_png(&frame.image)?;
                let displays = state.monitors()?;
                let windows = match windows.join() {
                    Ok(windows) => windows,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
                .unwrap_or_else(|error| {
                    eprintln!("window targets are unavailable for this capture: {error}");
                    Vec::new()
                });
                Ok::<_, AppError>((frame, snapshot_png, displays, windows))
            })?;
            if !displays
                .iter()
                .any(|candidate| candidate.id == frame.descriptor.id)
            {
                displays.push(frame.descriptor.clone());
            }
            windows.retain(|window| crate::window_is_capturable(window, &frame.descriptor));
            crate::refine_window_chrome_from_snapshot(
                &mut windows,
                &frame.descriptor,
                &frame.image,
                crate::window_corner_radius_points(),
            );
            (
                frame.descriptor,
                snapshot_png,
                Some(frame.image),
                displays,
                windows,
            )
        } else {
            let display = crate::display_under_pointer(&state)?;
            let mut displays = state.monitors()?;
            if !displays.iter().any(|candidate| candidate.id == display.id) {
                displays.push(display.clone());
            }
            let mut windows = state.windows().unwrap_or_else(|error| {
                eprintln!("window targets are unavailable for this capture: {error}");
                Vec::new()
            });
            windows.retain(|window| crate::window_is_capturable(window, &display));
            (display, Vec::new(), None, displays, windows)
        };
        let summary = RecordingSelectionSession {
            id: id.clone(),
            // Every capture starts from a high-quality video master. The editor
            // decides whether the final copy is video or GIF.
            kind: RecordingKind::Video,
            initial_mode,
            initial_target,
            recording_available: cfg!(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux"
            )),
            recording_capabilities: RecordingCapabilities::current(
                state.settings().include_recording_controls_in_captures,
            ),
            window_coordinate_scale: crate::window_coordinate_scale(&display),
            window_corner_radius: crate::window_corner_radius_points(),
            display_corner_radius: crate::display_corner_radius_points(&display.id),
            display,
            displays,
            frozen: freeze_screen,
            snapshot_url: if freeze_screen {
                recording_selection_url(&id)
            } else {
                String::new()
            },
            windows,
        };
        *state.recording_selection.lock() = Some(RecordingSelection {
            summary: summary.clone(),
            image,
            snapshot_png,
        });
        Ok::<_, AppError>(summary)
    })();
    match prepared {
        Ok(summary) => {
            if let Err(error) = prepare_recording_selector(&app, &summary, true).await {
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
pub async fn select_capture_display(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    selection_id: String,
    display_id: String,
) -> Result<RecordingSelectionSession, String> {
    select_capture_display_inner(app, state.inner().clone(), &selection_id, &display_id)
        .await
        .map_err(|error| error.to_string())
}

async fn select_capture_display_inner(
    app: AppHandle,
    state: Arc<AppState>,
    selection_id: &str,
    display_id: &str,
) -> Result<RecordingSelectionSession, AppError> {
    crate::ensure_capture_session_available()?;
    let current = state
        .recording_selection
        .lock()
        .as_ref()
        .filter(|selection| selection.summary.id == selection_id)
        .map(|selection| selection.summary.clone())
        .ok_or(AppError::SessionUnavailable)?;
    if current.display.id == display_id {
        return Ok(current);
    }

    let mut displays = state.monitors()?;
    let requested_display = displays
        .iter()
        .find(|display| display.id == display_id)
        .cloned()
        .ok_or(AppError::InvalidSelection)?;
    let freeze_screen = current.frozen;
    let (display, snapshot_png, image, windows) = if freeze_screen {
        let frame = state.backend.capture_display(&requested_display.id)?;
        let snapshot_png = storage::encode_png(&frame.image)?;
        let mut windows = state
            .windows()
            .unwrap_or_else(|error| {
                eprintln!("window targets are unavailable for this display: {error}");
                Vec::new()
            })
            .into_iter()
            .filter(|window| crate::window_is_capturable(window, &frame.descriptor))
            .collect::<Vec<_>>();
        crate::refine_window_chrome_from_snapshot(
            &mut windows,
            &frame.descriptor,
            &frame.image,
            crate::window_corner_radius_points(),
        );
        if let Some(display) = displays
            .iter_mut()
            .find(|candidate| candidate.id == frame.descriptor.id)
        {
            *display = frame.descriptor.clone();
        }
        (frame.descriptor, snapshot_png, Some(frame.image), windows)
    } else {
        let mut windows = state.windows().unwrap_or_else(|error| {
            eprintln!("window targets are unavailable for this display: {error}");
            Vec::new()
        });
        windows.retain(|window| crate::window_is_capturable(window, &requested_display));
        (requested_display, Vec::new(), None, windows)
    };

    let mut summary = current;
    summary.display = display;
    summary.displays = displays;
    summary.window_coordinate_scale = crate::window_coordinate_scale(&summary.display);
    summary.display_corner_radius = crate::display_corner_radius_points(&summary.display.id);
    summary.frozen = freeze_screen;
    summary.snapshot_url = if freeze_screen {
        format!(
            "{}?refresh={}",
            recording_selection_url(&summary.id),
            Uuid::new_v4()
        )
    } else {
        String::new()
    };
    summary.windows = windows;
    let replacement = RecordingSelection {
        summary: summary.clone(),
        image,
        snapshot_png,
    };
    let previous = {
        let mut pending = state.recording_selection.lock();
        if pending
            .as_ref()
            .is_none_or(|selection| selection.summary.id != selection_id)
        {
            return Err(AppError::SessionUnavailable);
        }
        pending
            .replace(replacement)
            .ok_or(AppError::SessionUnavailable)?
    };
    if let Err(error) = prepare_recording_selector(&app, &summary, false).await {
        let previous_summary = previous.summary.clone();
        *state.recording_selection.lock() = Some(previous);
        let _ = prepare_recording_selector(&app, &previous_summary, false).await;
        return Err(error);
    }
    Ok(summary)
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
    restore_after_recording_selection(app, state);
    Ok(())
}

fn restore_after_recording_selection(app: &AppHandle, state: &Arc<AppState>) {
    destroy_recording_selector(app);
    #[cfg(target_os = "macos")]
    captures_macos_window::restore_frontmost_app_after_capture();
    crate::reveal_document_windows_after_capture(app);
    crate::set_capture_huds_protected(app, false);
    crate::restore_thumbnail_capture_ui(app, state);
}

pub(crate) fn dismiss_open_recording_selection(app: &AppHandle, state: &Arc<AppState>) -> bool {
    let mut selection = state.recording_selection.lock();
    if selection.is_none() {
        return false;
    }
    *selection = None;
    drop(selection);
    restore_after_recording_selection(app, state);
    true
}

pub(crate) fn dismiss_recording_selection_for_update(app: &AppHandle, state: &Arc<AppState>) {
    let _ = dismiss_open_recording_selection(app, state);
}

#[tauri::command]
pub async fn capture_selection_screenshot(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    request: CaptureSelectionScreenshotRequest,
) -> Result<CaptureArtifact, String> {
    capture_selection_screenshot_inner(app, state.inner().clone(), request)
        .await
        .map_err(|error| error.to_string())
}

async fn capture_selection_screenshot_inner(
    app: AppHandle,
    state: Arc<AppState>,
    request: CaptureSelectionScreenshotRequest,
) -> Result<CaptureArtifact, AppError> {
    crate::ensure_capture_session_available()?;
    if crate::screenshot_countdown_is_active(&state) {
        return Err(AppError::CaptureInProgress);
    }
    let available = state
        .recording_selection
        .lock()
        .as_ref()
        .is_some_and(|selection| selection.summary.id == request.selection_id);
    if !available {
        return Err(AppError::SessionUnavailable);
    }
    let thumbnail_capture_generation = crate::begin_thumbnail_capture(&state)?;
    let selection = {
        let mut pending = state.recording_selection.lock();
        match pending.take() {
            Some(selection) if selection.summary.id == request.selection_id => selection,
            Some(selection) => {
                *pending = Some(selection);
                drop(pending);
                crate::restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(AppError::SessionUnavailable);
            }
            None => {
                drop(pending);
                crate::restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(AppError::SessionUnavailable);
            }
        }
    };
    destroy_recording_selector(&app);
    // Screenshot-from-controls is done with the selector; hand focus back so an
    // already-open editor does not stay covering the app the user was using.
    // Document windows stay ordered out until this path finishes or fails.
    #[cfg(target_os = "macos")]
    captures_macos_window::restore_frontmost_app_after_capture();
    let _reveal_documents = crate::RevealDocumentWindowsOnDrop::new(&app);
    crate::restore_thumbnail_capture_ui(&app, &state);
    crate::set_capture_huds_protected(&app, false);

    if let Err(error) = validate_target(&selection.summary, &request.target) {
        crate::restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
        return Err(error);
    }
    let mode = capture_mode_for_target(&request.target);
    let countdown_seconds = state.settings().screenshot_countdown_seconds;
    let image = if countdown_seconds > 0 {
        // Delay, then recapture live so menus and hover states are current.
        match crate::run_screenshot_countdown(
            app.clone(),
            state.clone(),
            &selection.summary.display,
            countdown_seconds,
            thumbnail_capture_generation,
        )
        .await
        {
            Ok(true) => match live_image_for_target(&state, &request.target) {
                Ok(image) => image,
                Err(error) => {
                    crate::restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                    return Err(error);
                }
            },
            Ok(false) => {
                // Cancel already restored the stack.
                return Err(AppError::ScreenshotCancelled);
            }
            Err(error) => {
                crate::restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error);
            }
        }
    } else {
        match pixels_for_selection(&state, &selection, &request.target) {
            Ok(image) => image,
            Err(error) => {
                crate::restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error);
            }
        }
    };

    let result =
        crate::finish_capture(&app, &state, mode, image, thumbnail_capture_generation).await;
    if result.is_err() {
        crate::restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
    }
    result
}

fn live_image_for_target(
    state: &AppState,
    target: &RecordingTarget,
) -> Result<image::RgbaImage, AppError> {
    crate::ensure_capture_session_available()?;
    match target {
        RecordingTarget::Display { display_id } => {
            Ok(state.backend.capture_display(display_id)?.image)
        }
        RecordingTarget::Region { display_id, rect } => {
            let frame = state.backend.capture_display(display_id)?;
            crop_display_region(&frame.image, &frame.descriptor, rect)
        }
        RecordingTarget::Window { window_id } => match state.backend.capture_window(window_id) {
            Ok(image) if !crate::image_is_effectively_blank(&image) => Ok(image),
            _ => {
                let windows = state.windows().unwrap_or_default();
                let window = windows
                    .iter()
                    .find(|window| &window.id == window_id)
                    .ok_or(AppError::InvalidSelection)?;
                let frame = state.backend.capture_display(&window.display_id)?;
                crop_window_from_display(&frame.image, &frame.descriptor, window)
            }
        },
    }
}

fn crop_display_region(
    image: &image::RgbaImage,
    display: &DisplayDescriptor,
    rect: &captures_recording::CaptureRect,
) -> Result<image::RgbaImage, AppError> {
    // Region rects come from the overlay in CSS/DIP space.
    let scale = display.overlay_to_buffer_scale(image.width(), image.height());
    let x = (f64::from(rect.x) * scale).round().max(0.0) as u32;
    let y = (f64::from(rect.y) * scale).round().max(0.0) as u32;
    let width = (f64::from(rect.width) * scale).round().max(1.0) as u32;
    let height = (f64::from(rect.height) * scale).round().max(1.0) as u32;
    Ok(image::imageops::crop_imm(
        image,
        x.min(image.width().saturating_sub(1)),
        y.min(image.height().saturating_sub(1)),
        width.min(image.width().saturating_sub(x)),
        height.min(image.height().saturating_sub(y)),
    )
    .to_image())
}

fn crop_window_from_display(
    image: &image::RgbaImage,
    display: &DisplayDescriptor,
    window: &captures_capture::WindowDescriptor,
) -> Result<image::RgbaImage, AppError> {
    let scale_x = f64::from(image.width()) / f64::from(display.width.max(1));
    let scale_y = f64::from(image.height()) / f64::from(display.height.max(1));
    let x = (f64::from(window.x - display.x) * scale_x).round().max(0.0) as u32;
    let y = (f64::from(window.y - display.y) * scale_y).round().max(0.0) as u32;
    let width = (f64::from(window.width) * scale_x).round().max(1.0) as u32;
    let height = (f64::from(window.height) * scale_y).round().max(1.0) as u32;
    Ok(image::imageops::crop_imm(
        image,
        x.min(image.width().saturating_sub(1)),
        y.min(image.height().saturating_sub(1)),
        width.min(image.width().saturating_sub(x)),
        height.min(image.height().saturating_sub(y)),
    )
    .to_image())
}

#[tauri::command]
pub fn list_recording_audio_devices() -> Vec<captures_recording::AudioDevice> {
    #[cfg(target_os = "macos")]
    {
        captures_recording_macos::microphone_devices()
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        captures_recording_xcap::microphone_devices()
    }
}

#[tauri::command]
pub fn recording_controls_are_excluded(state: tauri::State<'_, Arc<AppState>>) -> bool {
    controls_excluded_for_preference(state.settings().include_recording_controls_in_captures)
}

/// Whether this session should keep Captures recording chrome out of the
/// native recorder (macOS ScreenCaptureKit application exclusion).
pub(crate) fn should_exclude_captures_app_from_recording(state: &AppState) -> bool {
    controls_excluded_for_preference(state.settings().include_recording_controls_in_captures)
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
    crate::ensure_capture_session_available()?;
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
        let poster_png = poster_for_selection(&state, &selection, &request.options.target)?;
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
    let recording_ui = async {
        prepare_recording_hud(&app, &selected_display).await?;
        prepare_recording_region_indicator(&app, &selected_display, &snapshot.options.target)
            .await?;
        if snapshot.options.countdown_seconds > 0 {
            show_recording_countdown(&app, &selected_display)?;
        }
        Ok::<_, AppError>(())
    }
    .await;
    if let Err(error) = recording_ui {
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

#[cfg(target_os = "macos")]
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

fn start_native_segment(
    options: &RecordingOptions,
    path: &Path,
    display: &DisplayDescriptor,
    exclude_captures_app: bool,
) -> Result<NativeRecordingSegment, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = display;
        NativeRecordingSegment::start(options, path, exclude_captures_app)
            .map_err(|error| error.to_string())
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let _ = exclude_captures_app;
        NativeRecordingSegment::start(options, path, display).map_err(|error| error.to_string())
    }
}

fn play_recording_start_chime() {
    #[cfg(target_os = "macos")]
    captures_recording_macos::play_start_chime();
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    captures_recording_xcap::play_start_chime();
}

async fn start_segment(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: &str,
    generation: u64,
) -> Result<(), AppError> {
    let (options, path, display) = {
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
            session.display.clone(),
        )
    };

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    destroy_recording_countdown(&app);
    let path_for_start = path.clone();
    let exclude_captures_app = should_exclude_captures_app_from_recording(&state);
    let started = tauri::async_runtime::spawn_blocking(move || {
        start_native_segment(&options, &path_for_start, &display, exclude_captures_app)
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))?;
    let segment = match started {
        Ok(segment) => segment,
        Err(error) => return Err(AppError::Task(error)),
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
            let dimensions = segment.as_ref().map(NativeRecordingSegment::dimensions);
            let system_audio_draft = segment
                .as_ref()
                .and_then(NativeRecordingSegment::system_audio_draft_info);
            let microphone_draft = segment
                .as_ref()
                .and_then(NativeRecordingSegment::microphone_draft_info);
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
            let (system_audio_relative_path, system_audio_offset_ms) = system_audio_draft
                .map(|(system_audio_path, offset_ms)| {
                    let relative_path = system_audio_path
                        .strip_prefix(&session.directory)
                        .map_err(|_| {
                            AppError::Task(
                                "desktop audio segment escaped its recovery bundle".to_owned(),
                            )
                        })?
                        .to_string_lossy()
                        .into_owned();
                    Ok::<_, AppError>((Some(relative_path), offset_ms))
                })
                .transpose()?
                .unwrap_or((None, 0));
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
        play_recording_start_chime();
        #[cfg(target_os = "macos")]
        {
            tokio::time::sleep(Duration::from_millis(RECORDING_COUNTDOWN_FADE_OUT_MS)).await;
            if !recording_segment_is_current(&state, session_id, generation) {
                destroy_recording_countdown(&app);
                captures_macos_window::restore_frontmost_app_after_capture();
                crate::reveal_document_windows_after_capture(&app);
                return Ok(());
            }
        }
    }
    destroy_recording_countdown(&app);
    // Selector/countdown activation can leave editors frontmost. The recording
    // HUD is non-activating, so hand focus back while the user records.
    #[cfg(target_os = "macos")]
    captures_macos_window::restore_frontmost_app_after_capture();
    crate::reveal_document_windows_after_capture(&app);
    show_recording_hud(&app).await?;
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
                let system_audio = segment
                    .system_audio_relative_path
                    .map(|path| session.directory.join(path));
                let microphone = segment
                    .microphone_relative_path
                    .map(|path| session.directory.join(path));
                [Some(video), system_audio, microphone]
                    .into_iter()
                    .flatten()
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
    destroy_recording_region_indicator(&app);

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
                    system_audio_path: segment
                        .system_audio_relative_path
                        .as_ref()
                        .map(|path| session.directory.join(path)),
                    system_audio_offset_ms: segment.system_audio_offset_ms,
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
    let extension = if options.kind == RecordingKind::Video {
        "mp4"
    } else {
        "gif"
    };
    // Assemble into the session workspace first, then promote into private history
    // recovery storage (not the user's Captures folder) so dismiss/delete of the
    // editor can still recover the recording for HISTORY_RETENTION_DAYS.
    let assembled = directory.join(format!("assembled.{extension}"));
    let toolchain = media_toolchain(&app);
    let cancel = CancelToken::default();
    let destination_for_task = assembled.clone();
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
    let mut artifact = RecordingArtifact {
        id: artifact_id.clone(),
        kind: options.kind,
        path: assembled.to_string_lossy().into_owned(),
        saved_path: None,
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
    upsert_recording_artifact(&app, &state, &mut artifact, poster_png.clone());
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
    crate::restore_thumbnail_capture_ui(&app, &state);

    if options.kind == RecordingKind::Video {
        let _ = DraftStore::new(recording_recovery_directory()).remove(session_id);
    }
    let settings = state.settings();
    if settings.recording.open_editor_after_recording
        && let Err(error) = show_recording_editor(&app, &artifact.id)
    {
        eprintln!("recording was saved to history, but the editor could not open: {error}");
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
    destroy_recording_region_indicator(&app);
    destroy_recording_countdown(&app);
    crate::hide_window(&app, "recording-hud");
    crate::restore_thumbnail_capture_ui(&app, &state);
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

/// Result of a background export size estimate for the recording editor.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingExportEstimate {
    pub size_bytes: u64,
    /// True when the whole trimmed range was encoded (or the save is a plain
    /// copy), so the size is exact instead of extrapolated from samples.
    pub exact: bool,
}

/// Before/after frames rendered for the recording editor's compression preview.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingExportPreview {
    pub before_png: Vec<u8>,
    pub after_png: Vec<u8>,
}

/// Duration of the sample encoded for a before/after frame preview.
const PREVIEW_SAMPLE_MS: u64 = 1_500;
/// How far the sample starts ahead of the requested frame so the compared
/// frame lands mid-stream with typical quality instead of on the opening
/// keyframe.
const PREVIEW_FRAME_LEAD_MS: u64 = 1_000;

/// Estimate the saved file size for the current editor settings by encoding
/// short samples of the trimmed range with the same pipeline used to save.
#[tauri::command]
pub async fn estimate_recording_export(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
    mut edit: EditSpec,
    export: ExportSpec,
) -> Result<RecordingExportEstimate, String> {
    if export.format == ExportFormat::WebM {
        return Err("size estimates are not available for WebM".to_owned());
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
    edit.audio.source_has_system_audio = source.has_system_audio;
    edit.audio.source_has_microphone_audio = source.has_microphone_audio;
    let cancel = CancelToken::default();
    if let Some(previous) = state
        .recording
        .lock()
        .estimates
        .insert(artifact_id, cancel.clone())
    {
        previous.cancel();
    }
    let toolchain = media_toolchain(&app);
    let input = PathBuf::from(&source.path);
    tauri::async_runtime::spawn_blocking(move || {
        estimate_export_size(&toolchain, &input, &edit, &export, &cancel)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

/// Render matching before/after frames for the recording editor's compression
/// preview by encoding a short sample around `at_ms` with the save pipeline.
#[tauri::command]
pub async fn preview_recording_export(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
    mut edit: EditSpec,
    export: ExportSpec,
    at_ms: u64,
) -> Result<RecordingExportPreview, String> {
    if export.format == ExportFormat::WebM {
        return Err("previews are not available for WebM".to_owned());
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
    edit.audio.source_has_system_audio = source.has_system_audio;
    edit.audio.source_has_microphone_audio = source.has_microphone_audio;
    let cancel = CancelToken::default();
    if let Some(previous) = state
        .recording
        .lock()
        .previews
        .insert(artifact_id, cancel.clone())
    {
        previous.cancel();
    }
    let toolchain = media_toolchain(&app);
    let input = PathBuf::from(&source.path);
    tauri::async_runtime::spawn_blocking(move || {
        render_export_preview(&toolchain, &input, &edit, &export, at_ms, &cancel)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

fn estimate_export_size(
    toolchain: &MediaToolchain,
    input: &Path,
    edit: &EditSpec,
    export: &ExportSpec,
    cancel: &CancelToken,
) -> Result<RecordingExportEstimate, MediaToolError> {
    let probe = toolchain.probe(input)?;
    if export_preserves_source_bytes(&probe, edit, export) {
        return Ok(RecordingExportEstimate {
            size_bytes: probe.metadata.size_bytes,
            exact: true,
        });
    }
    if export.format == ExportFormat::Mp4
        && export.quality == QualityPreset::Preserve
        && export.max_size_bytes.is_none()
        && visual_edit_is_identity(&probe, edit)
    {
        // Only the audio track is re-encoded; the copied video stream
        // dominates the file, so the source size is a close estimate.
        return Ok(RecordingExportEstimate {
            size_bytes: probe.metadata.size_bytes,
            exact: false,
        });
    }
    let source_duration_ms = probe
        .metadata
        .duration_ms
        .ok_or(MediaToolError::IncompleteMetadata)?;
    let trim_end_ms = edit
        .trim_end_ms
        .unwrap_or(source_duration_ms)
        .min(source_duration_ms);
    let trimmed_ms = trim_end_ms.saturating_sub(edit.trim_start_ms).max(1);
    let windows = estimate_sample_windows(edit.trim_start_ms, trimmed_ms);
    let exact = windows.len() == 1;
    let extension = if export.format == ExportFormat::Gif {
        "gif"
    } else {
        "mp4"
    };
    let scratch = std::env::temp_dir().join(format!("captures-estimate-{}", Uuid::new_v4()));
    fs::create_dir_all(&scratch)?;
    let mut sampled_bytes: u64 = 0;
    let mut sampled_ms: u64 = 0;
    let result = (|| {
        for (index, (start_ms, window_ms)) in windows.iter().copied().enumerate() {
            if cancel.is_cancelled() {
                return Err(MediaToolError::Cancelled);
            }
            let mut sample_edit = edit.clone();
            sample_edit.trim_start_ms = start_ms;
            sample_edit.trim_end_ms = Some(start_ms + window_ms);
            let sample_export = sampled_export_spec(export, window_ms, trimmed_ms);
            let destination = scratch.join(format!("sample-{index}.{extension}"));
            let outcome = toolchain.export(
                input,
                &destination,
                &sample_edit,
                &sample_export,
                cancel,
                |_| {},
            )?;
            sampled_bytes = sampled_bytes.saturating_add(outcome.size_bytes);
            sampled_ms = sampled_ms.saturating_add(window_ms);
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&scratch);
    result?;
    Ok(RecordingExportEstimate {
        size_bytes: extrapolate_sampled_size(sampled_bytes, sampled_ms, trimmed_ms),
        exact,
    })
}

fn render_export_preview(
    toolchain: &MediaToolchain,
    input: &Path,
    edit: &EditSpec,
    export: &ExportSpec,
    at_ms: u64,
    cancel: &CancelToken,
) -> Result<RecordingExportPreview, MediaToolError> {
    let probe = toolchain.probe(input)?;
    let source_duration_ms = probe
        .metadata
        .duration_ms
        .ok_or(MediaToolError::IncompleteMetadata)?;
    let trim_start_ms = edit.trim_start_ms.min(source_duration_ms.saturating_sub(1));
    let trim_end_ms = edit
        .trim_end_ms
        .unwrap_or(source_duration_ms)
        .min(source_duration_ms)
        .max(trim_start_ms + 1);
    let trimmed_ms = trim_end_ms - trim_start_ms;
    let at = at_ms.clamp(trim_start_ms, trim_end_ms - 1);
    let window_ms = PREVIEW_SAMPLE_MS.min(trimmed_ms);
    let start_ms = at
        .saturating_sub(PREVIEW_FRAME_LEAD_MS)
        .min(trim_end_ms - window_ms)
        .max(trim_start_ms);
    let mut sample_edit = edit.clone();
    sample_edit.trim_start_ms = start_ms;
    sample_edit.trim_end_ms = Some(start_ms + window_ms);
    let sample_export = sampled_export_spec(export, window_ms, trimmed_ms);
    let extension = if export.format == ExportFormat::Gif {
        "gif"
    } else {
        "mp4"
    };
    let scratch = std::env::temp_dir().join(format!("captures-preview-{}", Uuid::new_v4()));
    fs::create_dir_all(&scratch)?;
    let result = (|| {
        let sample_path = scratch.join(format!("sample.{extension}"));
        toolchain.export(
            input,
            &sample_path,
            &sample_edit,
            &sample_export,
            cancel,
            |_| {},
        )?;
        let after_path = scratch.join("after.png");
        toolchain.extract_frame(&sample_path, at - start_ms, &after_path, cancel)?;
        let before_path = scratch.join("before.png");
        toolchain.extract_edited_frame(input, edit, export, at, &before_path, cancel)?;
        Ok(RecordingExportPreview {
            before_png: fs::read(&before_path)?,
            after_png: fs::read(&after_path)?,
        })
    })();
    let _ = fs::remove_dir_all(&scratch);
    result
}

/// Give a sampled window a proportional share of a whole-export size cap so
/// bitrate selection matches the real save.
fn sampled_export_spec(export: &ExportSpec, window_ms: u64, trimmed_ms: u64) -> ExportSpec {
    let mut sample = export.clone();
    sample.max_size_bytes = export.max_size_bytes.map(|cap| {
        u64::try_from(u128::from(cap) * u128::from(window_ms) / u128::from(trimmed_ms.max(1)))
            .unwrap_or(cap)
            .max(1)
    });
    sample
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
        return Err("changing the file format requires saving a new file".to_owned());
    }
    let selected_directory = request
        .destination_directory
        .as_deref()
        .filter(|directory| !directory.is_empty())
        .map(Path::new);
    let permanent_source = source
        .saved_path
        .as_deref()
        .map(Path::new)
        .filter(|path| path.is_file());
    let final_destination = match (
        request.overwrite_source,
        selected_directory,
        permanent_source,
    ) {
        (true, directory, Some(permanent)) => {
            storage::recording_replacement_destination_path_in_with_replaceable(
                &source_path,
                directory,
                &request.file_stem,
                extension,
                &[permanent],
            )
        }
        (true, directory, None) => storage::recording_replacement_destination_path_in(
            &source_path,
            directory,
            &request.file_stem,
            extension,
        ),
        (false, Some(directory), _) => storage::recording_destination_path_in(
            &source_path,
            Some(directory),
            &request.file_stem,
            extension,
        ),
        (false, None, _) => {
            storage::recording_destination_path(&source_path, &request.file_stem, extension)
        }
    }
    .map_err(|error| error.to_string())?;
    let working_destination = if request.overwrite_source {
        replacement_working_path(&final_destination, extension)
            .map_err(|error| error.to_string())?
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
    let requested_file_stem = request.file_stem.clone();
    let requested_destination_directory = request.destination_directory.clone();
    tauri::async_runtime::spawn(async move {
        let toolchain = media_toolchain(&app);
        let input = PathBuf::from(&source.path);
        let task_app = app.clone();
        let progress_export_id = task_export_id.clone();
        let edit = request.edit.clone();
        let export = request.export.clone();
        let overwrite_source = request.overwrite_source;
        let replacement_destination = final_destination.clone();
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
                replace_recording_source_at(&input, &outcome.path, &replacement_destination)?;
                outcome.path = replacement_destination;
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
                let exported_path = outcome.path.to_string_lossy().into_owned();
                let recovery_path = PathBuf::from(&source.path);
                // Prefer the user-facing Captures path:
                // - Save as new file / format change → the export path itself
                // - Overwrite that already landed outside recovery media → that path
                // - Overwrite of recovery media with an existing permanent save → keep it
                // - First history-only Save that only rewrote recovery media → promote
                //   using the editor's chosen folder + filename when possible
                let mut permanent_path = if request.overwrite_source {
                    if outcome.path != recovery_path {
                        Some(exported_path.clone())
                    } else {
                        source
                            .saved_path
                            .clone()
                            .filter(|path| Path::new(path).is_file())
                    }
                } else {
                    Some(exported_path.clone())
                };
                if let Some(saved_path) = permanent_path.as_ref()
                    && Path::new(saved_path) != outcome.path.as_path()
                {
                    if let Err(error) = fs::copy(&outcome.path, saved_path) {
                        eprintln!(
                            "recording export completed, but the Captures folder copy could not be updated: {error}"
                        );
                    }
                } else if permanent_path.is_none() {
                    let settings = state.settings();
                    let preferred = requested_destination_directory
                        .as_deref()
                        .map(str::trim)
                        .filter(|directory| !directory.is_empty())
                        .map(PathBuf::from)
                        .filter(|directory| directory.is_dir())
                        .map(|directory| {
                            directory.join(format!("{}.{}", requested_file_stem.trim(), extension))
                        })
                        .filter(|destination| {
                            !destination.exists() || destination == outcome.path.as_path()
                        });
                    let promote = preferred.or_else(|| {
                        storage::unique_media_path(Path::new(&settings.output_directory), extension)
                            .ok()
                    });
                    match promote {
                        Some(destination) => {
                            let copied = if destination == outcome.path {
                                true
                            } else {
                                match fs::copy(&outcome.path, &destination) {
                                    Ok(_) => true,
                                    Err(error) => {
                                        eprintln!(
                                            "recording export completed, but a Captures folder copy could not be created: {error}"
                                        );
                                        false
                                    }
                                }
                            };
                            if copied {
                                permanent_path = Some(destination.to_string_lossy().into_owned());
                            }
                        }
                        None => {
                            eprintln!(
                                "recording export completed, but a Captures folder path could not be prepared"
                            );
                        }
                    }
                }
                let mut artifact = RecordingArtifact {
                    id: artifact_id.clone(),
                    kind: if extension == "gif" {
                        RecordingKind::Gif
                    } else {
                        RecordingKind::Video
                    },
                    path: exported_path,
                    saved_path: permanent_path,
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
                    &mut artifact,
                    generated_poster.unwrap_or(poster_png),
                );
                let reveal_path = artifact
                    .saved_path
                    .clone()
                    .unwrap_or_else(|| artifact.path.clone());
                let reveal_error = app
                    .opener()
                    .reveal_item_in_dir(PathBuf::from(reveal_path))
                    .err()
                    .map(|error| error.to_string());
                let _ = app.emit(
                    "recording-export-complete",
                    RecordingExportComplete {
                        export_id: task_export_id,
                        artifact,
                        reveal_error,
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
    ensure_recording_artifact_loaded(state.inner(), &artifact_id)?;
    let artifact = state
        .recording_artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.summary.id == artifact_id)
        .map(|artifact| artifact.summary.clone())
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    let path = artifact
        .saved_path
        .filter(|path| Path::new(path).is_file())
        .ok_or_else(|| "Save this recording before showing it in its folder".to_owned())?;
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
    ensure_recording_artifact_loaded(state.inner(), &artifact_id)?;
    let available = state.recording_artifacts.lock().iter().any(|artifact| {
        artifact.summary.id == artifact_id && Path::new(&artifact.summary.path).is_file()
    });
    if !available {
        return Err("the recording file is missing".to_owned());
    }
    show_recording_editor(&app, &artifact_id).map_err(|error| error.to_string())
}

fn ensure_recording_artifact_loaded(state: &AppState, artifact_id: &str) -> Result<(), String> {
    if state
        .recording_artifacts
        .lock()
        .iter()
        .any(|artifact| artifact.summary.id == artifact_id)
    {
        return Ok(());
    }
    let entry = state
        .history
        .lock()
        .iter()
        .find(|entry| entry.id == artifact_id)
        .cloned()
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    let data = storage::load_recording_artifact(&entry)
        .ok_or_else(|| "the recording file is missing".to_owned())?;
    state.recording_artifacts.lock().push(data);
    Ok(())
}

#[tauri::command]
pub fn trash_recording_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<(), String> {
    let artifact = state
        .recording_artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.summary.id == artifact_id)
        .map(|artifact| artifact.summary.clone())
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    // Match screenshots: only trash an explicit Captures-folder save. Private
    // history recovery media stays available for HISTORY_RETENTION_DAYS.
    if let Some(saved_path) = artifact.saved_path.as_ref() {
        if Path::new(saved_path).exists() {
            trash::delete(saved_path).map_err(|error| error.to_string())?;
        }
        if let Some(entry) = state
            .history
            .lock()
            .iter_mut()
            .find(|entry| entry.id == artifact_id)
        {
            entry.saved_path = None;
            let _ = storage::update_history_entry_metadata(entry);
        }
        if let Some(live) = state
            .recording_artifacts
            .lock()
            .iter_mut()
            .find(|live| live.summary.id == artifact_id)
        {
            live.summary.saved_path = None;
        }
    }
    state
        .recording_artifacts
        .lock()
        .retain(|artifact| artifact.summary.id != artifact_id);
    state.recording_timeline_sprites.lock().remove(&artifact_id);
    let _ = app.emit("recording-artifact-removed", artifact_id);
    let _ = app.emit("capture-history-changed", ());
    Ok(())
}

/// Copy a history-only recording into the user's Captures folder permanently.
#[tauri::command]
pub async fn save_recording_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Result<RecordingArtifact, String> {
    let state = state.inner().clone();
    ensure_recording_artifact_loaded(&state, &artifact_id)?;
    let artifact = state
        .recording_artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.summary.id == artifact_id)
        .map(|artifact| artifact.summary.clone())
        .ok_or_else(|| "recording is no longer available".to_owned())?;
    if let Some(saved_path) = artifact.saved_path.as_ref()
        && Path::new(saved_path).is_file()
    {
        return Ok(artifact);
    }
    if !Path::new(&artifact.path).is_file() {
        return Err("the recording file is missing".to_owned());
    }
    let extension = Path::new(&artifact.path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or(if artifact.kind == RecordingKind::Gif {
            "gif"
        } else {
            "mp4"
        })
        .to_owned();
    let source = PathBuf::from(artifact.path.clone());
    let settings = state.settings();
    let destination = tauri::async_runtime::spawn_blocking(move || {
        storage::unique_media_path(Path::new(&settings.output_directory), &extension)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking({
        let source = source.clone();
        let destination = destination.clone();
        move || fs::copy(source, destination)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;

    let saved_path = destination.to_string_lossy().into_owned();
    let updated = {
        let mut recording_artifacts = state.recording_artifacts.lock();
        let live = recording_artifacts
            .iter_mut()
            .find(|live| live.summary.id == artifact_id)
            .ok_or_else(|| "recording is no longer available".to_owned())?;
        live.summary.saved_path = Some(saved_path.clone());
        live.summary.missing = false;
        live.summary.clone()
    };
    if let Some(entry) = state
        .history
        .lock()
        .iter_mut()
        .find(|entry| entry.id == artifact_id)
    {
        entry.saved_path = Some(saved_path);
        if let Err(error) = storage::update_history_entry_metadata(entry) {
            eprintln!("failed to update recording history after save: {error}");
        }
    }
    let _ = app.emit("capture-history-changed", ());
    let _ = app.emit(RECORDING_ARTIFACT_EVENT, &updated);
    Ok(updated)
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
    // Recover into the draft workspace, then promote into private history recovery.
    let destination = directory.join(format!("assembled.{extension}"));
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
            let system_audio_path = segment
                .system_audio_relative_path
                .as_deref()
                .map(|relative| recovery_child_path(&task_directory, relative))
                .transpose()?
                .filter(|path| path.is_file());
            segments.push(RecordingSegmentInput {
                video_path,
                system_audio_path,
                system_audio_offset_ms: segment.system_audio_offset_ms,
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
    let mut artifact = RecordingArtifact {
        id: artifact_id.clone(),
        kind: manifest.options.kind,
        path: destination.to_string_lossy().into_owned(),
        saved_path: None,
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
    upsert_recording_artifact(&app, &state, &mut artifact, poster_png);

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
            } else if let Some(range) =
                ByteRange::prefix_when_unbounded(total_length, DEFAULT_UNRANGED_MEDIA_BYTES)
            {
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

fn capture_mode_for_target(target: &RecordingTarget) -> CaptureMode {
    match target {
        RecordingTarget::Display { .. } => CaptureMode::Display,
        RecordingTarget::Region { .. } => CaptureMode::Region,
        RecordingTarget::Window { .. } => CaptureMode::Window,
    }
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
                && {
                    // Region rects are in overlay/CSS space, not necessarily
                    // the same units as display.width/height on Windows.
                    let (overlay_w, overlay_h) = selection.display.overlay_size();
                    let max_w = overlay_w.round().max(1.0) as u32;
                    let max_h = overlay_h.round().max(1.0) as u32;
                    u32::try_from(rect.x)
                        .ok()
                        .and_then(|x| x.checked_add(rect.width))
                        .is_some_and(|right| right <= max_w)
                        && u32::try_from(rect.y)
                            .ok()
                            .and_then(|y| y.checked_add(rect.height))
                            .is_some_and(|bottom| bottom <= max_h)
                } =>
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

fn image_for_selection(
    selection: &RecordingSelection,
    target: &RecordingTarget,
) -> Result<image::RgbaImage, AppError> {
    let image = selection
        .image
        .as_ref()
        .ok_or(AppError::SessionUnavailable)?;
    let image = match target {
        RecordingTarget::Display { .. } => image.clone(),
        RecordingTarget::Region { rect, .. } => {
            crop_display_region(image, &selection.summary.display, rect)?
        }
        RecordingTarget::Window { window_id } => {
            let window = selection
                .summary
                .windows
                .iter()
                .find(|window| &window.id == window_id)
                .ok_or(AppError::InvalidSelection)?;
            crop_window_from_display(image, &selection.summary.display, window)?
        }
    };
    Ok(image)
}

fn pixels_for_selection(
    state: &AppState,
    selection: &RecordingSelection,
    target: &RecordingTarget,
) -> Result<image::RgbaImage, AppError> {
    if selection.summary.frozen && selection.image.is_some() {
        image_for_selection(selection, target)
    } else {
        live_image_for_target(state, target)
    }
}

fn poster_for_selection(
    state: &AppState,
    selection: &RecordingSelection,
    target: &RecordingTarget,
) -> Result<Vec<u8>, AppError> {
    let image = pixels_for_selection(state, selection, target)?;
    storage::encode_thumbnail_png(&image)
}

fn take_active_segment(
    state: &AppState,
    session_id: &str,
    expected_state: RecordingState,
) -> Result<(NativeRecordingSegment, u64), AppError> {
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

async fn stop_native_segment(
    segment: NativeRecordingSegment,
) -> Result<RecordingSegmentInfo, AppError> {
    tauri::async_runtime::spawn_blocking(move || segment.stop())
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
        .map_err(|error| AppError::Task(error.to_string()))
}

fn append_segment(
    session: &mut RuntimeSession,
    info: RecordingSegmentInfo,
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
    let system_audio_relative_path = info
        .system_audio_path
        .as_ref()
        .map(|path| {
            path.strip_prefix(&session.directory)
                .map(|relative| relative.to_string_lossy().into_owned())
                .map_err(|_| {
                    AppError::Task("desktop audio segment escaped its recovery bundle".to_owned())
                })
        })
        .transpose()?;
    let segment = RecordingSegmentManifest {
        index: u32::try_from(session.manifest.segments.len())
            .map_err(|_| AppError::Task("recording has too many segments".to_owned()))?,
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
    let (snapshot, segment) = {
        let mut runtime = state.recording.lock();
        let now = now_ms();
        let snapshot = runtime
            .coordinator
            .fail(session_id, message.clone(), now)
            .ok();
        let segment = runtime.session.as_mut().and_then(|session| {
            if session.id != session_id {
                return None;
            }
            session.manifest.state = RecordingState::Failed;
            session.manifest.last_error = Some(message.clone());
            session.manifest.updated_at_ms = now;
            let _ = save_manifest(&session.manifest);
            session.active_segment.take()
        });
        (snapshot, segment)
    };
    if let Some(segment) = segment {
        tauri::async_runtime::spawn_blocking(move || {
            let _ = segment.discard();
        });
    }
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
    destroy_recording_region_indicator(app);
    destroy_recording_countdown(app);
    crate::hide_window(app, "recording-hud");
}

fn restore_recording_ui(app: &AppHandle, state: &Arc<AppState>) {
    destroy_recording_selector(app);
    destroy_recording_region_indicator(app);
    destroy_recording_countdown(app);
    crate::hide_window(app, "recording-hud");
    #[cfg(target_os = "macos")]
    captures_macos_window::restore_frontmost_app_after_capture();
    crate::reveal_document_windows_after_capture(app);
    crate::set_capture_huds_protected(app, false);
    crate::restore_thumbnail_capture_ui(app, state);
    crate::updates::restore_update_notice(app);
}

#[cfg(target_os = "macos")]
pub(crate) fn focus_recording_window(app: &AppHandle, label: &'static str) {
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
    if let Err(error) = crate::set_click_through(&window, true) {
        eprintln!("failed to disable recording selector pointer events: {error}");
    }
    // Hidden WKWebViews can suspend before processing the next selection.
    // Recreate this short-lived surface so every selection gets a live event
    // bridge and cannot inherit stale region or window state.
    if let Err(error) = window.destroy() {
        eprintln!("failed to destroy recording selector: {error}");
    }
    #[cfg(target_os = "macos")]
    captures_macos_window::release_capture_cursor();
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

fn replace_recording_source_at(
    source: &Path,
    replacement: &Path,
    destination: &Path,
) -> io::Result<()> {
    if source == destination {
        return replace_recording_source(source, replacement);
    }
    // Overwriting an existing permanent Captures save (destination != recovery source).
    if destination.exists() {
        replace_recording_source(destination, replacement)?;
        // Recovery media is rebuilt from the permanent save on the next history upsert.
        // Drop the old recovery file when it is a different path so we do not leave a
        // stale unedited copy beside the new permanent file.
        if source.exists() {
            let _ = fs::remove_file(source);
        }
        return Ok(());
    }

    fs::rename(replacement, destination)?;
    match fs::remove_file(source) {
        Ok(()) => Ok(()),
        Err(remove_error) => match fs::remove_file(destination) {
            Ok(()) => Err(remove_error),
            Err(rollback_error) => Err(io::Error::new(
                remove_error.kind(),
                format!(
                    "the original recording could not be removed ({remove_error}), and the new destination could not be rolled back automatically ({rollback_error})"
                ),
            )),
        },
    }
}

fn upsert_recording_artifact(
    app: &AppHandle,
    state: &AppState,
    artifact: &mut RecordingArtifact,
    poster_png: Vec<u8>,
) {
    let media_source = PathBuf::from(&artifact.path);
    let history_entry = HistoryEntry::from_recording(artifact);
    let history_saved =
        match storage::save_history_recording(&history_entry, &poster_png, &media_source) {
            Ok(recovery_path) => {
                artifact.path = recovery_path.to_string_lossy().into_owned();
                artifact.missing = false;
                true
            }
            Err(error) => {
                eprintln!("failed to save recording history: {error}");
                false
            }
        };
    let artifact_id = artifact.id.clone();
    let artifact_data = RecordingArtifactData {
        summary: artifact.clone(),
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
    let target_suffix = media_target_suffix(std::env::consts::OS, std::env::consts::ARCH);
    let find = |name: &str| {
        let executable_name = if cfg!(target_os = "windows") {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let suffixed_name = target_suffix.map(|target_suffix| {
            if cfg!(target_os = "windows") {
                format!("{name}-{target_suffix}.exe")
            } else {
                format!("{name}-{target_suffix}")
            }
        });
        executable_directory
            .as_ref()
            .map(|directory| directory.join(&executable_name))
            .filter(|path| path.is_file())
            .or_else(|| {
                executable_directory
                    .as_ref()
                    .and_then(|directory| {
                        suffixed_name
                            .as_ref()
                            .map(|suffixed_name| directory.join(suffixed_name))
                    })
                    .filter(|path| path.is_file())
            })
            .or_else(|| {
                resource_directory
                    .as_ref()
                    .map(|directory| directory.join("binaries").join(&executable_name))
                    .filter(|path| path.is_file())
            })
            .or_else(|| {
                resource_directory
                    .as_ref()
                    .and_then(|directory| {
                        suffixed_name
                            .as_ref()
                            .map(|suffixed_name| directory.join("binaries").join(suffixed_name))
                    })
                    .filter(|path| path.is_file())
            })
            .or_else(|| {
                suffixed_name.as_ref().and_then(|suffixed_name| {
                    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("binaries")
                        .join(suffixed_name);
                    path.is_file().then_some(path)
                })
            })
            .unwrap_or_else(|| PathBuf::from(name))
    };
    MediaToolchain::new(find("ffmpeg"), find("ffprobe"))
}

fn media_target_suffix(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        _ => None,
    }
}

fn create_recording_selector_window(app: &AppHandle) -> Result<(), AppError> {
    if app.get_webview_window("recording-selector").is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        app,
        "recording-selector",
        WebviewUrl::App("index.html?view=recording-selector".into()),
    )
    .title("Captures")
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
    // Keep the new 1x1 window offscreen while it is resized and moved over the
    // selected display. Showing it here lets WebKit composite that geometry
    // change at full alpha before prepare_recording_selector primes the native
    // reveal, which reads as a brief fullscreen zoom/flash.
    .visible(false)
    .build()?;
    // Selector sits over the desktop; keep it out of secondary captures.
    window.set_content_protected(cfg!(target_os = "windows"))?;
    Ok(())
}

async fn prepare_recording_selector(
    app: &AppHandle,
    selection: &RecordingSelectionSession,
    wake_webview: bool,
) -> Result<(), AppError> {
    let handle = app.clone();
    let selection = selection.clone();
    let wake_selection = selection.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            create_recording_selector_window(&handle).map_err(|error| error.to_string())?;
            let display = &selection.display;
            let window = handle
                .get_webview_window("recording-selector")
                .ok_or_else(|| "recording selector is unavailable".to_owned())?;
            #[cfg(target_os = "macos")]
            captures_macos_window::configure_capture_selector(&window).map_err(str::to_owned)?;
            // Match the screenshot overlay: on Windows xcap geometry is physical,
            // while Tauri LogicalSize/Position expect CSS DIPs.
            let (x, y, width, height) = display.overlay_geometry();
            window
                .set_size(LogicalSize::new(width, height))
                .map_err(|error| error.to_string())?;
            // A hidden borderless NSWindow grows from its bottom-left anchor.
            // Position it after resizing so the final top-left edge matches the
            // selected display instead of landing one full screen above it.
            window
                .set_position(tauri::LogicalPosition::new(x, y))
                .map_err(|error| error.to_string())?;
            #[cfg(target_os = "macos")]
            captures_macos_window::cover_display(&window, &display.id).map_err(str::to_owned)?;
            window
                .set_content_protected(cfg!(target_os = "windows"))
                .map_err(|error| error.to_string())?;
            // A hidden or zero-alpha WKWebView can be suspended before React
            // installs its recording-selection listener. Wake it at a tiny,
            // imperceptible alpha while pointer events still pass through.
            // React reveals the window only after the new snapshot has painted,
            // so a cached region or window highlight can never flash onscreen.
            crate::set_click_through(&window, true).map_err(|error| error.to_string())?;
            #[cfg(target_os = "macos")]
            captures_macos_window::prime_window_reveal(&window).map_err(str::to_owned)?;
            window.show().map_err(|error| error.to_string())?;
            crate::set_click_through(&window, true).map_err(|error| error.to_string())?;
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
    if wake_webview {
        schedule_recording_selector_webview_wake(app, wake_selection);
    }
    Ok(())
}

fn schedule_recording_selector_webview_wake(app: &AppHandle, selection: RecordingSelectionSession) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A hidden or near-transparent WKWebView can suspend JavaScript,
        // including the frontend's animation-frame and timer fallbacks. Wake
        // the native window independently while it is still click-through.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let selection_id = selection.id.clone();
        let snapshot_url = selection.snapshot_url.clone();
        let still_pending = app
            .state::<Arc<AppState>>()
            .recording_selection
            .lock()
            .as_ref()
            .is_some_and(|selection| {
                selection.summary.id == selection_id
                    && selection.summary.snapshot_url == snapshot_url
            });
        if !still_pending {
            return;
        }
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            let Some(window) = handle.get_webview_window("recording-selector") else {
                return;
            };
            #[cfg(target_os = "macos")]
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
            .is_some_and(|pending| {
                pending.summary.id == selection.id
                    && pending.summary.snapshot_url == selection.snapshot_url
            });
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
    crate::set_click_through(&window, true).map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    crate::set_click_through(&window, true).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn selector_cursor_icon(mode: CaptureMode) -> CursorIcon {
    if mode == CaptureMode::Region {
        CursorIcon::Crosshair
    } else {
        CursorIcon::Default
    }
}

fn apply_selector_capture_cursor(
    window: &tauri::WebviewWindow,
    mode: CaptureMode,
) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    window
        .set_cursor_icon(selector_cursor_icon(mode))
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    captures_macos_window::activate_capture_cursor(
        window,
        captures_macos_window::CaptureCursor::selector(
            mode == CaptureMode::Region,
            mode == CaptureMode::Window,
        ),
    )
    .map_err(str::to_owned)?;
    Ok(())
}

#[tauri::command]
pub fn reveal_recording_selector(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    selection_id: String,
) -> Result<(), String> {
    let mode = state
        .recording_selection
        .lock()
        .as_ref()
        .filter(|selection| selection.summary.id == selection_id)
        .map(|selection| selection.summary.initial_target)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let window = app
        .get_webview_window("recording-selector")
        .ok_or_else(|| "recording selector is unavailable".to_owned())?;
    #[cfg(target_os = "macos")]
    {
        captures_macos_window::reveal_window(&window).map_err(str::to_owned)?;
        captures_macos_window::conceal_documents_under_opaque_capture_surface();
        captures_macos_window::elevate_capture_surface(&window).map_err(str::to_owned)?;
    }
    crate::set_click_through(&window, false).map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    crate::set_click_through(&window, false).map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    {
        // Wait for activation on this thread so the cursor is applied after
        // Captures is frontmost. NSCursor.set() is ignored while inactive.
        if let Err(error) = captures_macos_window::focus_window(&window) {
            eprintln!("failed to activate recording selector: {error}");
        }
    }
    // Focus is helpful for Escape-key handling, but macOS can temporarily
    // reject it for an accessory app. The selector is already visible and
    // interactive, so do not turn that harmless failure into a hidden window.
    if let Err(error) = window.set_focus() {
        eprintln!("failed to focus recording selector: {error}");
    }
    apply_selector_capture_cursor(&window, mode)?;
    Ok(())
}

#[tauri::command]
pub fn sync_selector_cursor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    selection_id: String,
    mode: CaptureMode,
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
    apply_selector_capture_cursor(&window, mode)
}

fn recording_hud_position(display: &DisplayDescriptor) -> (f64, f64) {
    let (x, y, width, height) = display.overlay_geometry();
    recording_hud_logical_position(x, y, width, height)
}

fn recording_region_indicator_url(target: &RecordingTarget) -> Option<String> {
    let RecordingTarget::Region { rect, .. } = target else {
        return None;
    };
    rect.is_valid().then(|| {
        format!(
            "index.html?view=recording-region-indicator&x={}&y={}&width={}&height={}",
            rect.x, rect.y, rect.width, rect.height
        )
    })
}

async fn prepare_recording_region_indicator(
    app: &AppHandle,
    display: &DisplayDescriptor,
    target: &RecordingTarget,
) -> Result<(), AppError> {
    let Some(url) = recording_region_indicator_url(target) else {
        destroy_recording_region_indicator(app);
        return Ok(());
    };
    let handle = app.clone();
    let display = display.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            destroy_recording_region_indicator(&handle);
            let (x, y, width, height) = display.overlay_geometry();
            let window = WebviewWindowBuilder::new(
                &handle,
                RECORDING_REGION_INDICATOR_LABEL,
                WebviewUrl::App(url.into()),
            )
            .title(RECORDING_REGION_INDICATOR_TITLE)
            .inner_size(width, height)
            .position(x, y)
            .decorations(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .shadow(false)
            .resizable(false)
            .transparent(true)
            .background_color(Color(0, 0, 0, 0))
            .focused(false)
            .visible(false)
            .build()
            .map_err(|error| error.to_string())?;
            // Keep the overlay passive and permanently excluded where the
            // platform supports protected windows. Its shade and frame are
            // also painted wholly outside the recorded rectangle.
            window
                .set_content_protected(true)
                .map_err(|error| error.to_string())?;
            crate::set_click_through(&window, true).map_err(|error| error.to_string())?;
            #[cfg(target_os = "macos")]
            {
                captures_macos_window::configure_capture_selector(&window)
                    .map_err(str::to_owned)?;
                captures_macos_window::cover_display(&window, &display.id)
                    .map_err(str::to_owned)?;
                // Status level keeps this above apps but below the later
                // countdown and the recording HUD.
                captures_macos_window::show_without_activating(&window).map_err(str::to_owned)?;
            }
            #[cfg(not(target_os = "macos"))]
            window.show().map_err(|error| error.to_string())?;
            crate::set_click_through(&window, true).map_err(|error| error.to_string())?;
            Ok(())
        })();
        let _ = sender.send(result);
    })?;
    receiver
        .await
        .map_err(|_| AppError::Task("recording region setup was interrupted".to_owned()))?
        .map_err(AppError::Task)
}

fn destroy_recording_region_indicator(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(RECORDING_REGION_INDICATOR_LABEL)
        && let Err(error) = window.destroy()
    {
        eprintln!("failed to close recording region indicator: {error}");
    }
}

fn recording_hud_logical_position(x: f64, y: f64, width: f64, height: f64) -> (f64, f64) {
    (
        x + (width - RECORDING_HUD_FULL_WIDTH) / 2.0,
        y + height - RECORDING_HUD_HEIGHT - RECORDING_HUD_BOTTOM_MARGIN,
    )
}

async fn prepare_recording_hud(
    app: &AppHandle,
    display: &DisplayDescriptor,
) -> Result<(), AppError> {
    let (x, y) = recording_hud_position(display);
    let created = app.get_webview_window("recording-hud").is_none();
    if created {
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
        .accept_first_mouse(true)
        .focused(false)
        .visible(false)
        .build()?;
    }

    let handle = app.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // Async Tauri commands run on a Tokio worker. Converting the HUD to an
    // NSPanel touches AppKit and must complete on the main thread before the
    // countdown can reveal any recording controls.
    app.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            let window = handle
                .get_webview_window("recording-hud")
                .ok_or_else(|| "recording controls are unavailable".to_owned())?;
            #[cfg(target_os = "macos")]
            if created {
                captures_macos_window::configure_webview_inactive_hover(&window)
                    .map_err(str::to_owned)?;
            }
            window
                .set_size(tauri::LogicalSize::new(
                    RECORDING_HUD_FULL_WIDTH,
                    RECORDING_HUD_HEIGHT,
                ))
                .map_err(|error| error.to_string())?;
            window
                .set_position(tauri::LogicalPosition::new(x, y))
                .map_err(|error| error.to_string())?;
            window
                .set_content_protected(recording_overlay_content_protected(&handle))
                .map_err(|error| error.to_string())?;
            window.hide().map_err(|error| error.to_string())?;
            Ok(())
        })();
        let _ = sender.send(result);
    })?;
    receiver
        .await
        .map_err(|_| AppError::Task("recording controls setup was interrupted".to_owned()))?
        .map_err(AppError::Task)
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
    let notice_position = window.outer_position().ok().and_then(|position| {
        let size = window.outer_size().ok()?;
        let scale = window.scale_factor().ok()?.max(1.0);
        Some((
            f64::from(position.x) / scale
                + (f64::from(size.width) / scale - crate::RECORDING_CONTROLS_HIDDEN_NOTICE_WIDTH)
                    / 2.0,
            f64::from(position.y) / scale
                + (f64::from(size.height) / scale - crate::RECORDING_CONTROLS_HIDDEN_NOTICE_HEIGHT)
                    / 2.0,
        ))
    });
    window.hide().map_err(|error| error.to_string())?;
    if let Err(error) = crate::show_recording_controls_hidden_notice(&app, notice_position) {
        eprintln!("failed to show recording controls hidden notice: {error}");
    }
    Ok(())
}

async fn show_recording_hud(app: &AppHandle) -> Result<(), AppError> {
    let handle = app.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // The countdown completes on a Tokio worker. Revealing the native panel
    // orders an NSWindow and therefore must run on AppKit's main thread.
    app.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            crate::hide_recording_controls_hidden_notices(&handle);
            let window = handle
                .get_webview_window("recording-hud")
                .ok_or_else(|| "recording controls are unavailable".to_owned())?;
            window
                .set_content_protected(recording_overlay_content_protected(&handle))
                .map_err(|error| error.to_string())?;
            #[cfg(target_os = "macos")]
            captures_macos_window::show_without_activating(&window).map_err(str::to_owned)?;
            #[cfg(not(target_os = "macos"))]
            window.show().map_err(|error| error.to_string())?;
            Ok(())
        })();
        let _ = sender.send(result);
    })?;
    receiver
        .await
        .map_err(|_| AppError::Task("recording controls reveal was interrupted".to_owned()))?
        .map_err(AppError::Task)
}

fn show_recording_countdown(app: &AppHandle, display: &DisplayDescriptor) -> Result<(), AppError> {
    let (x, y, width, height) = display.overlay_geometry();
    if app.get_webview_window("recording-countdown").is_none() {
        WebviewWindowBuilder::new(
            app,
            "recording-countdown",
            WebviewUrl::App("index.html?view=recording-countdown".into()),
        )
        .title("Captures Recording Countdown")
        .inner_size(width, height)
        .position(x, y)
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
    window.set_size(tauri::LogicalSize::new(width, height))?;
    window.set_position(tauri::LogicalPosition::new(x, y))?;
    window.set_content_protected(recording_overlay_content_protected(app))?;
    window.show()?;
    #[cfg(target_os = "macos")]
    captures_macos_window::elevate_capture_surface(&window)
        .map_err(|error| AppError::Task(error.to_owned()))?;
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

fn recording_overlay_content_protected(app: &AppHandle) -> bool {
    // Windows content protection is the exclusion mechanism for recording chrome.
    // When the user opts into including recording controls, leave them capturable.
    if !cfg!(target_os = "windows") {
        return false;
    }
    app.try_state::<Arc<AppState>>().is_none_or(|state| {
        controls_excluded_for_preference(state.settings().include_recording_controls_in_captures)
    })
}

fn show_recording_editor(app: &AppHandle, artifact_id: &str) -> Result<(), AppError> {
    let label = format!("recording-editor-{artifact_id}");
    // Opening the editor is intentional; keep Captures focused instead of
    // restoring the app that was frontmost when recording started.
    #[cfg(target_os = "macos")]
    captures_macos_window::clear_frontmost_app_anchor();
    if let Some(window) = app.get_webview_window(&label) {
        crate::reveal_and_focus_document_window(&window)?;
        return Ok(());
    }
    let (theme, background) = crate::document_window_chrome(app);
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
    .theme(theme)
    .background_color(background)
    .focused(false)
    .visible(false)
    .on_page_load(crate::document_window_page_load_handler(
        "failed to reveal recording editor",
    ))
    .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use captures_recording::{CaptureRect, RecordingState, RecordingTarget};
    use tempfile::tempdir;

    use super::{
        RECORDING_HUD_BOTTOM_MARGIN, RECORDING_HUD_FULL_WIDTH, RECORDING_HUD_HEIGHT,
        media_target_suffix, recording_hud_logical_position, recording_in_progress_for,
        recording_region_indicator_url, replace_recording_source, replace_recording_source_at,
        replacement_working_path, screenshot_capture_is_blocked_for,
    };

    #[test]
    fn builds_a_region_indicator_only_for_valid_region_targets() {
        let region = RecordingTarget::Region {
            display_id: "display-1".to_owned(),
            rect: CaptureRect {
                x: 120,
                y: 80,
                width: 640,
                height: 360,
            },
        };
        assert_eq!(
            recording_region_indicator_url(&region).as_deref(),
            Some("index.html?view=recording-region-indicator&x=120&y=80&width=640&height=360")
        );
        assert!(
            recording_region_indicator_url(&RecordingTarget::Display {
                display_id: "display-1".to_owned(),
            })
            .is_none()
        );
        assert!(
            recording_region_indicator_url(&RecordingTarget::Region {
                display_id: "display-1".to_owned(),
                rect: CaptureRect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 360,
                },
            })
            .is_none()
        );
    }

    #[test]
    fn recording_hud_uses_overlay_dips_not_physical_pixels() {
        let (x, y) = recording_hud_logical_position(0.0, 0.0, 1_920.0, 1_080.0);
        assert!((x - (1_920.0 - RECORDING_HUD_FULL_WIDTH) / 2.0).abs() < f64::EPSILON);
        assert!(
            (y - (1_080.0 - RECORDING_HUD_HEIGHT - RECORDING_HUD_BOTTOM_MARGIN)).abs()
                < f64::EPSILON
        );

        let (width, height) =
            captures_capture::DisplayDescriptor::overlay_size_for(3_840, 2_160, 2.0, true);
        let (scaled_x, scaled_y) = recording_hud_logical_position(0.0, 0.0, width, height);
        assert!((scaled_x - x).abs() < f64::EPSILON);
        assert!((scaled_y - y).abs() < f64::EPSILON);
        let physical_x = (3_840.0 - RECORDING_HUD_FULL_WIDTH) / 2.0;
        assert!((scaled_x - physical_x).abs() > 100.0);
    }

    #[test]
    fn resolves_media_sidecars_only_for_packaged_platform_architectures() {
        assert_eq!(
            media_target_suffix("macos", "aarch64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            media_target_suffix("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(
            media_target_suffix("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(media_target_suffix("linux", "aarch64"), None);
        assert_eq!(media_target_suffix("windows", "aarch64"), None);
    }

    #[test]
    fn permits_screenshot_shortcuts_while_capture_controls_are_open() {
        // A pending capture selection intentionally does not participate in this
        // gate so a direct screenshot shortcut can capture the visible controls.
        assert!(!screenshot_capture_is_blocked_for(None));
    }

    #[test]
    fn permits_screenshots_while_recording_or_paused() {
        assert!(!screenshot_capture_is_blocked_for(Some(
            RecordingState::Recording
        )));
        assert!(!screenshot_capture_is_blocked_for(Some(
            RecordingState::Paused
        )));
        assert!(!screenshot_capture_is_blocked_for(Some(
            RecordingState::Failed
        )));
    }

    #[test]
    fn blocks_screenshots_during_recording_setup_and_finalization() {
        assert!(screenshot_capture_is_blocked_for(Some(
            RecordingState::Selecting
        )));
        assert!(screenshot_capture_is_blocked_for(Some(
            RecordingState::Countdown
        )));
        assert!(screenshot_capture_is_blocked_for(Some(
            RecordingState::Finalizing
        )));
    }

    #[test]
    fn treats_live_recordings_as_in_progress_for_updates() {
        assert!(recording_in_progress_for(Some(RecordingState::Countdown)));
        assert!(recording_in_progress_for(Some(RecordingState::Recording)));
        assert!(recording_in_progress_for(Some(RecordingState::Paused)));
        assert!(recording_in_progress_for(Some(RecordingState::Finalizing)));
        assert!(!recording_in_progress_for(Some(RecordingState::Selecting)));
        assert!(!recording_in_progress_for(Some(RecordingState::Editor)));
        assert!(!recording_in_progress_for(Some(RecordingState::Ready)));
        assert!(!recording_in_progress_for(None));
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

    #[test]
    fn replacing_a_recording_can_rename_or_move_the_original() {
        let source_directory = tempdir().expect("source directory");
        let destination_directory = tempdir().expect("destination directory");
        let source = source_directory.path().join("recording.mp4");
        let replacement = destination_directory.path().join(".replacement.mp4");
        let destination = destination_directory.path().join("renamed.mp4");
        std::fs::write(&source, b"original").expect("source");
        std::fs::write(&replacement, b"edited").expect("replacement");

        replace_recording_source_at(&source, &replacement, &destination).expect("source moved");

        assert!(!source.exists());
        assert!(!replacement.exists());
        assert_eq!(
            std::fs::read(&destination).expect("renamed recording"),
            b"edited"
        );
    }

    #[test]
    fn replacing_a_recording_can_overwrite_an_existing_permanent_destination() {
        let source_directory = tempdir().expect("source directory");
        let destination_directory = tempdir().expect("destination directory");
        let source = source_directory.path().join("media.mp4");
        let replacement = destination_directory.path().join(".replacement.mp4");
        let destination = destination_directory.path().join("Captures_clip.mp4");
        std::fs::write(&source, b"recovery").expect("source");
        std::fs::write(&replacement, b"edited").expect("replacement");
        std::fs::write(&destination, b"previous permanent").expect("destination");

        replace_recording_source_at(&source, &replacement, &destination)
            .expect("permanent destination replaced");

        assert!(!source.exists());
        assert!(!replacement.exists());
        assert_eq!(
            std::fs::read(&destination).expect("permanent recording"),
            b"edited"
        );
    }
}
