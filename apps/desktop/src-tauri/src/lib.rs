#![forbid(unsafe_code)]

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(not(target_os = "macos"))]
use std::sync::atomic::AtomicIsize;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

use tauri::CursorIcon;

use captures_capture::{CaptureError, CaptureMode, LogicalRect};
use chrono::{DateTime, Utc};
use image::RgbaImage;
use mouse_position::mouse_position::Mouse;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    webview::PageLoadEvent,
    window::Color,
};
use tauri_plugin_autostart::ManagerExt as AutoStartExt;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use thiserror::Error;
use uuid::Uuid;

mod models;
mod recording;
mod screenshot_editor;
mod state;
mod storage;
mod updates;

use models::{
    ActiveSession, AppSettings, ArtifactKind, ArtifactSummary, CaptureArtifact,
    CaptureSelectorMode, CaptureSession, ClipboardCopyStatus, ClipboardState,
    HISTORY_RETENTION_DAYS, HistoryEntry,
};
use screenshot_editor::SCREENSHOT_EDITOR_WINDOW_PREFIX;
use state::{AppState, ClipboardFingerprint};

#[derive(Debug, Error)]
enum AppError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("image operation failed: {0}")]
    Image(String),
    #[error("clipboard operation failed: {0}")]
    Clipboard(String),
    #[error("Tauri operation failed: {0}")]
    Tauri(#[from] tauri::Error),
    #[error("capture already in progress")]
    CaptureInProgress,
    #[error("screenshot cancelled")]
    ScreenshotCancelled,
    #[error("capture session is no longer available")]
    SessionUnavailable,
    #[error("capture history entry is no longer available")]
    HistoryUnavailable,
    #[error("the selection must be larger than zero pixels")]
    InvalidSelection,
    #[error("shortcut registration failed: {0}")]
    Shortcut(String),
    #[error("{0}")]
    Task(String),
    #[error("an update is being installed; Captures will restart when it finishes")]
    UpdateInstalling,
}

type CommandResult<T> = Result<T, String>;
const AUTOSTART_ARG: &str = "--captures-autostart";
const RECORDING_EDITOR_WINDOW_PREFIX: &str = "recording-editor-";
const RECORDING_SAVED_NOTICE_LABEL: &str = "recording-saved";
const RECORDING_SAVED_NOTICE_EVENT: &str = "recording-saved-artifact";
const RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX: &str = "recording-controls-hidden-";
#[cfg(any(target_os = "macos", test))]
const WINDOW_CORNER_MASK_SAMPLES_PER_AXIS: u32 = 4;

struct ClipboardWrite {
    revision: isize,
    fingerprint: ClipboardFingerprint,
}

pub fn run() {
    let state = AppState::new();
    let protocol_state = state.clone();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            focus_or_show_primary_app_window(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Captures")
                .arg(AUTOSTART_ARG)
                .build(),
        );

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(captures_macos_window::init_panel_plugin());

    builder
        .on_window_event(|window, event| {
            if !matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                return;
            }
            let Some(artifact_id) = window.label().strip_prefix(RECORDING_EDITOR_WINDOW_PREFIX)
            else {
                return;
            };
            if let Err(error) = show_recording_saved_notice(window.app_handle(), artifact_id) {
                eprintln!("failed to show recording saved notice: {error}");
            }
        })
        .manage(state)
        .manage(updates::UpdateCoordinator::default())
        .register_uri_scheme_protocol("captures-capture", move |_context, request| {
            let path = request.uri().path().trim_matches('/');
            let mut segments = path.split('/');
            let category = segments.next().unwrap_or_default();
            let id = segments.next().unwrap_or_default();
            let range = request
                .headers()
                .get("range")
                .and_then(|value| value.to_str().ok());
            if let Some(asset) =
                recording::resolve_recording_asset(&protocol_state, category, id, range)
            {
                let mut response = tauri::http::Response::builder()
                    .status(asset.status)
                    .header("Content-Type", asset.mime_type)
                    .header("Content-Length", asset.bytes.len().to_string())
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Cache-Control", "no-store");
                if asset.total_length.is_some() {
                    response = response.header("Accept-Ranges", "bytes");
                }
                if let Some(content_range) = asset.content_range {
                    response = response.header("Content-Range", content_range);
                }
                return response.body(asset.bytes).expect("valid media response");
            }
            match resolve_asset(&protocol_state, path) {
                Some(bytes) => tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", "image/png")
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Cache-Control", "no-store")
                    .body(bytes)
                    .expect("valid image response"),
                None => tauri::http::Response::builder()
                    .status(404)
                    .header("Content-Type", "text/plain")
                    .body(Vec::new())
                    .expect("valid missing response"),
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_capture,
            commit_region,
            commit_window,
            cancel_capture,
            cancel_screenshot_countdown,
            get_active_session,
            get_pending_session,
            get_settings,
            set_shortcut_capture_suppressed,
            update_settings,
            get_artifacts,
            get_artifact,
            prepare_artifact_drag,
            mark_internal_file_drop,
            should_keep_preview_after_file_drop,
            read_prepared_drag_image,
            prepared_drag_artifact_id,
            get_capture_history,
            restore_history_artifact,
            delete_history_artifact,
            clear_capture_history,
            get_clipboard_state,
            copy_artifact,
            save_artifact,
            reveal_artifact,
            trash_artifact,
            dismiss_artifact,
            open_artifact_viewer,
            screenshot_editor::open_screenshot_editor,
            screenshot_editor::default_screenshot_edit_path,
            screenshot_editor::copy_screenshot_edit,
            screenshot_editor::save_screenshot_edit,
            show_capture_overlay,
            reveal_capture_overlay,
            sync_capture_cursor,
            thumbnail_ready,
            sync_thumbnail_stack,
            get_thumbnail_pointer_position,
            set_thumbnail_cursor,
            reassert_thumbnail_cursor,
            set_thumbnail_ignore_cursor_events,
            refresh_thumbnail_interactivity,
            open_captures_folder,
            open_capture_history,
            open_preferences,
            dismiss_recording_saved_notice,
            updates::get_update_status,
            updates::check_for_updates,
            updates::install_update,
            recording::prepare_recording,
            recording::get_recording_selection,
            recording::show_recording_selector,
            recording::reveal_recording_selector,
            recording::cancel_recording_selection,
            recording::capture_selection_screenshot,
            recording::list_recording_audio_devices,
            recording::recording_controls_are_excluded,
            recording::get_recording_snapshot,
            recording::start_recording,
            recording::pause_recording,
            recording::resume_recording,
            recording::restart_recording,
            recording::stop_recording,
            recording::discard_recording,
            recording::set_recording_microphone_muted,
            recording::hide_recording_hud,
            recording::get_recording_artifacts,
            recording::get_recording_artifact,
            recording::prepare_recording_timeline_preview,
            recording::start_recording_export,
            recording::cancel_recording_export,
            recording::reveal_recording_artifact,
            recording::open_recording_editor,
            recording::save_recording_artifact,
            recording::trash_recording_artifact,
            recording::get_recording_drafts,
            recording::recover_recording_draft,
            recording::discard_recording_draft,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }
            setup_tray(app)?;
            recording::prune_expired_gif_sources();
            let handle = app.handle().clone();
            updates::initialize(&handle);
            register_shortcuts(&handle)
                .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error.to_string())))?;
            if let Err(error) = create_thumbnail_window(&handle, false) {
                eprintln!("failed to prepare capture thumbnail: {error}");
            }
            if let Err(error) = create_overlay_window(&handle) {
                eprintln!("failed to prepare capture overlay: {error}");
            }
            let pending_capture = {
                let state = app.state::<Arc<AppState>>().inner().clone();
                match take_pending_capture_after_restart(&state) {
                    Ok(pending) => pending,
                    Err(error) => {
                        eprintln!("failed to restore capture after restart: {error}");
                        None
                    }
                }
            };
            refresh_autostart_registration(app);
            if pending_capture.is_none() {
                if launched_from_autostart() {
                    if let Err(error) = show_startup_notice(&handle) {
                        eprintln!("failed to show Captures launch notice: {error}");
                    }
                } else {
                    open_capture_controls(&handle, CaptureSelectorMode::Screenshot);
                }
            }
            if let Some(mode) = pending_capture {
                let state = app.state::<Arc<AppState>>().inner().clone();
                let app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = start_capture_inner(app.clone(), state, mode).await {
                        report_capture_error(&app, &error, mode);
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Captures")
        .run(|_app, event| match event {
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => api.prevent_exit(),
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => focus_or_show_primary_app_window(_app),
            _ => {}
        });
}

fn launched_from_autostart() -> bool {
    std::env::args_os().any(|argument| argument == std::ffi::OsStr::new(AUTOSTART_ARG))
}

fn refresh_autostart_registration(app: &tauri::App) {
    #[cfg(not(debug_assertions))]
    {
        let settings = app.state::<Arc<AppState>>().settings();
        if settings.launch_at_login
            && app.autolaunch().is_enabled().unwrap_or(false)
            && let Err(error) = app.autolaunch().enable()
        {
            eprintln!("failed to refresh launch-at-login registration: {error}");
        }
    }

    #[cfg(debug_assertions)]
    let _ = app;
}

fn open_capture_controls(app: &AppHandle, initial_mode: CaptureSelectorMode) {
    if restore_hidden_recording_controls(app) {
        return;
    }
    let state = app.state::<Arc<AppState>>().inner().clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) =
            recording::prepare_capture_selector_inner(app.clone(), state, initial_mode).await
        {
            match initial_mode {
                CaptureSelectorMode::Screenshot => {
                    report_capture_error(&app, &error, CaptureMode::Region);
                }
                CaptureSelectorMode::Recording => report_recording_error(&app, &error),
            }
        }
    });
}

#[tauri::command]
async fn start_capture(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    mode: CaptureMode,
) -> CommandResult<Option<ActiveSession>> {
    start_capture_inner(app, state.inner().clone(), mode)
        .await
        .map_err(|error| error.to_string())
}

async fn start_capture_inner(
    app: AppHandle,
    state: Arc<AppState>,
    mode: CaptureMode,
) -> Result<Option<ActiveSession>, AppError> {
    if updates::install_is_active(&app) {
        return Err(AppError::UpdateInstalling);
    }
    if recording::screenshot_capture_is_blocked(&state) || screenshot_countdown_is_active(&state) {
        return Err(AppError::CaptureInProgress);
    }
    // A failed overlay (image never loaded, webview stuck, etc.) leaves a session
    // behind. CaptureInProgress was silent on the shortcut path, so region mode
    // appeared completely dead until restart. Drop the stale session and retry.
    let stale_capture_generations = {
        let mut sessions = state.sessions.lock();
        sessions
            .drain()
            .map(|(_, session)| session.thumbnail_capture_generation)
            .collect::<Vec<_>>()
    };
    if !stale_capture_generations.is_empty() {
        eprintln!("clearing stuck capture session before starting {mode:?}");
        let mut visibility = state.thumbnail_visibility.lock();
        for capture_generation in stale_capture_generations {
            visibility.restore_capture(capture_generation);
        }
        drop(visibility);
        hide_capture_overlay(&app);
    }

    let thumbnail_capture_generation = begin_thumbnail_capture(&state)?;
    hide_capture_huds_before_snapshot(&app).await;

    let countdown_seconds = state.settings().screenshot_countdown_seconds;
    if countdown_seconds > 0 {
        let display = match display_under_pointer(&state) {
            Ok(display) => display,
            Err(error) => {
                set_capture_huds_protected(&app, false);
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error);
            }
        };
        match run_screenshot_countdown(
            app.clone(),
            state.clone(),
            &display,
            countdown_seconds,
            thumbnail_capture_generation,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                // Cancel already restored the stack and cleared HUD protection.
                return Ok(None);
            }
            Err(error) => {
                set_capture_huds_protected(&app, false);
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error);
            }
        }
    }

    let result = prepare_capture(
        app.clone(),
        state.clone(),
        mode,
        thumbnail_capture_generation,
    )
    .await;
    if result.is_err() {
        set_capture_huds_protected(&app, false);
        state.sessions.lock().clear();
        hide_capture_overlay(&app);
        restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
    }
    result
}

async fn prepare_capture(
    app: AppHandle,
    state: Arc<AppState>,
    mode: CaptureMode,
    thumbnail_capture_generation: u64,
) -> Result<Option<ActiveSession>, AppError> {
    let request_permission = mark_screen_permission_request(&state)?;
    if let Err(error) = state.backend.ensure_permission(request_permission) {
        if matches!(&error, CaptureError::PermissionRequestStarted) {
            *state.screen_permission_requested_this_launch.lock() = true;
        }
        return Err(error.into());
    }
    let display = display_under_pointer(&state)?;
    let frame = state.backend.capture_display(&display.id)?;
    // The background frame is frozen now, so this capture no longer needs HUD
    // exclusion. Release it before encoding can emit a new preview and allow a
    // rapid follow-up capture to start with its own protection generation.
    set_capture_huds_protected(&app, false);

    if mode == CaptureMode::Display {
        let _ = finish_capture(
            &app,
            &state,
            mode,
            frame.image,
            thumbnail_capture_generation,
        )
        .await?;
        return Ok(None);
    }

    let id = Uuid::new_v4();
    // Keep the selector background full-resolution and lossless. Region/window
    // crops also come directly from `frame.image`, so no lossy stage is involved.
    let snapshot_png = storage::encode_png(&frame.image)?;
    let mut windows = if mode == CaptureMode::Window {
        state
            .windows()?
            .into_iter()
            .filter(|window| window_is_capturable(window, &display))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if mode == CaptureMode::Window {
        refine_window_chrome_from_snapshot(
            &mut windows,
            &frame.descriptor,
            &frame.image,
            window_corner_radius_points(),
        );
    }
    let session = CaptureSession {
        id,
        mode,
        thumbnail_capture_generation,
        display: frame.descriptor,
        image: frame.image,
        snapshot_png,
        windows,
    };
    let active = ActiveSession {
        id: id.to_string(),
        mode,
        window_coordinate_scale: window_coordinate_scale(&session.display),
        window_corner_radius: window_corner_radius_points(),
        display: session.display.clone(),
        snapshot_url: models::snapshot_url(&id.to_string()),
        windows: session.windows.clone(),
    };
    state.sessions.lock().insert(id, session);
    show_capture_window(&app, &active);
    Ok(Some(active))
}

#[tauri::command]
async fn commit_region(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
    rect: LogicalRect,
) -> CommandResult<CaptureArtifact> {
    hide_capture_overlay(&app);
    let state = state.inner().clone();
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let session = state
        .sessions
        .lock()
        .remove(&id)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let thumbnail_capture_generation = session.thumbnail_capture_generation;
    // Map overlay/CSS DIPs onto the capture buffer. On Windows the display
    // descriptor is physical while the overlay is logical, so do not use the
    // native-geometry scale used for window crops.
    let scale = session
        .display
        .overlay_to_buffer_scale(session.image.width(), session.image.height());
    let physical = rect.to_physical(scale, session.image.width(), session.image.height());
    let Some(image) = session.view(physical) else {
        restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
        return Err(AppError::InvalidSelection.to_string());
    };

    let result = finish_capture(
        &app,
        &state,
        CaptureMode::Region,
        image,
        thumbnail_capture_generation,
    )
    .await;
    if result.is_err() {
        restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
    }
    result.map_err(|error| error.to_string())
}

#[tauri::command]
async fn commit_window(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
    window_id: String,
) -> CommandResult<CaptureArtifact> {
    hide_capture_overlay(&app);
    let state = state.inner().clone();
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let session = state
        .sessions
        .lock()
        .remove(&id)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let thumbnail_capture_generation = session.thumbnail_capture_generation;

    // Prefer cropping the freeze-frame (sharp, matches what the user saw). Live
    // CGWindow capture often returns black/empty frames for some windows on macOS.
    let image = match crop_window_from_session(&session, &window_id) {
        Some(image) if !image_is_effectively_blank(&image) => image,
        _ => match state.backend.capture_window(&window_id) {
            Ok(image) if !image_is_effectively_blank(&image) => image,
            Ok(_) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(
                    "Could not capture that window (empty frame). Try Region capture.".to_owned(),
                );
            }
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        },
    };

    let result = finish_capture(
        &app,
        &state,
        CaptureMode::Window,
        image,
        thumbnail_capture_generation,
    )
    .await;
    if result.is_err() {
        restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
    }
    result.map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_capture(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> CommandResult<()> {
    hide_capture_overlay(&app);
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    if let Some(session) = state.sessions.lock().remove(&id) {
        restore_thumbnail_capture(&app, state.inner(), session.thumbnail_capture_generation);
    }
    Ok(())
}

#[derive(Clone, serde::Serialize)]
struct ScreenshotCountdownTick {
    remaining_seconds: u8,
}

#[tauri::command]
fn cancel_screenshot_countdown(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<()> {
    cancel_screenshot_countdown_inner(&app, state.inner().clone());
    Ok(())
}

pub(crate) fn screenshot_countdown_is_active(state: &AppState) -> bool {
    state.screenshot_countdown.lock().active
}

/// Runs the full-display screenshot countdown. Returns `Ok(true)` when it
/// finishes, `Ok(false)` when the user cancels, or an error if the overlay fails.
pub(crate) async fn run_screenshot_countdown(
    app: AppHandle,
    state: Arc<AppState>,
    display: &captures_capture::DisplayDescriptor,
    seconds: u8,
    thumbnail_capture_generation: u64,
) -> Result<bool, AppError> {
    if seconds == 0 {
        return Ok(true);
    }

    let generation = {
        let mut runtime = state.screenshot_countdown.lock();
        if runtime.active {
            return Err(AppError::CaptureInProgress);
        }
        runtime.generation = runtime.generation.wrapping_add(1);
        runtime.active = true;
        runtime.thumbnail_capture_generation = Some(thumbnail_capture_generation);
        runtime.generation
    };

    if let Err(error) = show_screenshot_countdown(&app, display) {
        let mut runtime = state.screenshot_countdown.lock();
        if runtime.generation == generation {
            runtime.active = false;
            runtime.thumbnail_capture_generation = None;
        }
        return Err(error);
    }

    for remaining in (1..=seconds).rev() {
        if !screenshot_countdown_is_current(&state, generation) {
            destroy_screenshot_countdown(&app);
            return Ok(false);
        }
        let _ = app.emit(
            "screenshot-countdown",
            ScreenshotCountdownTick {
                remaining_seconds: remaining,
            },
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let completed = screenshot_countdown_is_current(&state, generation);
    {
        let mut runtime = state.screenshot_countdown.lock();
        if runtime.generation == generation {
            runtime.active = false;
            runtime.thumbnail_capture_generation = None;
        }
    }
    destroy_screenshot_countdown(&app);
    // Give the overlay a beat to leave the display before freezing a frame.
    if completed {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    Ok(completed)
}

fn screenshot_countdown_is_current(state: &AppState, generation: u64) -> bool {
    let runtime = state.screenshot_countdown.lock();
    runtime.active && runtime.generation == generation
}

pub(crate) fn cancel_screenshot_countdown_inner(app: &AppHandle, state: Arc<AppState>) {
    let thumbnail_capture_generation = {
        let mut runtime = state.screenshot_countdown.lock();
        if !runtime.active {
            return;
        }
        runtime.generation = runtime.generation.wrapping_add(1);
        runtime.active = false;
        runtime.thumbnail_capture_generation.take()
    };
    destroy_screenshot_countdown(app);
    set_capture_huds_protected(app, false);
    if let Some(thumbnail_capture_generation) = thumbnail_capture_generation {
        restore_thumbnail_capture(app, &state, thumbnail_capture_generation);
    }
}

fn show_screenshot_countdown(
    app: &AppHandle,
    display: &captures_capture::DisplayDescriptor,
) -> Result<(), AppError> {
    let (x, y, width, height) = display.overlay_geometry();
    if app.get_webview_window("screenshot-countdown").is_none() {
        WebviewWindowBuilder::new(
            app,
            "screenshot-countdown",
            WebviewUrl::App("index.html?view=screenshot-countdown".into()),
        )
        .title("Captures Screenshot Countdown")
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
        .get_webview_window("screenshot-countdown")
        .ok_or_else(|| AppError::Task("screenshot countdown is unavailable".to_owned()))?;
    window.set_size(tauri::LogicalSize::new(width, height))?;
    window.set_position(tauri::LogicalPosition::new(x, y))?;
    // Keep the countdown out of Captures' own recordings on Windows.
    window.set_content_protected(cfg!(target_os = "windows"))?;
    window.show()?;
    #[cfg(target_os = "macos")]
    recording::focus_recording_window(app, "screenshot-countdown");
    if let Err(error) = window.set_focus() {
        eprintln!("failed to focus screenshot countdown: {error}");
    }
    Ok(())
}

fn destroy_screenshot_countdown(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("screenshot-countdown")
        && let Err(error) = window.destroy()
    {
        eprintln!("failed to close screenshot countdown: {error}");
    }
}

#[tauri::command]
fn get_active_session(
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> CommandResult<Option<ActiveSession>> {
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    Ok(state.sessions.lock().get(&id).map(|session| ActiveSession {
        id: session.id.to_string(),
        mode: session.mode,
        window_coordinate_scale: window_coordinate_scale(&session.display),
        window_corner_radius: window_corner_radius_points(),
        display: session.display.clone(),
        snapshot_url: models::snapshot_url(&session.id.to_string()),
        windows: session.windows.clone(),
    }))
}

#[tauri::command]
fn get_pending_session(state: tauri::State<'_, Arc<AppState>>) -> Option<ActiveSession> {
    state
        .sessions
        .lock()
        .values()
        .next()
        .map(|session| ActiveSession {
            id: session.id.to_string(),
            mode: session.mode,
            window_coordinate_scale: window_coordinate_scale(&session.display),
            window_corner_radius: window_corner_radius_points(),
            display: session.display.clone(),
            snapshot_url: models::snapshot_url(&session.id.to_string()),
            windows: session.windows.clone(),
        })
}

#[tauri::command]
fn show_capture_overlay(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> CommandResult<()> {
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let mode = state
        .sessions
        .lock()
        .get(&id)
        .map(|session| session.mode)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    if let Some(window) = app.get_webview_window("overlay") {
        #[cfg(not(target_os = "macos"))]
        let cursor = if mode == CaptureMode::Region {
            CursorIcon::Crosshair
        } else {
            CursorIcon::Default
        };
        #[cfg(not(target_os = "macos"))]
        window
            .set_cursor_icon(cursor)
            .map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        captures_macos_window::prepare_capture_overlay(&window).map_err(str::to_owned)?;
        // Focusing the overlay activates Captures and would otherwise leave
        // open editors/history windows frontmost after the overlay hides.
        // Remember the user's app first so hide_capture_overlay can restore it.
        #[cfg(target_os = "macos")]
        captures_macos_window::remember_frontmost_app_before_activation();
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        if should_activate_capture_cursor_before_reveal(mode) {
            captures_macos_window::activate_capture_cursor(&window, false)
                .map_err(str::to_owned)?;
        }
        Ok(())
    } else {
        Err("capture overlay is unavailable".to_owned())
    }
}

#[tauri::command]
fn reveal_capture_overlay(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> CommandResult<()> {
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let mode = state
        .sessions
        .lock()
        .get(&id)
        .map(|session| session.mode)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let window = app
        .get_webview_window("overlay")
        .ok_or_else(|| "capture overlay is unavailable".to_owned())?;
    #[cfg(target_os = "macos")]
    {
        captures_macos_window::reveal_capture_overlay(&window).map_err(str::to_owned)?;
        captures_macos_window::activate_capture_cursor(&window, mode == CaptureMode::Region)
            .map_err(str::to_owned)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, mode);
    Ok(())
}

#[tauri::command]
fn sync_capture_cursor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> CommandResult<()> {
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let mode = state
        .sessions
        .lock()
        .get(&id)
        .map(|session| session.mode)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let window = app
        .get_webview_window("overlay")
        .ok_or_else(|| "capture overlay is unavailable".to_owned())?;
    #[cfg(target_os = "macos")]
    captures_macos_window::activate_capture_cursor(&window, mode == CaptureMode::Region)
        .map_err(str::to_owned)?;
    #[cfg(not(target_os = "macos"))]
    let _ = (window, mode);
    Ok(())
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, Arc<AppState>>) -> AppSettings {
    state.settings()
}

#[tauri::command]
fn set_shortcut_capture_suppressed(state: tauri::State<'_, Arc<AppState>>, suppressed: bool) {
    state
        .shortcut_capture_suppressed
        .store(suppressed, Ordering::Release);
}

#[derive(Clone, serde::Serialize)]
struct ThumbnailPointerPosition {
    x: f64,
    y: f64,
    inside: bool,
}

#[tauri::command]
fn get_thumbnail_pointer_position(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Option<ThumbnailPointerPosition> {
    #[cfg(target_os = "macos")]
    {
        if state.thumbnail_visibility.lock().is_suppressed() {
            return None;
        }
        let window = app.get_webview_window("thumbnail")?;
        if !window.is_visible().ok()? {
            return None;
        }
        let position = window.outer_position().ok()?;
        let size = window.inner_size().ok()?;
        let scale = window.scale_factor().ok()?.max(1.0);
        let (mouse_x, mouse_y) = match Mouse::get_mouse_position() {
            Mouse::Position { x, y } => (f64::from(x), f64::from(y)),
            Mouse::Error => return None,
        };
        Some(thumbnail_pointer_position(
            mouse_x,
            mouse_y,
            position.x,
            position.y,
            size.width,
            size.height,
            scale,
        ))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThumbnailCursorKind {
    Default,
    Pointer,
    Grab,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThumbnailCursorAction {
    Ignore,
    Reset,
    Apply(ThumbnailCursorKind),
}

#[cfg(any(target_os = "macos", test))]
fn thumbnail_cursor_action(
    suppressed: bool,
    visible: bool,
    kind: ThumbnailCursorKind,
) -> ThumbnailCursorAction {
    if suppressed {
        ThumbnailCursorAction::Ignore
    } else if visible {
        ThumbnailCursorAction::Apply(kind)
    } else {
        ThumbnailCursorAction::Reset
    }
}

#[cfg(target_os = "macos")]
fn apply_thumbnail_cursor_kind(
    window: &tauri::WebviewWindow,
    kind: ThumbnailCursorKind,
) -> Result<(), &'static str> {
    let native_kind = match kind {
        ThumbnailCursorKind::Default => captures_macos_window::ThumbnailCursorKind::Default,
        ThumbnailCursorKind::Pointer => captures_macos_window::ThumbnailCursorKind::Pointer,
        ThumbnailCursorKind::Grab => captures_macos_window::ThumbnailCursorKind::Grab,
    };
    captures_macos_window::set_thumbnail_cursor(window, native_kind)
}

#[cfg(target_os = "macos")]
fn reassert_thumbnail_cursor_kind(
    window: &tauri::WebviewWindow,
    kind: ThumbnailCursorKind,
) -> Result<(), &'static str> {
    let native_kind = match kind {
        ThumbnailCursorKind::Default => captures_macos_window::ThumbnailCursorKind::Default,
        ThumbnailCursorKind::Pointer => captures_macos_window::ThumbnailCursorKind::Pointer,
        ThumbnailCursorKind::Grab => captures_macos_window::ThumbnailCursorKind::Grab,
    };
    captures_macos_window::reassert_thumbnail_cursor(window, native_kind)
}

#[tauri::command]
fn set_thumbnail_cursor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    kind: ThumbnailCursorKind,
) -> CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        let window = app
            .get_webview_window("thumbnail")
            .ok_or_else(|| "capture thumbnail is unavailable".to_owned())?;
        let state = state.inner().clone();
        let cursor_window = window.clone();
        app.run_on_main_thread(move || {
            let suppressed = state.thumbnail_visibility.lock().is_suppressed();
            let visible = cursor_window.is_visible().unwrap_or(false);
            let result = match thumbnail_cursor_action(suppressed, visible, kind) {
                // NSCursor is application-wide. Do not even invalidate the
                // hidden preview's cursor rectangles while capture owns it.
                ThumbnailCursorAction::Ignore => return,
                ThumbnailCursorAction::Reset => {
                    let _ = cursor_window.set_cursor_icon(CursorIcon::Default);
                    captures_macos_window::reset_pointing_cursor_state(&cursor_window)
                }
                ThumbnailCursorAction::Apply(effective_kind) => {
                    // AppKit owns the inactive preview cursor on macOS. Asking
                    // both Tauri/WebKit and AppKit to set it lets their cursor
                    // rectangles alternate during focus handoffs.
                    apply_thumbnail_cursor_kind(&cursor_window, effective_kind)
                }
            };
            if let Err(error) = result {
                eprintln!("failed to update capture thumbnail cursor: {error}");
            }
        })
        .map_err(|error| error.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, kind);
        Ok(())
    }
}

#[tauri::command]
fn reassert_thumbnail_cursor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    kind: ThumbnailCursorKind,
) -> CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        let window = app
            .get_webview_window("thumbnail")
            .ok_or_else(|| "capture thumbnail is unavailable".to_owned())?;
        let state = state.inner().clone();
        app.run_on_main_thread(move || {
            let suppressed = state.thumbnail_visibility.lock().is_suppressed();
            let visible = window.is_visible().unwrap_or(false);
            let result = match thumbnail_cursor_action(suppressed, visible, kind) {
                ThumbnailCursorAction::Ignore => return,
                ThumbnailCursorAction::Reset => {
                    let _ = window.set_cursor_icon(CursorIcon::Default);
                    captures_macos_window::reset_pointing_cursor_state(&window)
                }
                ThumbnailCursorAction::Apply(effective_kind) => {
                    reassert_thumbnail_cursor_kind(&window, effective_kind)
                }
            };
            if let Err(error) = result {
                eprintln!("failed to reassert capture thumbnail cursor: {error}");
            }
        })
        .map_err(|error| error.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, kind);
        Ok(())
    }
}

#[tauri::command]
fn set_thumbnail_ignore_cursor_events(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    ignore: bool,
) -> CommandResult<()> {
    if state.thumbnail_visibility.lock().is_suppressed() {
        return Ok(());
    }
    let window = app
        .get_webview_window("thumbnail")
        .ok_or_else(|| "capture thumbnail is unavailable".to_owned())?;
    window
        .set_ignore_cursor_events(ignore)
        .map_err(|error| error.to_string())
}

/// Re-arm the preview stack after sleep/resume or a hung WebView.
///
/// Power transitions often leave `ignore_cursor_events` stuck, drop always-on-top,
/// or freeze hit testing so cards render but do not hover or click.
#[tauri::command]
fn refresh_thumbnail_interactivity(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<()> {
    let suppressed = state.thumbnail_visibility.lock().is_suppressed();
    let count = state.artifacts.lock().len();
    let Some(window) = app.get_webview_window("thumbnail") else {
        return Ok(());
    };
    // Always re-enable hit testing first — a stuck click-through state is the
    // usual "frozen previews" symptom after sleep.
    let _ = window.set_ignore_cursor_events(false);
    let _ = window.set_always_on_top(true);
    if count > 0 && !suppressed {
        show_thumbnail_window(&window);
        // Re-apply geometry after display sleep (DPI / work area can change).
        update_thumbnail_stack(&app);
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn thumbnail_pointer_position(
    mouse_x: f64,
    mouse_y: f64,
    window_x: i32,
    window_y: i32,
    window_width: u32,
    window_height: u32,
    scale: f64,
) -> ThumbnailPointerPosition {
    let scale = scale.max(1.0);
    let x = mouse_x - f64::from(window_x) / scale;
    let y = mouse_y - f64::from(window_y) / scale;
    let width = f64::from(window_width) / scale;
    let height = f64::from(window_height) / scale;
    ThumbnailPointerPosition {
        x,
        y,
        inside: x >= 0.0 && y >= 0.0 && x < width && y < height,
    }
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    mut settings: AppSettings,
) -> CommandResult<AppSettings> {
    if settings.output_directory.trim().is_empty() {
        settings.output_directory = models::default_output_directory()
            .to_string_lossy()
            .into_owned();
    }
    if settings.new_capture_shortcut.trim().is_empty()
        || settings.region_shortcut.trim().is_empty()
        || settings.window_shortcut.trim().is_empty()
        || settings.display_shortcut.trim().is_empty()
        || settings.recording.video_shortcut.trim().is_empty()
    {
        return Err("all shortcuts must be set".to_owned());
    }
    let new_capture_shortcut =
        parse_shortcut(&settings.new_capture_shortcut).map_err(|error| error.to_string())?;
    let region_shortcut =
        parse_shortcut(&settings.region_shortcut).map_err(|error| error.to_string())?;
    let window_shortcut =
        parse_shortcut(&settings.window_shortcut).map_err(|error| error.to_string())?;
    let display_shortcut =
        parse_shortcut(&settings.display_shortcut).map_err(|error| error.to_string())?;
    let video_shortcut =
        parse_shortcut(&settings.recording.video_shortcut).map_err(|error| error.to_string())?;
    let shortcuts = [
        new_capture_shortcut,
        region_shortcut,
        window_shortcut,
        display_shortcut,
        video_shortcut,
    ];
    if shortcuts
        .iter()
        .enumerate()
        .any(|(index, shortcut)| shortcuts[index + 1..].contains(shortcut))
    {
        return Err("shortcuts must be unique".to_owned());
    }
    if !matches!(settings.recording.video_fps, 15 | 30 | 60)
        || !matches!(settings.recording.gif_fps, 8..=30)
        || settings.recording.gif_max_width < 320
        || !(64..=256).contains(&settings.recording.gif_max_colors)
        || settings.recording.countdown_seconds > 10
        || settings.screenshot_countdown_seconds > 10
    {
        return Err("capture settings are outside their supported range".to_owned());
    }
    if !settings.custom_theme.is_valid() {
        return Err("custom theme colors must use #RRGGBB values".to_owned());
    }

    // Migration and permission bookkeeping are internal state, not
    // user-editable settings.
    let previous_settings = state.settings();
    settings.settings_schema_version = previous_settings.settings_schema_version;
    settings.last_screen_permission_request_id =
        previous_settings.last_screen_permission_request_id.clone();
    settings.pending_capture_after_restart = previous_settings.pending_capture_after_restart;

    let shortcuts_changed = settings.new_capture_shortcut != previous_settings.new_capture_shortcut
        || settings.region_shortcut != previous_settings.region_shortcut
        || settings.window_shortcut != previous_settings.window_shortcut
        || settings.display_shortcut != previous_settings.display_shortcut
        || settings.recording.video_shortcut != previous_settings.recording.video_shortcut;
    if shortcuts_changed && let Err(error) = register_shortcuts_with(&app, &settings) {
        let _ = register_shortcuts_with(&app, &previous_settings);
        return Err(error.to_string());
    }
    if settings.launch_at_login != previous_settings.launch_at_login {
        if settings.launch_at_login {
            app.autolaunch()
                .enable()
                .map_err(|error| error.to_string())?;
        } else {
            app.autolaunch()
                .disable()
                .map_err(|error| error.to_string())?;
        }
    }
    storage::save_settings(&settings).map_err(|error| error.to_string())?;
    let mini_preview_setting_changed =
        settings.show_mini_previews != previous_settings.show_mini_previews;
    *state.settings.write() = settings.clone();
    if mini_preview_setting_changed {
        if !settings.show_mini_previews {
            state
                .thumbnail_visibility
                .lock()
                .stop_waiting_for_artifact();
        }
        update_thumbnail_stack(&app);
    }
    if let Err(error) = app.emit("settings-changed", &settings) {
        eprintln!("failed to broadcast updated settings: {error}");
    }
    Ok(settings)
}

#[tauri::command]
fn get_artifacts(state: tauri::State<'_, Arc<AppState>>) -> Vec<CaptureArtifact> {
    state.artifacts.lock().clone()
}

#[tauri::command]
fn get_artifact(
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> Option<CaptureArtifact> {
    state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
}

#[derive(serde::Serialize)]
struct ArtifactDragPayload {
    path: String,
    icon_path: String,
}

#[tauri::command]
async fn prepare_artifact_drag(
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<ArtifactDragPayload> {
    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
        .ok_or_else(|| "artifact is no longer available".to_owned())?;
    let files =
        tauri::async_runtime::spawn_blocking(move || storage::prepare_artifact_drag(&artifact))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    let file_name = files
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("capture.png")
        .to_owned();
    *state.prepared_artifact_drag.lock() = Some(state::PreparedArtifactDrag {
        artifact_id,
        path: files.path.clone(),
        file_name,
    });
    Ok(ArtifactDragPayload {
        path: files.path.to_string_lossy().into_owned(),
        icon_path: files.icon_path.to_string_lossy().into_owned(),
    })
}

/// Called by in-app drop targets (screenshot editor) so a successful OS file
/// drop into Captures itself does not dismiss the source preview.
#[tauri::command]
fn mark_internal_file_drop(state: tauri::State<'_, Arc<AppState>>) {
    *state.last_internal_file_drop.lock() = Some(std::time::Instant::now());
}

/// Keep the preview when the drop landed on a Captures window or an in-app
/// drop target just accepted the file. External drops still dismiss.
#[tauri::command]
fn should_keep_preview_after_file_drop(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    x: f64,
    y: f64,
) -> bool {
    if let Some(at) = *state.last_internal_file_drop.lock() {
        // Editor marks the drop before the drag source gets Dropped; allow a
        // short window so the preview is not dismissed mid-import.
        if at.elapsed() <= std::time::Duration::from_millis(1_500) {
            return true;
        }
    }
    captures_window_contains_point(&app, x, y)
}

/// Read the full-resolution PNG staged for the current preview file drag.
///
/// Same-app drops into a webview sometimes deliver an unreadable/empty
/// `File` to HTML5 drop handlers. The editor falls back to this path.
#[tauri::command]
fn read_prepared_drag_image(
    state: tauri::State<'_, Arc<AppState>>,
    file_name: String,
) -> CommandResult<Vec<u8>> {
    let prepared = state
        .prepared_artifact_drag
        .lock()
        .clone()
        .ok_or_else(|| "no prepared drag image is available".to_owned())?;
    if prepared.file_name != file_name {
        return Err("file does not match the prepared drag image".to_owned());
    }
    if !prepared.path.is_file() {
        return Err("the prepared drag image is no longer available".to_owned());
    }
    fs::read(&prepared.path).map_err(|error| error.to_string())
}

/// Artifact id for the staged preview drag, when the drop is from a mini preview.
#[tauri::command]
fn prepared_drag_artifact_id(
    state: tauri::State<'_, Arc<AppState>>,
    file_name: String,
) -> Option<String> {
    let prepared = state.prepared_artifact_drag.lock().clone()?;
    if prepared.file_name != file_name {
        return None;
    }
    Some(prepared.artifact_id)
}

/// Screen-space hit test: is `(x, y)` over any visible Captures window?
///
/// Coordinates match `drag::CursorPosition` (top-left origin screen pixels on
/// macOS/Windows after the drag crate's conversion).
fn captures_window_contains_point(app: &AppHandle, x: f64, y: f64) -> bool {
    for (_label, window) in app.webview_windows() {
        let Ok(true) = window.is_visible() else {
            continue;
        };
        let Ok(position) = window.outer_position() else {
            continue;
        };
        let Ok(size) = window.outer_size() else {
            continue;
        };
        if screen_rect_contains_point(
            f64::from(position.x),
            f64::from(position.y),
            f64::from(size.width),
            f64::from(size.height),
            x,
            y,
        ) {
            return true;
        }
    }
    false
}

fn screen_rect_contains_point(
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
) -> bool {
    x >= left && x < left + width && y >= top && y < top + height
}

#[tauri::command]
fn get_capture_history(state: tauri::State<'_, Arc<AppState>>) -> Vec<ArtifactSummary> {
    let cutoff = Utc::now() - chrono::Duration::days(HISTORY_RETENTION_DAYS);
    let (history, expired_ids) = {
        let mut entries = state.history.lock();
        let mut expired_ids = Vec::new();
        entries.retain(|entry| {
            let recent = DateTime::parse_from_rfc3339(&entry.created_at)
                .map(|created_at| created_at.with_timezone(&Utc) >= cutoff)
                .unwrap_or(false);
            if !recent {
                expired_ids.push(entry.id.clone());
            }
            recent
        });
        (entries.clone(), expired_ids)
    };
    if !expired_ids.is_empty() {
        tauri::async_runtime::spawn_blocking(move || {
            for entry_id in expired_ids {
                if let Err(error) = storage::delete_history_capture(&entry_id) {
                    eprintln!("failed to prune capture history entry {entry_id}: {error}");
                }
            }
        });
    }
    history.iter().filter_map(HistoryEntry::summary).collect()
}

#[tauri::command]
async fn restore_history_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<CaptureArtifact> {
    let entry = state
        .history
        .lock()
        .iter()
        .find(|entry| entry.id == artifact_id)
        .cloned()
        .ok_or_else(|| AppError::HistoryUnavailable.to_string())?;
    if entry.kind != ArtifactKind::Screenshot {
        return Err("recordings can be opened directly from Capture History".to_owned());
    }
    let mode = entry
        .mode
        .ok_or_else(|| AppError::HistoryUnavailable.to_string())?;

    let existing_artifact = {
        state
            .artifacts
            .lock()
            .iter()
            .find(|artifact| artifact.id == artifact_id)
            .cloned()
    };
    let artifact = if let Some(artifact) = existing_artifact {
        artifact
    } else {
        let history_artifact_id = artifact_id.clone();
        let (image_png, preview_png) = tauri::async_runtime::spawn_blocking(move || {
            storage::load_history_images(&history_artifact_id)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        let artifact = CaptureArtifact {
            id: entry.id,
            path: None,
            preview_url: models::artifact_url(&artifact_id),
            full_url: models::artifact_full_url(&artifact_id),
            width: entry.width,
            height: entry.height,
            size_bytes: entry.size_bytes,
            created_at: entry.created_at,
            mode,
            history_saved: true,
            clipboard_copy_status: ClipboardCopyStatus::Skipped,
            image_png,
            preview_png,
        };
        state.artifacts.lock().push(artifact.clone());
        artifact
    };

    app.emit("capture-completed", &artifact)
        .map_err(|error| error.to_string())?;
    refresh_thumbnail_stack(&app);
    Ok(artifact)
}

#[tauri::command]
async fn delete_history_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    let available = state
        .history
        .lock()
        .iter()
        .any(|entry| entry.id == artifact_id);
    if !available {
        return Err(AppError::HistoryUnavailable.to_string());
    }

    let history_artifact_id = artifact_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        storage::delete_history_capture(&history_artifact_id)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    state.history.lock().retain(|entry| entry.id != artifact_id);
    state
        .recording_artifacts
        .lock()
        .retain(|artifact| artifact.summary.id != artifact_id);
    app.emit("capture-history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
async fn clear_capture_history(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<()> {
    let ids: Vec<String> = state
        .history
        .lock()
        .iter()
        .map(|entry| entry.id.clone())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }

    let ids_for_delete = ids.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for artifact_id in &ids_for_delete {
            storage::delete_history_capture(artifact_id)?;
        }
        Ok::<(), AppError>(())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;

    state.history.lock().clear();
    state
        .recording_artifacts
        .lock()
        .retain(|artifact| !ids.iter().any(|id| id == &artifact.summary.id));
    app.emit("capture-history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
async fn get_clipboard_state(
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<ClipboardState> {
    let state = state.inner().clone();
    #[cfg(target_os = "linux")]
    verify_linux_clipboard_ownership(&state).await;

    let revision = current_clipboard_revision();
    let artifact_id = state.clipboard_ownership.lock().current_artifact(revision);
    let artifact_id = artifact_id.filter(|artifact_id| {
        state
            .artifacts
            .lock()
            .iter()
            .any(|artifact| artifact.id == *artifact_id)
    });
    Ok(ClipboardState {
        revision,
        artifact_id,
    })
}

#[tauri::command]
async fn copy_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
        .ok_or_else(|| "artifact is no longer available".to_owned())?;
    let image = image::load_from_memory(&artifact.image_png)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let clipboard_write = copy_to_clipboard(&app, image)
        .await
        .map_err(|error| error.to_string())?;
    let artifact = {
        let mut artifacts = state.artifacts.lock();
        let artifact = artifacts
            .iter_mut()
            .find(|artifact| artifact.id == artifact_id)
            .ok_or_else(|| "artifact is no longer available".to_owned())?;
        artifact.clipboard_copy_status = ClipboardCopyStatus::Copied;
        artifact.clone()
    };
    state.clipboard_ownership.lock().record(
        clipboard_write.revision,
        artifact_id.clone(),
        clipboard_write.fingerprint,
    );
    app.emit("artifact-updated", &artifact)
        .map_err(|error| error.to_string())?;
    app.emit(
        "clipboard-owner-changed",
        &ClipboardState {
            revision: clipboard_write.revision,
            artifact_id: Some(artifact_id),
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
async fn save_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<CaptureArtifact> {
    let (png, existing_path) = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .map(|artifact| (artifact.image_png.clone(), artifact.path.clone()))
        .ok_or_else(|| "artifact is no longer available".to_owned())?;

    if existing_path.is_none() {
        let settings = state.settings();
        let path = tauri::async_runtime::spawn_blocking(move || {
            storage::save_encoded_capture(&png, &settings)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        let path = path.to_string_lossy().into_owned();
        let mut artifacts = state.artifacts.lock();
        let Some(artifact) = artifacts
            .iter_mut()
            .find(|artifact| artifact.id == artifact_id)
        else {
            let _ = fs::remove_file(&path);
            return Err("artifact is no longer available".to_owned());
        };
        artifact.path = Some(path);
    }

    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
        .ok_or_else(|| "artifact is no longer available".to_owned())?;
    app.emit("artifact-updated", &artifact)
        .map_err(|error| error.to_string())?;
    Ok(artifact)
}

#[tauri::command]
fn reveal_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
        .ok_or_else(|| "artifact is no longer available".to_owned())?;
    let path = artifact
        .path
        .ok_or_else(|| "Save this capture before showing it in its folder".to_owned())?;
    app.opener()
        .reveal_item_in_dir(PathBuf::from(path))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn trash_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
        .ok_or_else(|| "artifact is no longer available".to_owned())?;
    if let Some(path) = artifact.path {
        trash::delete(path).map_err(|error| error.to_string())?;
    }
    remove_artifact(&app, state.inner(), &artifact_id)?;
    Ok(())
}

#[tauri::command]
fn dismiss_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    remove_artifact(&app, state.inner(), &artifact_id)
}

// WebView2 can stall a newly constructed webview at about:blank when window
// creation runs inside its synchronous IPC callback. Force these commands
// onto Tauri's async executor before they dispatch window work.
#[tauri::command(async)]
fn open_artifact_viewer(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    let artifact_available = state
        .artifacts
        .lock()
        .iter()
        .any(|artifact| artifact.id == artifact_id);
    if !artifact_available {
        return Err("artifact is no longer available".to_owned());
    }

    let label = viewer_window_label(&artifact_id);
    if let Some(window) = app.get_webview_window(&label) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let viewer_count = app
        .webview_windows()
        .keys()
        .filter(|label| label.starts_with(VIEWER_WINDOW_PREFIX))
        .count();
    let window = WebviewWindowBuilder::new(
        &app,
        label,
        WebviewUrl::App(format!("index.html?view=viewer&artifact_id={artifact_id}").into()),
    )
    .title("Captures Preview")
    .inner_size(1_000.0, 700.0)
    .min_inner_size(560.0, 400.0)
    .center()
    .resizable(true)
    .background_color(Color(17, 18, 26, 255))
    .focused(false)
    .visible(false)
    .build()
    .map_err(|error| error.to_string())?;
    if viewer_count > 0 {
        let scale = window.scale_factor().unwrap_or(1.0);
        let offset = ((viewer_count % 6) as f64 * 28.0 * scale).round() as i32;
        if let Ok(position) = window.outer_position() {
            let _ = window.set_position(tauri::PhysicalPosition::new(
                position.x.saturating_add(offset),
                position.y.saturating_add(offset),
            ));
        }
    }
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

const VIEWER_WINDOW_PREFIX: &str = "viewer-";

fn viewer_window_label(artifact_id: &str) -> String {
    format!("{VIEWER_WINDOW_PREFIX}{artifact_id}")
}

fn remove_artifact(app: &AppHandle, state: &Arc<AppState>, artifact_id: &str) -> CommandResult<()> {
    {
        let mut artifacts = state.artifacts.lock();
        let original_len = artifacts.len();
        artifacts.retain(|artifact| artifact.id != artifact_id);
        if artifacts.len() == original_len {
            return Err("artifact is no longer available".to_owned());
        }
    }
    app.emit("artifact-removed", artifact_id)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_captures_folder(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<()> {
    let path = PathBuf::from(state.settings().output_directory);
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
fn open_capture_history(app: AppHandle) -> CommandResult<()> {
    show_capture_history(&app);
    Ok(())
}

#[tauri::command(async)]
fn open_preferences(app: AppHandle) -> CommandResult<()> {
    show_preferences(&app);
    Ok(())
}

#[tauri::command]
fn dismiss_recording_saved_notice(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<()> {
    state
        .recording_saved_notice_generation
        .fetch_add(1, Ordering::Relaxed);
    if let Some(window) = app.get_webview_window(RECORDING_SAVED_NOTICE_LABEL) {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn finish_capture(
    app: &AppHandle,
    state: &Arc<AppState>,
    mode: CaptureMode,
    image: RgbaImage,
    thumbnail_capture_generation: u64,
) -> Result<CaptureArtifact, AppError> {
    #[cfg(target_os = "macos")]
    if let Err(error) = app.run_on_main_thread(|| {
        if let Err(error) = captures_macos_window::play_capture_sound() {
            eprintln!("failed to play capture sound: {error}");
        }
    }) {
        eprintln!("failed to schedule capture sound: {error}");
    }

    let width = image.width();
    let height = image.height();
    let artifact_id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let history_artifact_id = artifact_id.clone();
    let history_created_at = created_at.clone();
    let image_for_encoding = image.clone();
    let encode_task = tauri::async_runtime::spawn_blocking(move || -> Result<_, AppError> {
        let image_png = storage::encode_png(&image_for_encoding)?;
        let preview_png = storage::encode_thumbnail_png(&image_for_encoding)?;
        let history_entry = HistoryEntry {
            id: history_artifact_id.clone(),
            kind: ArtifactKind::Screenshot,
            preview_url: models::history_preview_url(&history_artifact_id),
            full_url: models::history_full_url(&history_artifact_id),
            width,
            height,
            size_bytes: u64::try_from(image_png.len()).unwrap_or(u64::MAX),
            created_at: history_created_at,
            mode: Some(mode),
            saved_path: None,
            mime_type: None,
            duration_ms: None,
            target: None,
            has_system_audio: false,
            has_microphone_audio: false,
            dropped_frames: 0,
        };
        let history_saved =
            match storage::save_history_capture(&history_entry, &image_png, &preview_png) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("failed to save capture history: {error}");
                    false
                }
            };
        Ok((image_png, preview_png, history_entry, history_saved))
    });
    let clipboard_task = state.settings().auto_copy_to_clipboard.then(|| {
        let clipboard_app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            write_image_to_clipboard(&clipboard_app, image)
        })
    });
    let (image_png, preview_png, history_entry, history_saved) = encode_task
        .await
        .map_err(|error| AppError::Task(error.to_string()))??;
    let size_bytes = u64::try_from(image_png.len()).unwrap_or(u64::MAX);
    let mut artifact = CaptureArtifact {
        id: artifact_id.clone(),
        preview_url: models::artifact_url(&artifact_id),
        full_url: models::artifact_full_url(&artifact_id),
        path: None,
        width,
        height,
        size_bytes,
        created_at,
        mode,
        history_saved,
        clipboard_copy_status: if clipboard_task.is_some() {
            ClipboardCopyStatus::Pending
        } else {
            ClipboardCopyStatus::Skipped
        },
        image_png,
        preview_png,
    };
    if history_saved {
        state.history.lock().insert(0, history_entry);
    }
    state.artifacts.lock().push(artifact.clone());
    if state.settings().show_mini_previews {
        let waiting = state
            .thumbnail_visibility
            .lock()
            .wait_for_artifact(thumbnail_capture_generation, artifact.id.clone());
        if !waiting {
            eprintln!(
                "capture preview {} arrived after its visibility generation was replaced",
                artifact.id
            );
        }
    } else {
        state
            .thumbnail_visibility
            .lock()
            .restore_capture(thumbnail_capture_generation);
    }
    app.emit("capture-completed", &artifact)?;
    if !state.settings().show_mini_previews {
        update_thumbnail_stack(app);
    }
    if history_saved {
        app.emit("capture-history-changed", ())?;
    }

    if let Some(clipboard_task) = clipboard_task {
        let clipboard_result = clipboard_task
            .await
            .map_err(|error| AppError::Task(error.to_string()))?;
        artifact.clipboard_copy_status = match clipboard_result {
            Ok(clipboard_write) => {
                state.clipboard_ownership.lock().record(
                    clipboard_write.revision,
                    artifact.id.clone(),
                    clipboard_write.fingerprint,
                );
                app.emit(
                    "clipboard-owner-changed",
                    &ClipboardState {
                        revision: clipboard_write.revision,
                        artifact_id: Some(artifact.id.clone()),
                    },
                )?;
                ClipboardCopyStatus::Copied
            }
            Err(_) => ClipboardCopyStatus::Failed,
        };
        if let Some(stored) = state
            .artifacts
            .lock()
            .iter_mut()
            .find(|stored| stored.id == artifact.id)
        {
            stored.clipboard_copy_status = artifact.clipboard_copy_status;
        }
        app.emit("artifact-updated", &artifact)?;
    }
    Ok(artifact)
}

async fn copy_to_clipboard(app: &AppHandle, image: RgbaImage) -> Result<ClipboardWrite, AppError> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || write_image_to_clipboard(&app, image))
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
}

fn write_image_to_clipboard(app: &AppHandle, image: RgbaImage) -> Result<ClipboardWrite, AppError> {
    let width = image.width();
    let height = image.height();
    let rgba = image.into_raw();
    let fingerprint = clipboard_fingerprint(width, height, &rgba);
    let clipboard_image = Image::new_owned(rgba, width, height);
    app.clipboard()
        .write_image(&clipboard_image)
        .map_err(|error| AppError::Clipboard(error.to_string()))?;
    Ok(ClipboardWrite {
        revision: record_clipboard_write(),
        fingerprint,
    })
}

fn clipboard_fingerprint(width: u32, height: u32, rgba: &[u8]) -> ClipboardFingerprint {
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    rgba.hash(&mut hasher);
    ClipboardFingerprint {
        width,
        height,
        checksum: hasher.finish(),
    }
}

#[cfg(target_os = "macos")]
fn current_clipboard_revision() -> isize {
    captures_macos_window::clipboard_change_count()
}

#[cfg(target_os = "macos")]
fn record_clipboard_write() -> isize {
    current_clipboard_revision()
}

#[cfg(target_os = "windows")]
static WINDOWS_CLIPBOARD_REVISION_FALLBACK: AtomicIsize = AtomicIsize::new(0);

#[cfg(target_os = "windows")]
fn windows_clipboard_revision() -> Option<isize> {
    clipboard_win::seq_num().map(|revision| {
        isize::try_from(revision.get()).unwrap_or_else(|_| {
            // Captures currently ships 64-bit Windows bundles, but retain a
            // monotonic fallback if a 32-bit target cannot represent u32.
            WINDOWS_CLIPBOARD_REVISION_FALLBACK.load(Ordering::Acquire)
        })
    })
}

#[cfg(target_os = "windows")]
fn current_clipboard_revision() -> isize {
    windows_clipboard_revision()
        .unwrap_or_else(|| WINDOWS_CLIPBOARD_REVISION_FALLBACK.load(Ordering::Acquire))
}

#[cfg(target_os = "windows")]
fn record_clipboard_write() -> isize {
    if let Some(revision) = windows_clipboard_revision() {
        WINDOWS_CLIPBOARD_REVISION_FALLBACK.store(revision, Ordering::Release);
        revision
    } else {
        WINDOWS_CLIPBOARD_REVISION_FALLBACK
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }
}

#[cfg(target_os = "linux")]
static APPLICATION_CLIPBOARD_REVISION: AtomicIsize = AtomicIsize::new(0);

#[cfg(target_os = "linux")]
fn current_clipboard_revision() -> isize {
    APPLICATION_CLIPBOARD_REVISION.load(Ordering::Acquire)
}

#[cfg(target_os = "linux")]
fn record_clipboard_write() -> isize {
    APPLICATION_CLIPBOARD_REVISION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1)
}

#[cfg(target_os = "linux")]
async fn verify_linux_clipboard_ownership(state: &Arc<AppState>) {
    const MINIMUM_VERIFICATION_INTERVAL: Duration = Duration::from_secs(1);

    let Some(verification) = state
        .clipboard_ownership
        .lock()
        .verification(Instant::now(), MINIMUM_VERIFICATION_INTERVAL)
    else {
        return;
    };
    let expected = verification.fingerprint;
    let result =
        tauri::async_runtime::spawn_blocking(move || linux_clipboard_matches(expected)).await;
    match result {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            if APPLICATION_CLIPBOARD_REVISION
                .compare_exchange(
                    verification.revision,
                    verification.revision.wrapping_add(1),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                state
                    .clipboard_ownership
                    .lock()
                    .clear_if_revision(verification.revision);
            }
        }
        Ok(Err(error)) => {
            eprintln!("failed to verify the Linux clipboard owner: {error}");
        }
        Err(error) => {
            eprintln!("Linux clipboard verification task failed: {error}");
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_clipboard_matches(expected: ClipboardFingerprint) -> Result<bool, AppError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| AppError::Clipboard(error.to_string()))?;
    match clipboard.get_image() {
        Ok(image) => {
            let width = u32::try_from(image.width).unwrap_or(u32::MAX);
            let height = u32::try_from(image.height).unwrap_or(u32::MAX);
            Ok(clipboard_fingerprint(width, height, &image.bytes) == expected)
        }
        Err(arboard::Error::ContentNotAvailable | arboard::Error::ConversionFailure) => Ok(false),
        Err(error) => Err(AppError::Clipboard(error.to_string())),
    }
}

fn display_under_pointer(
    state: &AppState,
) -> Result<captures_capture::DisplayDescriptor, AppError> {
    let displays = state.monitors()?;
    pointer_position()
        .and_then(|(x, y)| {
            displays
                .iter()
                .find(|display| {
                    let pointer_scale = if cfg!(target_os = "linux") {
                        display.scale_factor
                    } else {
                        1.0
                    };
                    display_contains_pointer(display, x, y, pointer_scale)
                })
                .cloned()
        })
        .or_else(|| displays.iter().find(|display| display.is_primary).cloned())
        .or_else(|| displays.first().cloned())
        .ok_or(CaptureError::TargetUnavailable.into())
}

fn pointer_position() -> Option<(i32, i32)> {
    #[cfg(target_os = "linux")]
    if !x11_display_available() {
        // mouse_position uses Xlib and dereferences a null display on a
        // Wayland-only session. Fall back to the primary monitor instead.
        return None;
    }

    match Mouse::get_mouse_position() {
        Mouse::Position { x, y } => Some((x, y)),
        Mouse::Error => None,
    }
}

fn display_contains_pointer(
    display: &captures_capture::DisplayDescriptor,
    pointer_x: i32,
    pointer_y: i32,
    pointer_scale: f64,
) -> bool {
    let scale = pointer_scale.max(1.0);
    let x = f64::from(pointer_x) / scale;
    let y = f64::from(pointer_y) / scale;
    let left = f64::from(display.x);
    let top = f64::from(display.y);
    x >= left
        && y >= top
        && x < left + f64::from(display.width)
        && y < top + f64::from(display.height)
}

fn mark_screen_permission_request(state: &AppState) -> Result<bool, AppError> {
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe()?;
        let metadata = executable.metadata()?;
        let modified = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let request_id = format!(
            "{}:{}:{modified}",
            executable.to_string_lossy(),
            metadata.len()
        );
        let mut settings = state.settings.write();
        if settings.last_screen_permission_request_id.as_deref() == Some(&request_id) {
            return Ok(false);
        }
        settings.last_screen_permission_request_id = Some(request_id);
        storage::save_settings(&settings)?;
        Ok(true)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        Ok(false)
    }
}

fn take_pending_capture_after_restart(state: &AppState) -> Result<Option<CaptureMode>, AppError> {
    let mut settings = state.settings.write();
    let pending = settings.pending_capture_after_restart.take();
    if pending.is_some() {
        storage::save_settings(&settings)?;
    }
    Ok(pending)
}

#[cfg(target_os = "macos")]
fn restart_and_retry_capture(app: &AppHandle, mode: CaptureMode) -> Result<(), AppError> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    {
        let mut settings = state.settings.write();
        settings.pending_capture_after_restart = Some(mode);
        storage::save_settings(&settings)?;
    }
    app.request_restart();
    Ok(())
}

fn window_coordinate_scale(display: &captures_capture::DisplayDescriptor) -> f64 {
    #[cfg(target_os = "windows")]
    return display.scale_factor.max(1.0);

    #[cfg(not(target_os = "windows"))]
    {
        let _ = display;
        1.0
    }
}

fn window_corner_radius_points() -> f64 {
    #[cfg(target_os = "macos")]
    {
        captures_macos_window::standard_window_corner_radius_points()
    }
    #[cfg(not(target_os = "macos"))]
    {
        0.0
    }
}

fn register_shortcuts(app: &AppHandle) -> Result<(), AppError> {
    let settings = app.state::<Arc<AppState>>().settings();
    register_shortcuts_with(app, &settings)
}

fn register_shortcuts_with(app: &AppHandle, settings: &AppSettings) -> Result<(), AppError> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|error| AppError::Shortcut(error.to_string()))?;
    register_new_capture_shortcut(app, &settings.new_capture_shortcut)?;
    register_shortcut(app, &settings.region_shortcut, CaptureMode::Region)?;
    register_shortcut(app, &settings.window_shortcut, CaptureMode::Window)?;
    register_shortcut(app, &settings.display_shortcut, CaptureMode::Display)?;
    register_recording_shortcut(app, &settings.recording.video_shortcut)?;
    Ok(())
}

fn register_new_capture_shortcut(app: &AppHandle, shortcut: &str) -> Result<(), AppError> {
    let parsed = parse_shortcut(shortcut)?;
    let armed = AtomicBool::new(false);
    app.global_shortcut()
        .on_shortcut(parsed, move |app, _shortcut, event| {
            if !should_trigger_shortcut(&armed, event.state()) {
                return;
            }
            if app
                .get_webview_window("preferences")
                .is_some_and(|window| window.is_focused().unwrap_or(false))
            {
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                wait_for_capture_shortcut_release().await;
                open_capture_controls(&app, CaptureSelectorMode::Screenshot);
            });
        })
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn register_shortcut(app: &AppHandle, shortcut: &str, mode: CaptureMode) -> Result<(), AppError> {
    let parsed = parse_shortcut(shortcut)?;
    let armed = AtomicBool::new(false);
    let suppressed_while_pressed = AtomicBool::new(false);
    app.global_shortcut()
        .on_shortcut(parsed, move |app, _shortcut, event| {
            let state = app.state::<Arc<AppState>>().inner().clone();
            let suppressed = shortcut_capture_is_suppressed(app, &state);
            let trigger_is_suppressed =
                track_shortcut_suppression(&suppressed_while_pressed, event.state(), suppressed);
            if !should_trigger_shortcut(&armed, event.state()) || trigger_is_suppressed {
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                wait_for_capture_shortcut_release().await;
                if let Err(error) = start_capture_inner(app.clone(), state, mode).await
                    && !matches!(&error, AppError::CaptureInProgress)
                {
                    report_capture_error(&app, &error, mode);
                }
            });
        })
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn register_recording_shortcut(app: &AppHandle, shortcut: &str) -> Result<(), AppError> {
    let parsed = parse_shortcut(shortcut)?;
    let armed = AtomicBool::new(false);
    let suppressed_while_pressed = AtomicBool::new(false);
    app.global_shortcut()
        .on_shortcut(parsed, move |app, _shortcut, event| {
            let state = app.state::<Arc<AppState>>().inner().clone();
            let suppressed = shortcut_capture_is_suppressed(app, &state);
            let trigger_is_suppressed =
                track_shortcut_suppression(&suppressed_while_pressed, event.state(), suppressed);
            if !should_trigger_shortcut(&armed, event.state()) || trigger_is_suppressed {
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                wait_for_capture_shortcut_release().await;
                if let Err(error) = recording::prepare_recording_inner(app.clone(), state).await
                    && !matches!(&error, AppError::CaptureInProgress)
                {
                    report_recording_error(&app, &error);
                }
            });
        })
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn shortcut_capture_is_suppressed(app: &AppHandle, state: &AppState) -> bool {
    state.shortcut_capture_suppressed.load(Ordering::Acquire)
        && app
            .get_webview_window("preferences")
            .is_some_and(|window| window.is_focused().unwrap_or(false))
}

fn track_shortcut_suppression(
    suppressed_while_pressed: &AtomicBool,
    state: ShortcutState,
    currently_suppressed: bool,
) -> bool {
    match state {
        ShortcutState::Pressed => {
            suppressed_while_pressed.store(currently_suppressed, Ordering::Release);
            currently_suppressed
        }
        ShortcutState::Released => {
            suppressed_while_pressed.swap(false, Ordering::AcqRel) || currently_suppressed
        }
    }
}

async fn wait_for_capture_shortcut_release() {
    #[cfg(target_os = "macos")]
    {
        use std::time::Duration;

        const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(5);
        const APPKIT_RELEASE_SETTLE_TIME: Duration = Duration::from_millis(16);

        while captures_macos_window::capture_shortcut_modifiers_pressed() {
            tokio::time::sleep(MODIFIER_POLL_INTERVAL).await;
        }
        // `modifierFlags` becomes clear during the flags-changed event. Give
        // AppKit one display beat to finish its arrow-cursor restoration before
        // the capture overlay claims the cursor exactly once.
        tokio::time::sleep(APPKIT_RELEASE_SETTLE_TIME).await;
    }
}

fn parse_shortcut(shortcut: &str) -> Result<Shortcut, AppError> {
    shortcut
        .parse::<Shortcut>()
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn should_trigger_shortcut(armed: &AtomicBool, state: ShortcutState) -> bool {
    match state {
        ShortcutState::Pressed => {
            armed.store(true, Ordering::Release);
            false
        }
        ShortcutState::Released => armed.swap(false, Ordering::AcqRel),
    }
}

#[cfg(any(target_os = "macos", test))]
fn should_activate_capture_cursor_before_reveal(mode: CaptureMode) -> bool {
    mode != CaptureMode::Region
}

fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let new_capture = MenuItem::with_id(app, "new-capture", "New Capture…", true, None::<&str>)?;
    let capture_history = MenuItem::with_id(
        app,
        "capture-history",
        "Capture History…",
        true,
        None::<&str>,
    )?;
    let open_folder =
        MenuItem::with_id(app, "open-folder", "Open Save Location", true, None::<&str>)?;
    let preferences = MenuItem::with_id(app, "preferences", "Preferences", true, None::<&str>)?;
    let update_item = MenuItem::with_id(
        app,
        "check-updates",
        "Check for Updates…",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Captures", true, None::<&str>)?;
    let separator_1 = MenuItem::with_id(app, "separator-1", "────────", false, None::<&str>)?;
    let separator_2 = MenuItem::with_id(app, "separator-2", "────────", false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &new_capture,
            &separator_1,
            &capture_history,
            &open_folder,
            &preferences,
            &update_item,
            &separator_2,
            &quit,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("Captures — Screen capture utility");

    #[cfg(target_os = "macos")]
    if let Some(icon) = macos_tray_icon() {
        tray = tray.icon(icon).icon_as_template(true);
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    tray.on_menu_event(|app, event| match event.id().as_ref() {
        "new-capture" => {
            open_capture_controls(app, CaptureSelectorMode::Screenshot);
        }
        "capture-history" => {
            show_capture_history(app);
        }
        "open-folder" => {
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                let path = PathBuf::from(state.settings().output_directory);
                let _ = fs::create_dir_all(&path);
                let _ = app.opener().open_path(path.to_string_lossy(), None::<&str>);
            }
        }
        "preferences" => {
            show_preferences(app);
        }
        "check-updates" => {
            updates::handle_tray_action(app);
        }
        "quit" => {
            app.exit(0);
        }
        _ => {}
    })
    .build(app)?;

    updates::register_menu_item(app.handle(), update_item);
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_tray_icon() -> Option<Image<'static>> {
    let source = image::load_from_memory(include_bytes!("../icons/icon.png"))
        .ok()?
        .to_rgba8();
    let mut icon = image::imageops::resize(&source, 22, 22, image::imageops::FilterType::Lanczos3);
    for pixel in icon.pixels_mut() {
        let [red, green, blue, alpha] = pixel.0;
        let minimum = red.min(green).min(blue);
        let maximum = red.max(green).max(blue);
        pixel.0 = if minimum >= 180 && maximum - minimum <= 55 {
            [255, 255, 255, alpha]
        } else {
            [0, 0, 0, 0]
        };
    }
    Some(Image::new_owned(icon.into_raw(), 22, 22))
}

fn show_capture_window(app: &AppHandle, session: &ActiveSession) {
    let display = session.display.clone();
    let session = session.clone();
    let app = app.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        // On Windows xcap geometry is physical; Tauri LogicalSize expects DIPs.
        let (x, y, width, height) = display.overlay_geometry();
        if handle.get_webview_window("overlay").is_none()
            && let Err(error) = create_overlay_window(&handle)
        {
            eprintln!("failed to create capture overlay: {error}");
            return;
        }
        if let Some(window) = handle.get_webview_window("overlay") {
            // Size first, then position. A borderless NSWindow grows from its
            // bottom-left anchor; positioning after the final size keeps the
            // top-left edge on the selected display (same as the recording selector).
            let _ = window.set_size(LogicalSize::new(width, height));
            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
            #[cfg(target_os = "linux")]
            let _ = window.set_fullscreen(wayland_session());
            if let Err(error) = handle.emit("capture-session-ready", &session) {
                eprintln!("failed to prepare capture session: {error}");
            }
        }
    });
}

fn create_overlay_window(app: &AppHandle) -> Result<(), tauri::Error> {
    let builder = WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App("index.html?view=overlay".into()),
    )
    .title("Captures")
    .inner_size(1.0, 1.0)
    .position(-10_000.0, -10_000.0);
    #[cfg(target_os = "linux")]
    let builder = if wayland_session() {
        builder.fullscreen(true)
    } else {
        builder
    };
    let window = builder
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
        .build()?;

    #[cfg(target_os = "macos")]
    captures_macos_window::configure_capture_overlay(&window)
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;
    #[cfg(not(target_os = "macos"))]
    let _ = window;

    Ok(())
}

const STARTUP_NOTICE_WIDTH: f64 = 356.0;
const STARTUP_NOTICE_HEIGHT: f64 = 112.0;

fn show_startup_notice(app: &AppHandle) -> Result<(), tauri::Error> {
    let (x, y) = startup_notice_position(app);
    let window = WebviewWindowBuilder::new(
        app,
        "startup",
        WebviewUrl::App("index.html?view=startup".into()),
    )
    .title("Captures is running")
    .inner_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT)
    .position(x, y)
    .decorations(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .focused(false)
    .visible(false)
    .build()?;
    window.set_ignore_cursor_events(true)?;

    #[cfg(target_os = "macos")]
    captures_macos_window::show_without_activating(&window)
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;

    #[cfg(not(target_os = "macos"))]
    window.show()?;

    let timer_app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let handle = timer_app.clone();
        let _ = timer_app.run_on_main_thread(move || {
            if let Some(window) = handle.get_webview_window("startup") {
                let _ = window.hide();
            }
        });
    });
    Ok(())
}

fn startup_notice_position(app: &AppHandle) -> (f64, f64) {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            let scale = monitor.scale_factor().max(1.0);
            let position = monitor.position();
            let size = monitor.size();
            let left = f64::from(position.x) / scale;
            let top = f64::from(position.y) / scale;
            let right = left + f64::from(size.width) / scale;
            (right - STARTUP_NOTICE_WIDTH - 18.0, top + 30.0)
        })
        .unwrap_or((20.0, 30.0))
}

const RECORDING_SAVED_NOTICE_WIDTH: f64 = 440.0;
const RECORDING_SAVED_NOTICE_HEIGHT: f64 = 116.0;
const RECORDING_CONTROLS_HIDDEN_NOTICE_WIDTH: f64 = 390.0;
const RECORDING_CONTROLS_HIDDEN_NOTICE_HEIGHT: f64 = 82.0;

#[derive(Clone, serde::Serialize)]
struct RecordingSavedNoticePayload {
    artifact_id: String,
    generation: u64,
}

fn show_recording_saved_notice(app: &AppHandle, artifact_id: &str) -> Result<(), tauri::Error> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let available = state.recording_artifacts.lock().iter().any(|artifact| {
        artifact.summary.id == artifact_id && PathBuf::from(&artifact.summary.path).is_file()
    });
    if !available {
        return Ok(());
    }

    let generation = state
        .recording_saved_notice_generation
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    let (x, y) = top_right_notice_position(
        app,
        RECORDING_SAVED_NOTICE_WIDTH,
        RECORDING_SAVED_NOTICE_HEIGHT,
    );
    let payload = RecordingSavedNoticePayload {
        artifact_id: artifact_id.to_owned(),
        generation,
    };
    let window = if let Some(window) = app.get_webview_window(RECORDING_SAVED_NOTICE_LABEL) {
        window.emit(RECORDING_SAVED_NOTICE_EVENT, &payload)?;
        window
    } else {
        WebviewWindowBuilder::new(
            app,
            RECORDING_SAVED_NOTICE_LABEL,
            WebviewUrl::App(
                format!("index.html?view=recording-saved&artifact_id={artifact_id}").into(),
            ),
        )
        .title("Recording saved")
        .inner_size(RECORDING_SAVED_NOTICE_WIDTH, RECORDING_SAVED_NOTICE_HEIGHT)
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
        .build()?
    };
    let _ = window.set_content_protected(true);

    #[cfg(target_os = "macos")]
    {
        captures_macos_window::configure_inactive_hover(&window)
            .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;
        captures_macos_window::show_without_activating(&window)
            .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;
    }

    #[cfg(not(target_os = "macos"))]
    window.show()?;

    let timer_app = app.clone();
    let timer_state = state;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(11_200));
        let handle = timer_app.clone();
        let _ = timer_app.run_on_main_thread(move || {
            if timer_state
                .recording_saved_notice_generation
                .load(Ordering::Relaxed)
                == generation
                && let Some(window) = handle.get_webview_window(RECORDING_SAVED_NOTICE_LABEL)
            {
                let _ = window.hide();
            }
        });
    });
    Ok(())
}

fn show_recording_controls_hidden_notice(
    app: &AppHandle,
    position: Option<(f64, f64)>,
) -> Result<(), tauri::Error> {
    for (label, window) in app.webview_windows() {
        if label.starts_with(RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX) {
            window.destroy()?;
        }
    }
    let label = format!(
        "{RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX}{}",
        Uuid::new_v4()
    );
    let (x, y) = position.unwrap_or_else(|| {
        bottom_center_notice_position(
            app,
            RECORDING_CONTROLS_HIDDEN_NOTICE_WIDTH,
            RECORDING_CONTROLS_HIDDEN_NOTICE_HEIGHT,
        )
    });
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App("index.html?view=recording-controls-hidden".into()),
    )
    .title("Recording controls hidden")
    .inner_size(
        RECORDING_CONTROLS_HIDDEN_NOTICE_WIDTH,
        RECORDING_CONTROLS_HIDDEN_NOTICE_HEIGHT,
    )
    .position(x, y)
    .decorations(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .focused(false)
    .visible(false)
    .build()?;
    let _ = window.set_content_protected(true);
    window.set_ignore_cursor_events(true)?;

    #[cfg(target_os = "macos")]
    captures_macos_window::show_without_activating(&window)
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;

    #[cfg(not(target_os = "macos"))]
    window.show()?;

    let timer_app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(6_200));
        let handle = timer_app.clone();
        let _ = timer_app.run_on_main_thread(move || {
            if let Some(window) = handle.get_webview_window(&label) {
                let _ = window.destroy();
            }
        });
    });
    Ok(())
}

fn top_right_notice_position(app: &AppHandle, width: f64, _height: f64) -> (f64, f64) {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            let scale = monitor.scale_factor().max(1.0);
            let position = monitor.position();
            let size = monitor.size();
            let left = f64::from(position.x) / scale;
            let top = f64::from(position.y) / scale;
            let right = left + f64::from(size.width) / scale;
            (right - width - 18.0, top + 30.0)
        })
        .unwrap_or((20.0, 30.0))
}

fn bottom_center_notice_position(app: &AppHandle, width: f64, height: f64) -> (f64, f64) {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            let scale = monitor.scale_factor().max(1.0);
            let position = monitor.position();
            let size = monitor.size();
            let left = f64::from(position.x) / scale;
            let top = f64::from(position.y) / scale;
            let display_width = f64::from(size.width) / scale;
            let display_height = f64::from(size.height) / scale;
            (
                left + (display_width - width) / 2.0,
                top + display_height - height - 18.0,
            )
        })
        .unwrap_or((20.0, 30.0))
}

fn create_thumbnail_window(app: &AppHandle, visible: bool) -> Result<(), tauri::Error> {
    let (x, y, height) = thumbnail_window_geometry(app, 1);
    let window = WebviewWindowBuilder::new(
        app,
        "thumbnail",
        WebviewUrl::App("index.html?view=thumbnail".into()),
    )
    .title("Captures")
    .inner_size(THUMBNAIL_WIDTH, height)
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
    .disable_drag_drop_handler()
    .focused(false)
    .visible(false)
    .build()?;

    #[cfg(target_os = "macos")]
    captures_macos_window::configure_inactive_hover(&window)
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;

    if visible {
        show_thumbnail_window(&window);
    }
    Ok(())
}

const THUMBNAIL_WIDTH: f64 = 340.0;
const THUMBNAIL_CARD_HEIGHT: f64 = 160.0;
const THUMBNAIL_GAP: f64 = 24.0;
const THUMBNAIL_PADDING: f64 = 28.0;

fn update_thumbnail_stack(app: &AppHandle) {
    let app = app.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let state = handle.state::<Arc<AppState>>().inner().clone();
        let count = state.artifacts.lock().len();
        let suppressed = state.thumbnail_visibility.lock().is_suppressed();
        let show_mini_previews = state.settings().show_mini_previews;
        let Some(window) = handle.get_webview_window("thumbnail") else {
            if let Err(error) = create_thumbnail_window(
                &handle,
                thumbnail_stack_should_be_visible(count, suppressed, show_mini_previews),
            ) {
                eprintln!("failed to create capture thumbnail stack: {error}");
            }
            return;
        };
        if !thumbnail_stack_should_be_visible(count, suppressed, show_mini_previews) {
            let _ = window.hide();
            return;
        }
        let (x, y, desired_height) = thumbnail_window_geometry(&handle, count);
        let visible = window.is_visible().unwrap_or(false);
        // WKWebView blanks every painted card when its NSWindow shrinks. Keep
        // the taller frame on macOS, where native hit testing makes the empty
        // top space click-through. Other platforms shrink normally so an
        // invisible window area cannot block desktop clicks.
        let height = thumbnail_visible_window_height(
            desired_height,
            visible
                .then(|| thumbnail_window_logical_height(&window))
                .flatten(),
            cfg!(target_os = "macos"),
        );
        if visible {
            #[cfg(target_os = "macos")]
            if let Err(error) =
                captures_macos_window::resize_from_bottom(&window, THUMBNAIL_WIDTH, height)
            {
                eprintln!("failed to resize capture thumbnail stack: {error}");
                let _ = window.set_size(LogicalSize::new(THUMBNAIL_WIDTH, height));
                let _ = window.set_position(tauri::LogicalPosition::new(x, y));
            }

            #[cfg(not(target_os = "macos"))]
            {
                let _ = window.set_size(LogicalSize::new(THUMBNAIL_WIDTH, height));
                let _ = window.set_position(tauri::LogicalPosition::new(x, y));
            }
        } else {
            let _ = window.set_size(LogicalSize::new(THUMBNAIL_WIDTH, height));
            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
            show_thumbnail_window(&window);
        }
    });
}

fn thumbnail_stack_should_be_visible(
    count: usize,
    suppressed: bool,
    show_mini_previews: bool,
) -> bool {
    count > 0 && !suppressed && show_mini_previews
}

fn thumbnail_window_logical_height(window: &tauri::WebviewWindow) -> Option<f64> {
    let scale = window.scale_factor().ok()?.max(1.0);
    let size = window.inner_size().ok()?;
    Some(f64::from(size.height) / scale)
}

/// On macOS, keep a visible stack from shrinking because WKWebView blanks its
/// surviving cards during NSWindow recomposition. Other platforms can shrink.
fn thumbnail_visible_window_height(
    desired: f64,
    current: Option<f64>,
    preserve_current: bool,
) -> f64 {
    match (preserve_current, current) {
        (true, Some(current)) => desired.max(current),
        _ => desired,
    }
}

fn show_thumbnail_window(window: &tauri::WebviewWindow) {
    // Sleep/resume and compositor handoffs can leave the window click-through.
    // Showing always re-arms hit testing; the JS hover poll then re-applies
    // ignore-cursor for empty stack chrome within a frame.
    let _ = window.set_ignore_cursor_events(false);
    // Tauri's hide pauses the WebView lifecycle on macOS. Resume it through
    // Tauri before raising the native panel so React hover and IPC polling do
    // not remain frozen after a capture hides the stack.
    let _ = window.show();

    #[cfg(target_os = "macos")]
    if let Err(error) = captures_macos_window::show_without_activating(window) {
        eprintln!("failed to raise capture thumbnail stack: {error}");
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Re-assert topmost around show so Windows taskbar / Linux panels cannot
        // cover the stack when the two share the same topmost z-band.
        let _ = window.set_always_on_top(true);
        let _ = window.set_always_on_top(true);
    }
    let _ = window.eval("window.dispatchEvent(new Event('captures-thumbnail-resumed'))");
}

fn refresh_thumbnail_stack(app: &AppHandle) {
    update_thumbnail_stack(app);
}

fn suppress_thumbnail_capture_ui(state: &Arc<AppState>) {
    state.thumbnail_visibility.lock().suppress_for_capture_ui();
}

fn restore_thumbnail_capture_ui(app: &AppHandle, state: &Arc<AppState>) {
    state.thumbnail_visibility.lock().restore_capture_ui();
    update_thumbnail_stack(app);
}

fn restore_thumbnail_capture(app: &AppHandle, state: &Arc<AppState>, capture_generation: u64) {
    if state
        .thumbnail_visibility
        .lock()
        .restore_capture(capture_generation)
    {
        update_thumbnail_stack(app);
    }
}

fn begin_thumbnail_capture(state: &Arc<AppState>) -> Result<u64, AppError> {
    state
        .thumbnail_visibility
        .lock()
        .begin_capture()
        .ok_or(AppError::CaptureInProgress)
}

#[tauri::command]
fn thumbnail_ready(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    {
        if !state
            .thumbnail_visibility
            .lock()
            .mark_artifact_ready(&artifact_id)
        {
            return Ok(());
        }
    }
    update_thumbnail_stack(&app);
    Ok(())
}

#[tauri::command]
fn sync_thumbnail_stack(app: AppHandle) -> CommandResult<()> {
    update_thumbnail_stack(&app);
    Ok(())
}

fn thumbnail_window_geometry(app: &AppHandle, count: usize) -> (f64, f64, f64) {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            // Prefer the usable desktop (work area). Full monitor bounds include
            // reserved UI such as the Windows taskbar, macOS Dock, and Linux panels.
            let work_area = monitor.work_area();
            let full_position = *monitor.position();
            let full_size = *monitor.size();
            thumbnail_geometry(
                ThumbnailMonitorBounds {
                    work_x: work_area.position.x,
                    work_y: work_area.position.y,
                    work_width: work_area.size.width,
                    work_height: work_area.size.height,
                    full_x: full_position.x,
                    full_y: full_position.y,
                    full_width: full_size.width,
                    full_height: full_size.height,
                    scale_factor: monitor.scale_factor(),
                },
                count,
            )
        })
        .unwrap_or((20.0, 20.0, thumbnail_stack_height(count)))
}

fn thumbnail_stack_height(count: usize) -> f64 {
    let cards = count.max(1) as f64;
    THUMBNAIL_PADDING * 2.0 + cards * THUMBNAIL_CARD_HEIGHT + (cards - 1.0) * THUMBNAIL_GAP
}

/// Extra logical pixels to keep the stack clear of system chrome.
/// Applied on every platform so previews never sit flush against a dock/taskbar.
const THUMBNAIL_SYSTEM_CHROME_GAP: f64 = 12.0;

/// When the work area reaches the monitor bottom (auto-hide taskbar/dock/panel),
/// reserve this many logical pixels so revealing chrome cannot cover cards.
const THUMBNAIL_AUTO_HIDE_RESERVE: f64 = 48.0;

#[derive(Clone, Copy, Debug)]
struct ThumbnailMonitorBounds {
    work_x: i32,
    work_y: i32,
    work_width: u32,
    work_height: u32,
    full_x: i32,
    full_y: i32,
    full_width: u32,
    full_height: u32,
    scale_factor: f64,
}

fn thumbnail_geometry(bounds: ThumbnailMonitorBounds, count: usize) -> (f64, f64, f64) {
    let scale = bounds.scale_factor.max(1.0);
    let left = f64::from(bounds.work_x) / scale;
    let top = f64::from(bounds.work_y) / scale;
    let width = f64::from(bounds.work_width) / scale;
    let mut height = f64::from(bounds.work_height) / scale;

    // Auto-hide taskbars/docks leave the work area flush with the monitor's
    // bottom edge. Compare bottom edges instead of whole rectangles: macOS
    // still excludes its top menu bar, so its work area never equals the full
    // monitor even when an auto-hidden bottom Dock is unreserved.
    let work_bottom = i64::from(bounds.work_y) + i64::from(bounds.work_height);
    let full_bottom = i64::from(bounds.full_y) + i64::from(bounds.full_height);
    let work_spans_full_width =
        bounds.work_x == bounds.full_x && bounds.work_width == bounds.full_width;
    if work_bottom == full_bottom && work_spans_full_width {
        let bottom_reserve = THUMBNAIL_AUTO_HIDE_RESERVE.min((height * 0.12).max(0.0));
        height = (height - bottom_reserve).max(1.0);
    }

    // Keep a small permanent gap above the work-area bottom so cards never sit
    // flush against a visible taskbar/panel edge (padding alone is easy to miss).
    let bottom_gap = THUMBNAIL_SYSTEM_CHROME_GAP;
    let available_height = (height - bottom_gap - THUMBNAIL_PADDING).max(1.0);
    let stack_height = thumbnail_stack_height(count).min(available_height);
    // Window left sits at the work-area edge; CSS stack padding provides the
    // visual inset so the transparent frame can still reach the screen edge.
    let left_aligned = left;
    let bottom_aligned = top + height - stack_height - bottom_gap;
    (
        left_aligned.min(left + width - THUMBNAIL_WIDTH).max(left),
        bottom_aligned.max(top),
        stack_height,
    )
}

fn report_capture_error(app: &AppHandle, error: &AppError, mode: CaptureMode) {
    eprintln!("capture failed: {error}");
    #[cfg(not(target_os = "macos"))]
    let _ = mode;

    #[cfg(target_os = "macos")]
    if matches!(
        error,
        AppError::Capture(CaptureError::PermissionRequestStarted)
    ) {
        // macOS is already presenting its own permission prompt. Showing a
        // second Captures dialog here obscures that prompt and confuses setup.
        return;
    }

    #[cfg(target_os = "macos")]
    if matches!(error, AppError::Capture(CaptureError::PermissionDenied)) {
        let state = app.state::<Arc<AppState>>().inner().clone();
        if *state.screen_permission_requested_this_launch.lock() {
            let app = app.clone();
            app.dialog()
                .message(
                    "macOS requires Captures to restart before newly granted Screen Recording access becomes available. Captures can restart now and automatically retry this capture.",
                )
                .title("Captures Setup")
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "Restart & Retry".to_owned(),
                    "Not Now".to_owned(),
                ))
                .kind(MessageDialogKind::Info)
                .show(move |restart| {
                    if restart
                        && let Err(error) = restart_and_retry_capture(&app, mode)
                    {
                        show_macos_permission_recovery_error(&app, &error);
                    }
                });
            return;
        }

        let app = app.clone();
        app.dialog()
            .message(
                "This locally built Captures copy no longer matches macOS's saved Screen Recording record. Captures can reset only its own record, restart, and retry this capture. You will still need to approve Captures in System Settings; macOS does not allow apps to toggle this permission themselves.",
            )
            .title("Captures Setup")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Reset, Restart & Retry".to_owned(),
                "Not Now".to_owned(),
            ))
            .kind(MessageDialogKind::Error)
            .show(move |reset_permission| {
                if reset_permission {
                    let result = reset_macos_screen_capture_permission(&app)
                        .and_then(|()| restart_and_retry_capture(&app, mode));
                    if let Err(error) = result {
                        show_macos_permission_recovery_error(&app, &error);
                    }
                }
            });
        return;
    }

    let message = capture_error_message(error);
    app.dialog()
        .message(message)
        .title("Captures")
        .buttons(MessageDialogButtons::Ok)
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

fn report_recording_error(app: &AppHandle, error: &AppError) {
    if matches!(error, AppError::Capture(_)) {
        report_capture_error(app, error, CaptureMode::Region);
        return;
    }
    eprintln!("recording failed: {error}");
    app.dialog()
        .message(error.to_string())
        .title("Captures Recording")
        .buttons(MessageDialogButtons::Ok)
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

fn capture_error_message(error: &AppError) -> String {
    if matches!(error, AppError::Capture(CaptureError::Unsupported)) {
        #[cfg(target_os = "linux")]
        if wayland_session() {
            return "Window capture is not available on a pure Wayland session yet. Use Region or Full Screen capture, or log in to an X11 session for Window capture. Region and Full Screen capture use your desktop screenshot portal.".to_owned();
        }

        return "This capture mode is not supported on the current desktop session. Try Region capture instead.".to_owned();
    }

    #[cfg(target_os = "linux")]
    if wayland_session() && matches!(error, AppError::Capture(CaptureError::Backend(_))) {
        if !x11_display_available() {
            return "Captures cannot discover monitors in a native Wayland-only session yet. Enable or install XWayland, then retry Region or Full Screen capture.".to_owned();
        }
        return "Captures could not capture this Wayland desktop. Make sure an xdg-desktop-portal screenshot backend is installed and running, then try Region or Full Screen capture again.".to_owned();
    }

    if matches!(
        error,
        AppError::Capture(CaptureError::PermissionDenied | CaptureError::PermissionRequestStarted)
    ) {
        #[cfg(target_os = "windows")]
        return "Captures could not access the screen. Windows desktop capture does not use a separate Screen Recording permission; secure/UAC windows and protected content cannot be captured.".to_owned();

        #[cfg(not(target_os = "windows"))]
        return "Captures needs Screen Recording permission to capture your open windows. Enable it in your operating system's privacy settings, then restart Captures.".to_owned();
    }

    format!("Captures could not start the capture: {error}")
}

#[cfg(target_os = "macos")]
fn reset_macos_screen_capture_permission(app: &AppHandle) -> Result<(), AppError> {
    let status = Command::new("/usr/bin/tccutil")
        .args(["reset", "ScreenCapture", app.config().identifier.as_str()])
        .status()?;
    if !status.success() {
        return Err(AppError::Task(format!(
            "tccutil exited with status {status}"
        )));
    }

    let state = app.state::<Arc<AppState>>().inner().clone();
    {
        let mut settings = state.settings.write();
        settings.last_screen_permission_request_id = None;
        storage::save_settings(&settings)?;
    }
    *state.screen_permission_requested_this_launch.lock() = false;
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_macos_permission_recovery_error(app: &AppHandle, error: &AppError) {
    eprintln!("failed to recover Screen Recording permission: {error}");
    app.dialog()
        .message(format!(
            "Captures could not reset or restart its Screen Recording setup: {error}"
        ))
        .title("Captures Setup")
        .buttons(MessageDialogButtons::Ok)
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

#[cfg(target_os = "linux")]
fn wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("XDG_SESSION_TYPE")
            .is_some_and(|session| session.to_string_lossy().eq_ignore_ascii_case("wayland"))
}

/// Show, unminimize, and focus a document window so hover and cursor styles
/// work immediately after opening from a mini-preview Edit click.
///
/// On macOS, Tauri `set_focus` alone is not enough when the always-on-top
/// thumbnail panel just handled the click: the app must activate and the
/// editor must become key for WebKit CSS `:hover` / `cursor` to update.
pub(crate) fn reveal_and_focus_document_window(
    window: &tauri::WebviewWindow,
) -> Result<(), tauri::Error> {
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    #[cfg(target_os = "macos")]
    schedule_document_window_activation(window);
    Ok(())
}

#[cfg(target_os = "macos")]
fn schedule_document_window_activation(window: &tauri::WebviewWindow) {
    let window = window.clone();
    let app = window.app_handle().clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if let Err(error) = captures_macos_window::activate_document_window(&window) {
            eprintln!("failed to activate document window: {error}");
        }
    }) {
        eprintln!("failed to schedule document window activation: {error}");
    }
}

fn show_capture_history(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("history") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let app = app.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let result = WebviewWindowBuilder::new(
            &handle,
            "history",
            WebviewUrl::App("index.html?view=history".into()),
        )
        .title("Capture History")
        .inner_size(960.0, 680.0)
        .min_inner_size(640.0, 440.0)
        .center()
        .resizable(true)
        .background_color(Color(17, 18, 26, 255))
        .focused(false)
        .visible(false)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished
                && let Err(error) = window.show().and_then(|_| window.set_focus())
            {
                eprintln!("failed to reveal capture history window: {error}");
            }
        })
        .build();
        if let Err(error) = result {
            eprintln!("failed to show capture history window: {error}");
        }
    });
}

fn primary_app_window_priority(label: &str) -> Option<u8> {
    if matches!(
        label,
        "recording-selector" | "recording-countdown" | "screenshot-countdown"
    ) {
        return Some(0);
    }
    if label.starts_with(RECORDING_EDITOR_WINDOW_PREFIX)
        || label.starts_with(SCREENSHOT_EDITOR_WINDOW_PREFIX)
    {
        return Some(1);
    }
    if label == "history" {
        return Some(2);
    }
    if label == "preferences" || label.starts_with(VIEWER_WINDOW_PREFIX) {
        return Some(3);
    }
    (label == "recording-hud").then_some(4)
}

fn focus_or_show_primary_app_window(app: &AppHandle) {
    focus_primary_app_window(app);
}

fn focus_primary_app_window(app: &AppHandle) {
    if restore_hidden_recording_controls(app) {
        return;
    }
    let primary = app
        .webview_windows()
        .into_iter()
        .filter(|(_, window)| window.is_visible().unwrap_or(false))
        .filter_map(|(label, window)| {
            primary_app_window_priority(&label).map(|priority| (priority, window))
        })
        .min_by_key(|(priority, _)| *priority);
    if let Some((_, window)) = primary {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    } else {
        open_capture_controls(app, CaptureSelectorMode::Screenshot);
    }
}

fn restore_hidden_recording_controls(app: &AppHandle) -> bool {
    let recording_is_active = {
        let state = app.state::<Arc<AppState>>();
        recording::recording_controls_are_available(state.inner())
    };
    if recording_is_active
        && let Some(window) = app.get_webview_window("recording-hud")
        && !window.is_visible().unwrap_or(false)
    {
        hide_recording_controls_hidden_notices(app);
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return true;
    }
    false
}

#[cfg(target_os = "linux")]
fn x11_display_available() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

fn show_preferences(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("preferences") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let app = app.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let result = WebviewWindowBuilder::new(
            &handle,
            "preferences",
            WebviewUrl::App("index.html?view=preferences".into()),
        )
        .title("Captures Preferences")
        .inner_size(520.0, 480.0)
        .min_inner_size(420.0, 360.0)
        .center()
        .resizable(true)
        .background_color(Color(23, 24, 33, 255))
        .focused(false)
        .visible(false)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished
                && let Err(error) = window.show().and_then(|_| window.set_focus())
            {
                eprintln!("failed to reveal preferences window: {error}");
            }
        })
        .build();
        if let Err(error) = result {
            eprintln!("failed to show preferences window: {error}");
        }
    });
}

fn hide_window(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
    }
}

const CAPTURE_HUD_HIDE_SETTLE_MS: u64 = 40;

pub(crate) async fn hide_capture_huds_before_snapshot(app: &AppHandle) {
    set_capture_huds_protected(app, true);
    let had_visible_hud = [
        "thumbnail",
        "startup",
        "update",
        RECORDING_SAVED_NOTICE_LABEL,
    ]
    .into_iter()
    .any(|label| {
        app.get_webview_window(label)
            .is_some_and(|window| window.is_visible().unwrap_or(false))
    });
    hide_window(app, "thumbnail");
    hide_window(app, "startup");
    hide_recording_saved_notices(app);
    updates::defer_visible_notice(app);
    hide_window(app, "update");

    // Native hide/content-protection calls return before every compositor has
    // necessarily presented the new window state. Give a previously visible
    // HUD two frames to disappear before freezing the desktop background.
    if had_visible_hud {
        tokio::time::sleep(std::time::Duration::from_millis(CAPTURE_HUD_HIDE_SETTLE_MS)).await;
    }
}

fn hide_recording_saved_notices(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(RECORDING_SAVED_NOTICE_LABEL) {
        let _ = window.hide();
    }
}

fn hide_recording_controls_hidden_notices(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX) {
            let _ = window.hide();
        }
    }
}

fn set_capture_huds_protected(app: &AppHandle, protected: bool) {
    // The window server may still composite a just-hidden HUD into an
    // immediate display capture. Exclude Captures HUDs until the frozen background
    // frame has been read so they cannot reappear as pixels during fade-in.
    for (label, window) in app.webview_windows() {
        if (matches!(
            label.as_str(),
            "thumbnail" | "startup" | "update" | "recording-hud" | RECORDING_SAVED_NOTICE_LABEL
        )) && let Err(error) = window.set_content_protected(protected)
        {
            eprintln!("failed to update {label} capture protection: {error}");
        }
    }
}

fn hide_capture_overlay(app: &AppHandle) {
    // Restore the previous frontmost app while the overlay is still covering
    // the screen so open editors cannot flash above Chrome/Discord for a frame.
    #[cfg(target_os = "macos")]
    captures_macos_window::restore_frontmost_app_after_capture();
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
        let _ = window.set_cursor_icon(CursorIcon::Default);
        #[cfg(target_os = "macos")]
        if let Err(error) = captures_macos_window::reset_capture_overlay(&window) {
            eprintln!("failed to reset capture overlay: {error}");
        }
    }
}

fn resolve_asset(state: &AppState, path: &str) -> Option<Vec<u8>> {
    let mut segments = path.split('/');
    match (segments.next(), segments.next()) {
        (Some("session"), Some(id)) => Uuid::parse_str(id).ok().and_then(|id| {
            state
                .sessions
                .lock()
                .get(&id)
                .map(|session| session.snapshot_png.clone())
        }),
        (Some("artifact"), Some(id)) => state
            .artifacts
            .lock()
            .iter()
            .find(|artifact| artifact.id == id)
            .map(|artifact| artifact.preview_png.clone()),
        (Some("artifact-full"), Some(id)) => state
            .artifacts
            .lock()
            .iter()
            .find(|artifact| artifact.id == id)
            .map(|artifact| artifact.image_png.clone()),
        (Some("history-preview"), Some(id)) => {
            let available = state.history.lock().iter().any(|entry| entry.id == id);
            available
                .then(|| storage::load_history_image(id, true).ok())
                .flatten()
        }
        (Some("history-full"), Some(id)) => {
            let available = state.history.lock().iter().any(|entry| entry.id == id);
            available
                .then(|| storage::load_history_image(id, false).ok())
                .flatten()
        }
        _ => None,
    }
}

impl CaptureSession {
    fn view(&self, rect: captures_capture::PhysicalRect) -> Option<RgbaImage> {
        if rect.width == 0 || rect.height == 0 {
            return None;
        }
        let right = rect.x.checked_add(rect.width)?;
        let bottom = rect.y.checked_add(rect.height)?;
        if right > self.image.width() || bottom > self.image.height() {
            return None;
        }
        Some(
            image::imageops::crop_imm(&self.image, rect.x, rect.y, rect.width, rect.height)
                .to_image(),
        )
    }
}

/// Map native window/display geometry onto the capture buffer.
///
/// Coordinates come from the capture backend in the same units as
/// `display.width`/`height` (logical points on macOS, physical pixels on
/// Windows). Region selections from the overlay use
/// [`DisplayDescriptor::overlay_to_buffer_scale`] instead.
fn capture_buffer_scale(display: &captures_capture::DisplayDescriptor, image: &RgbaImage) -> f64 {
    let logical_w = f64::from(display.width.max(1));
    let logical_h = f64::from(display.height.max(1));
    let scale_x = f64::from(image.width()) / logical_w;
    let scale_y = f64::from(image.height()) / logical_h;
    let derived = ((scale_x + scale_y) * 0.5).max(1.0);
    // If the platform scale disagrees badly, trust the buffer dimensions.
    if (derived - display.scale_factor.max(1.0)).abs() > 0.25 {
        return derived;
    }
    display.scale_factor.max(1.0).max(derived)
}

fn crop_window_from_session(session: &CaptureSession, window_id: &str) -> Option<RgbaImage> {
    let window = session
        .windows
        .iter()
        .find(|window| window.id == window_id)?;
    let scale = capture_buffer_scale(&session.display, &session.image);
    let rect = LogicalRect {
        x: f64::from(window.x - session.display.x),
        y: f64::from(window.y - session.display.y),
        width: f64::from(window.width),
        height: f64::from(window.height),
    };
    let physical = rect.to_physical(scale, session.image.width(), session.image.height());
    let image = session.view(physical)?;
    #[cfg(target_os = "macos")]
    let image = {
        let mut image = image;
        mask_macos_window_corners(
            &mut image,
            window,
            &session.display,
            scale,
            window_visible_corner_radius(window),
        );
        image
    };
    Some(image)
}

#[cfg(target_os = "macos")]
fn window_visible_corner_radius(window: &captures_capture::WindowDescriptor) -> f64 {
    window
        .corner_radius
        .filter(|radius| radius.is_finite() && *radius >= 0.0)
        .unwrap_or_else(window_corner_radius_points)
}

/// Measure each window's visible corner radius from the freeze-frame so the
/// selector ring, dim cutout, and PNG mask share one shape.
///
/// A single OS-default radius is wrong for panels, terminals, and other apps
/// that keep tighter chrome than the current system window style. Sampling the
/// already-captured display image avoids a second per-window capture pass.
fn refine_window_chrome_from_snapshot(
    windows: &mut [captures_capture::WindowDescriptor],
    display: &captures_capture::DisplayDescriptor,
    image: &RgbaImage,
    fallback_radius: f64,
) {
    let scale = capture_buffer_scale(display, image);
    for window in windows.iter_mut() {
        if let Some(radius) = estimate_window_corner_radius_from_snapshot(
            window,
            display,
            image,
            scale,
            fallback_radius,
        ) {
            window.corner_radius = Some(radius);
        }
    }
}

fn estimate_window_corner_radius_from_snapshot(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
    image: &RgbaImage,
    scale: f64,
    fallback_radius: f64,
) -> Option<f64> {
    let scale = scale.max(1.0);
    let left = ((f64::from(window.x - display.x) * scale).round() as i64).max(0);
    let top = ((f64::from(window.y - display.y) * scale).round() as i64).max(0);
    let width = ((f64::from(window.width) * scale).round() as i64).max(1);
    let height = ((f64::from(window.height) * scale).round() as i64).max(1);
    let right = left + width;
    let bottom = top + height;
    if right > i64::from(image.width()) || bottom > i64::from(image.height()) {
        return None;
    }

    // Fullscreen-ish targets keep square display edges.
    if window.x <= display.x
        && window.y <= display.y
        && window.x + window.width as i32 >= display.x + display.width as i32
        && window.y + window.height as i32 >= display.y + display.height as i32
    {
        return Some(0.0);
    }

    let max_radius_px = ((fallback_radius * scale)
        .min(width as f64 / 2.0)
        .min(height as f64 / 2.0)
        .floor() as i64)
        .max(0);
    if max_radius_px < 2 {
        return Some(0.0);
    }

    let mut samples = Vec::with_capacity(4);
    for (corner_x, corner_y, dir_x, dir_y) in [
        (left, top, 1_i64, 1_i64),
        (right - 1, top, -1, 1),
        (left, bottom - 1, 1, -1),
        (right - 1, bottom - 1, -1, -1),
    ] {
        if let Some(radius_px) = estimate_corner_radius_px(
            image,
            corner_x,
            corner_y,
            dir_x,
            dir_y,
            max_radius_px,
            width,
            height,
        ) {
            samples.push(radius_px);
        }
    }
    if samples.is_empty() {
        return None;
    }
    // Inclusive pixel bounds make the trailing edge of a corner one pixel short
    // of the true radius. Prefer the strongest readable corner instead of the
    // median, which systematically under-reads rounded chrome.
    let best_px = *samples.iter().max().unwrap_or(&0) as f64;
    let radius_points = (best_px / scale).clamp(0.0, fallback_radius.max(0.0));
    // Prefer half-point steps so CSS border-radius stays stable on Retina.
    Some((radius_points * 2.0).round() / 2.0)
}

#[allow(clippy::too_many_arguments)]
fn estimate_corner_radius_px(
    image: &RgbaImage,
    corner_x: i64,
    corner_y: i64,
    dir_x: i64,
    dir_y: i64,
    max_radius_px: i64,
    window_width_px: i64,
    window_height_px: i64,
) -> Option<i64> {
    let outside = sample_image(image, corner_x, corner_y)?;
    // Deep interior of this corner — should land on window chrome/content.
    let inset = (max_radius_px.max(8) + 4)
        .min(window_width_px / 3)
        .min(window_height_px / 3);
    if inset < 4 {
        return None;
    }
    let inside = sample_image(image, corner_x + dir_x * inset, corner_y + dir_y * inset)?;
    // If the corner already looks like the interior, this corner is square or
    // the freeze-frame has no readable edge (e.g. same-colored neighbor).
    if pixels_similar(outside, inside, 18) {
        return Some(0);
    }

    let mut along_x = 0_i64;
    while along_x < max_radius_px {
        let x = corner_x + dir_x * along_x;
        let Some(pixel) = sample_image(image, x, corner_y) else {
            break;
        };
        if !pixels_similar(pixel, outside, 18) {
            break;
        }
        along_x += 1;
    }

    let mut along_y = 0_i64;
    while along_y < max_radius_px {
        let y = corner_y + dir_y * along_y;
        let Some(pixel) = sample_image(image, corner_x, y) else {
            break;
        };
        if !pixels_similar(pixel, outside, 18) {
            break;
        }
        along_y += 1;
    }

    // At an inclusive trailing edge the arc is one pixel short of R, so the two
    // runs can disagree. Keep the longer readable edge for this corner.
    let radius = along_x.max(along_y).clamp(0, max_radius_px);
    // Tiny runs are usually anti-alias or 1px framing, not real window chrome.
    if radius <= 1 {
        return Some(0);
    }
    Some(radius)
}

fn sample_image(image: &RgbaImage, x: i64, y: i64) -> Option<[u8; 4]> {
    if x < 0 || y < 0 {
        return None;
    }
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    if x >= image.width() || y >= image.height() {
        return None;
    }
    Some(image.get_pixel(x, y).0)
}

fn pixels_similar(left: [u8; 4], right: [u8; 4], max_channel_delta: u8) -> bool {
    left.iter()
        .zip(right.iter())
        .all(|(a, b)| a.abs_diff(*b) <= max_channel_delta)
}

#[cfg(any(target_os = "macos", test))]
fn mask_macos_window_corners(
    image: &mut RgbaImage,
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
    scale: f64,
    corner_radius_points: f64,
) {
    let window_left = i64::from(window.x);
    let window_top = i64::from(window.y);
    let window_right = window_left + i64::from(window.width);
    let window_bottom = window_top + i64::from(window.height);
    let display_left = i64::from(display.x);
    let display_top = i64::from(display.y);
    let display_right = display_left + i64::from(display.width);
    let display_bottom = display_top + i64::from(display.height);

    // A fullscreen window has square display edges. A larger, clipped window
    // also has no visible window corners within this display crop.
    if window_left <= display_left
        && window_top <= display_top
        && window_right >= display_right
        && window_bottom >= display_bottom
    {
        return;
    }

    let scale = scale.max(1.0);
    let full_width = f64::from(window.width) * scale;
    let full_height = f64::from(window.height) * scale;
    let radius = (corner_radius_points * scale)
        .min(full_width / 2.0)
        .min(full_height / 2.0);
    if radius <= 0.0 {
        return;
    }

    // Crops are clipped to the selected display. Keep coordinates relative to
    // the full window so a partially offscreen rounded corner is masked only
    // where that corner is still visible.
    let crop_offset_x = ((display_left - window_left).max(0) as f64) * scale;
    let crop_offset_y = ((display_top - window_top).max(0) as f64) * scale;
    let samples = WINDOW_CORNER_MASK_SAMPLES_PER_AXIS;
    let sample_count = samples * samples;

    for y in 0..image.height() {
        let window_y = crop_offset_y + f64::from(y);
        let near_vertical_corner = window_y < radius || window_y + 1.0 > full_height - radius;
        if !near_vertical_corner {
            continue;
        }

        for x in 0..image.width() {
            let window_x = crop_offset_x + f64::from(x);
            let near_horizontal_corner = window_x < radius || window_x + 1.0 > full_width - radius;
            if !near_horizontal_corner {
                continue;
            }

            let mut inside_samples = 0;
            for sample_y in 0..samples {
                for sample_x in 0..samples {
                    let sample_x = window_x + (f64::from(sample_x) + 0.5) / f64::from(samples);
                    let sample_y = window_y + (f64::from(sample_y) + 0.5) / f64::from(samples);
                    let center_x = sample_x.clamp(radius, full_width - radius);
                    let center_y = sample_y.clamp(radius, full_height - radius);
                    let distance_x = sample_x - center_x;
                    let distance_y = sample_y - center_y;
                    if distance_x.mul_add(distance_x, distance_y * distance_y) <= radius * radius {
                        inside_samples += 1;
                    }
                }
            }

            let mask_alpha = u8::try_from((inside_samples * 255 + sample_count / 2) / sample_count)
                .expect("corner coverage stays within one byte");
            let pixel = image.get_pixel_mut(x, y);
            if mask_alpha == 0 {
                // Do not leave pixels from windows behind the target hidden in
                // fully transparent PNG data.
                pixel.0 = [0, 0, 0, 0];
            } else {
                pixel.0[3] = pixel.0[3].min(mask_alpha);
            }
        }
    }
}

pub(crate) fn image_is_effectively_blank(image: &RgbaImage) -> bool {
    // Solid / near-solid frames from failed CGWindow captures (common black full-screen).
    let mut samples = 0u32;
    let mut matching = 0u32;
    let first = image.get_pixel(0, 0).0;
    let step_x = (image.width() / 16).max(1);
    let step_y = (image.height() / 16).max(1);
    for y in (0..image.height()).step_by(step_y as usize) {
        for x in (0..image.width()).step_by(step_x as usize) {
            samples += 1;
            let pixel = image.get_pixel(x, y).0;
            let close = pixel
                .iter()
                .zip(first.iter())
                .all(|(a, b)| a.abs_diff(*b) <= 2);
            if close {
                matching += 1;
            }
        }
    }
    samples > 0 && matching * 100 / samples >= 98
}

fn window_is_capturable(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
) -> bool {
    if window.display_id != display.id {
        return false;
    }
    if window.width < 48 || window.height < 48 {
        return false;
    }
    if captures_window_is_internal(window) {
        return false;
    }
    #[cfg(target_os = "macos")]
    if macos_window_is_capture_overlay(window) {
        return false;
    }
    #[cfg(target_os = "windows")]
    if windows_window_is_capture_overlay(window) {
        return false;
    }
    // Skip system chrome that is listed as full-screen "windows" and breaks selection.
    const EXCLUDED_APPS: &[&str] = &[
        "Dock",
        "Control Center",
        "Notification Centre",
        "Notification Center",
        "SystemUIServer",
        "Window Server",
        "Spotlight",
        "Wallpaper",
        "loginwindow",
    ];
    if window.app_name.as_deref().is_some_and(|name| {
        EXCLUDED_APPS
            .iter()
            .any(|excluded| name.eq_ignore_ascii_case(excluded))
    }) {
        return false;
    }
    true
}

#[cfg(target_os = "macos")]
fn macos_window_is_capture_overlay(window: &captures_capture::WindowDescriptor) -> bool {
    window.app_name.as_deref().is_some_and(|name| {
        let name = name.trim();
        name.eq_ignore_ascii_case("Screenshot") || name.eq_ignore_ascii_case("screencaptureui")
    })
}

fn captures_window_is_internal(window: &captures_capture::WindowDescriptor) -> bool {
    let captures_owned = window.app_name.as_deref().is_some_and(|name| {
        let name = name.trim();
        name.eq_ignore_ascii_case("Captures")
            || name.eq_ignore_ascii_case("Captures.app")
            || name.eq_ignore_ascii_case("captures.exe")
    });
    if !captures_owned {
        return false;
    }

    const INTERNAL_WINDOW_TITLES: &[&str] = &[
        "Captures",
        "Captures is running",
        "Captures Recording Controls",
        "Captures Recording Countdown",
        "Captures Update",
        "Recording saved",
    ];
    let title = window.title.trim();
    INTERNAL_WINDOW_TITLES
        .iter()
        .any(|internal| title.eq_ignore_ascii_case(internal))
}

#[cfg(any(target_os = "windows", test))]
fn windows_window_is_capture_overlay(window: &captures_capture::WindowDescriptor) -> bool {
    window
        .app_name
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case("NVIDIA App"))
        && window
            .title
            .to_ascii_lowercase()
            .starts_with("nvidia geforce overlay")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use image::{Rgba, RgbaImage};
    use tauri_plugin_global_shortcut::ShortcutState;

    #[cfg(target_os = "macos")]
    use super::macos_window_is_capture_overlay;
    use super::{
        AppError, CaptureMode, THUMBNAIL_AUTO_HIDE_RESERVE, THUMBNAIL_SYSTEM_CHROME_GAP,
        ThumbnailCursorAction, ThumbnailCursorKind, ThumbnailMonitorBounds, clipboard_fingerprint,
        display_contains_pointer, mask_macos_window_corners, parse_shortcut,
        primary_app_window_priority, refine_window_chrome_from_snapshot,
        should_activate_capture_cursor_before_reveal, should_trigger_shortcut,
        thumbnail_cursor_action, thumbnail_geometry, thumbnail_pointer_position,
        thumbnail_stack_should_be_visible, thumbnail_visible_window_height,
        track_shortcut_suppression, viewer_window_label, window_is_capturable,
        windows_window_is_capture_overlay,
    };

    fn bounds(
        work: (i32, i32, u32, u32),
        full: (i32, i32, u32, u32),
        scale_factor: f64,
    ) -> ThumbnailMonitorBounds {
        ThumbnailMonitorBounds {
            work_x: work.0,
            work_y: work.1,
            work_width: work.2,
            work_height: work.3,
            full_x: full.0,
            full_y: full.1,
            full_width: full.2,
            full_height: full.3,
            scale_factor,
        }
    }

    use captures_capture::{DisplayDescriptor, WindowDescriptor};

    #[test]
    fn region_cursor_waits_until_the_hidden_webview_is_primed() {
        assert!(!should_activate_capture_cursor_before_reveal(
            CaptureMode::Region
        ));
        assert!(should_activate_capture_cursor_before_reveal(
            CaptureMode::Window
        ));
    }

    #[test]
    fn estimates_rounded_window_chrome_from_the_freeze_frame() {
        use image::{Rgba, RgbaImage};

        let display = DisplayDescriptor {
            id: "display".to_owned(),
            name: "Display".to_owned(),
            x: 0,
            y: 0,
            width: 200,
            height: 160,
            scale_factor: 1.0,
            is_primary: true,
        };
        // Solid background with a rounded window painted on top.
        let mut image = RgbaImage::from_pixel(200, 160, Rgba([30, 30, 30, 255]));
        let window_x = 40_i32;
        let window_y = 30_i32;
        let window_w = 100_u32;
        let window_h = 80_u32;
        let radius = 12.0_f64;
        for y in 0..window_h {
            for x in 0..window_w {
                let px = f64::from(x);
                let py = f64::from(y);
                let width = f64::from(window_w);
                let height = f64::from(window_h);
                let cx = px.clamp(radius, width - radius);
                let cy = py.clamp(radius, height - radius);
                let dx = px - cx;
                let dy = py - cy;
                if dx * dx + dy * dy <= radius * radius {
                    image.put_pixel(
                        (window_x as u32) + x,
                        (window_y as u32) + y,
                        Rgba([200, 210, 220, 255]),
                    );
                }
            }
        }
        let mut window = WindowDescriptor {
            id: "window".to_owned(),
            title: "Rounded".to_owned(),
            app_name: Some("App".to_owned()),
            z_order: 1,
            x: window_x,
            y: window_y,
            width: window_w,
            height: window_h,
            display_id: display.id.clone(),
            corner_radius: None,
        };

        refine_window_chrome_from_snapshot(
            std::slice::from_mut(&mut window),
            &display,
            &image,
            25.0,
        );

        let measured = window
            .corner_radius
            .expect("corner radius should be measured");
        assert!(
            (measured - radius).abs() <= 2.0,
            "expected ~{radius}pt, got {measured}"
        );
    }

    #[test]
    fn treats_fullscreen_freeze_frame_windows_as_square() {
        use image::{Rgba, RgbaImage};

        let display = DisplayDescriptor {
            id: "display".to_owned(),
            name: "Display".to_owned(),
            x: 0,
            y: 0,
            width: 80,
            height: 60,
            scale_factor: 1.0,
            is_primary: true,
        };
        let image = RgbaImage::from_pixel(80, 60, Rgba([10, 20, 30, 255]));
        let mut window = WindowDescriptor {
            id: "fullscreen".to_owned(),
            title: "Full".to_owned(),
            app_name: Some("App".to_owned()),
            z_order: 1,
            x: 0,
            y: 0,
            width: 80,
            height: 60,
            display_id: display.id.clone(),
            corner_radius: None,
        };

        refine_window_chrome_from_snapshot(
            std::slice::from_mut(&mut window),
            &display,
            &image,
            25.0,
        );
        assert_eq!(window.corner_radius, Some(0.0));
    }

    #[test]
    fn masks_background_pixels_outside_macos_window_corners() {
        let display = DisplayDescriptor {
            id: "display".to_owned(),
            name: "Display".to_owned(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            scale_factor: 2.0,
            is_primary: true,
        };
        let window = WindowDescriptor {
            id: "window".to_owned(),
            title: "Window".to_owned(),
            app_name: Some("App".to_owned()),
            z_order: 1,
            x: 10,
            y: 10,
            width: 50,
            height: 40,
            display_id: display.id.clone(),
            corner_radius: None,
        };
        let mut image = RgbaImage::from_pixel(100, 80, Rgba([12, 34, 56, 255]));

        mask_macos_window_corners(&mut image, &window, &display, 2.0, 10.0);

        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(99, 0).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(0, 79).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(99, 79).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(50, 0).0, [12, 34, 56, 255]);
        assert_eq!(image.get_pixel(50, 40).0, [12, 34, 56, 255]);
        assert!(
            image
                .pixels()
                .any(|pixel| pixel.0[3] > 0 && pixel.0[3] < 255),
            "rounded edges should retain antialiased alpha"
        );
    }

    #[test]
    fn masks_only_window_corners_that_remain_inside_the_display_crop() {
        let display = DisplayDescriptor {
            id: "display".to_owned(),
            name: "Display".to_owned(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            scale_factor: 2.0,
            is_primary: true,
        };
        let mut window = WindowDescriptor {
            id: "window".to_owned(),
            title: "Window".to_owned(),
            app_name: Some("App".to_owned()),
            z_order: 1,
            x: -12,
            y: 10,
            width: 50,
            height: 40,
            display_id: display.id.clone(),
            corner_radius: None,
        };
        let mut clipped = RgbaImage::from_pixel(76, 80, Rgba([12, 34, 56, 255]));

        mask_macos_window_corners(&mut clipped, &window, &display, 2.0, 10.0);

        assert_eq!(clipped.get_pixel(0, 0).0[3], 255);
        assert_eq!(clipped.get_pixel(75, 0).0[3], 0);

        window.x = 0;
        window.y = 0;
        window.width = display.width;
        window.height = display.height;
        let mut fullscreen = RgbaImage::from_pixel(200, 200, Rgba([12, 34, 56, 255]));

        mask_macos_window_corners(&mut fullscreen, &window, &display, 2.0, 10.0);

        assert_eq!(fullscreen.get_pixel(0, 0).0[3], 255);
        assert_eq!(fullscreen.get_pixel(199, 199).0[3], 255);
    }

    #[test]
    fn excludes_nvidia_capture_overlays_from_window_selection() {
        let overlay = WindowDescriptor {
            id: "overlay".to_owned(),
            title: "NVIDIA GeForce Overlay DT".to_owned(),
            app_name: Some("NVIDIA App".to_owned()),
            z_order: 1,
            x: 0,
            y: 0,
            width: 3_840,
            height: 2_160,
            display_id: "display".to_owned(),
            corner_radius: None,
        };
        assert!(windows_window_is_capture_overlay(&overlay));

        let mut app = overlay.clone();
        app.title = "NVIDIA App".to_owned();
        assert!(!windows_window_is_capture_overlay(&app));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn excludes_macos_screenshot_overlay_from_window_selection() {
        let display = DisplayDescriptor {
            id: "display".to_owned(),
            name: "Display".to_owned(),
            x: 0,
            y: 0,
            width: 1_728,
            height: 1_117,
            scale_factor: 2.0,
            is_primary: true,
        };
        let overlay = WindowDescriptor {
            id: "overlay".to_owned(),
            title: String::new(),
            app_name: Some("Screenshot".to_owned()),
            z_order: 1,
            x: 0,
            y: 0,
            width: 1_728,
            height: 1_117,
            display_id: "display".to_owned(),
            corner_radius: None,
        };
        assert!(macos_window_is_capture_overlay(&overlay));
        assert!(!window_is_capturable(&overlay, &display));

        let mut normal_window = overlay;
        normal_window.app_name = Some("Preview".to_owned());
        assert!(!macos_window_is_capture_overlay(&normal_window));
        assert!(window_is_capturable(&normal_window, &display));
    }

    #[test]
    fn includes_user_facing_captures_windows_but_excludes_capture_chrome() {
        let display = DisplayDescriptor {
            id: "display".to_owned(),
            name: "Display".to_owned(),
            x: 0,
            y: 0,
            width: 1_440,
            height: 900,
            scale_factor: 2.0,
            is_primary: true,
        };
        let captures_window = |title: &str| WindowDescriptor {
            id: title.to_owned(),
            title: title.to_owned(),
            app_name: Some("Captures.app".to_owned()),
            z_order: 1,
            x: 80,
            y: 80,
            width: 640,
            height: 480,
            display_id: display.id.clone(),
            corner_radius: None,
        };

        for title in [
            "Captures Preferences",
            "Capture History",
            "Captures Preview",
            "Captures Editor",
        ] {
            assert!(
                window_is_capturable(&captures_window(title), &display),
                "{title} should be available for self-capture"
            );
        }
        for title in [
            "Captures",
            "Captures is running",
            "Captures Recording Controls",
            "Captures Recording Countdown",
            "Captures Update",
            "Recording saved",
        ] {
            assert!(
                !window_is_capturable(&captures_window(title), &display),
                "{title} should stay out of capture targets"
            );
        }

        let mut other_app = captures_window("Captures");
        other_app.app_name = Some("Browser".to_owned());
        assert!(window_is_capturable(&other_app, &display));
    }

    #[test]
    fn gives_each_artifact_a_stable_viewer_window() {
        assert_eq!(viewer_window_label("first"), "viewer-first");
        assert_eq!(viewer_window_label("second"), "viewer-second");
        assert_ne!(viewer_window_label("first"), viewer_window_label("second"));
    }

    #[test]
    fn viewer_windows_can_complete_close_requests() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/viewer.json"))
                .expect("viewer capability should be valid JSON");
        let windows = capability["windows"]
            .as_array()
            .expect("viewer capability should target windows");
        let permissions = capability["permissions"]
            .as_array()
            .expect("viewer capability should grant permissions");

        assert!(windows.iter().any(|window| window == "viewer-*"));
        for permission in ["core:window:allow-close", "core:window:allow-destroy"] {
            assert!(
                permissions.iter().any(|granted| granted == permission),
                "viewer capability should grant {permission}"
            );
        }
    }

    #[test]
    fn ignores_preview_cursor_updates_while_capture_is_active() {
        assert_eq!(
            thumbnail_cursor_action(true, false, ThumbnailCursorKind::Default),
            ThumbnailCursorAction::Ignore
        );
        assert_eq!(
            thumbnail_cursor_action(true, true, ThumbnailCursorKind::Pointer),
            ThumbnailCursorAction::Ignore
        );
        assert_eq!(
            thumbnail_cursor_action(false, true, ThumbnailCursorKind::Pointer),
            ThumbnailCursorAction::Apply(ThumbnailCursorKind::Pointer)
        );
        assert_eq!(
            thumbnail_cursor_action(false, true, ThumbnailCursorKind::Grab),
            ThumbnailCursorAction::Apply(ThumbnailCursorKind::Grab)
        );
    }

    #[test]
    fn treats_legacy_and_recorded_shortcut_formats_as_the_same_combination() {
        assert_eq!(
            parse_shortcut("Ctrl+Shift+4").expect("legacy shortcut should parse"),
            parse_shortcut("Control+Shift+Digit4").expect("recorded shortcut should parse")
        );
        assert!(parse_shortcut("Ctrl+Shift+Space").is_ok());
    }

    #[test]
    fn triggers_shortcuts_once_after_the_keys_are_released() {
        let armed = AtomicBool::new(false);

        assert!(!should_trigger_shortcut(&armed, ShortcutState::Pressed));
        assert!(!should_trigger_shortcut(&armed, ShortcutState::Pressed));
        assert!(should_trigger_shortcut(&armed, ShortcutState::Released));
        assert!(!should_trigger_shortcut(&armed, ShortcutState::Released));
        assert!(!should_trigger_shortcut(&armed, ShortcutState::Pressed));
        assert!(should_trigger_shortcut(&armed, ShortcutState::Released));
    }

    #[test]
    fn keeps_a_shortcut_suppressed_until_its_keys_are_released() {
        let suppressed_while_pressed = AtomicBool::new(false);

        assert!(track_shortcut_suppression(
            &suppressed_while_pressed,
            ShortcutState::Pressed,
            true,
        ));
        assert!(track_shortcut_suppression(
            &suppressed_while_pressed,
            ShortcutState::Released,
            false,
        ));
        assert!(!track_shortcut_suppression(
            &suppressed_while_pressed,
            ShortcutState::Pressed,
            false,
        ));
        assert!(!track_shortcut_suppression(
            &suppressed_while_pressed,
            ShortcutState::Released,
            false,
        ));
    }

    #[test]
    fn stacks_thumbnails_upward_in_logical_pixels_on_retina_displays() {
        // Work area already excludes dock/taskbar; full bounds differ so no
        // auto-hide reserve is applied. Extra system-chrome gap lifts the stack.
        assert_eq!(
            thumbnail_geometry(bounds((0, 0, 3_992, 2_048), (0, 0, 3_992, 2_160), 2.0), 1),
            (0.0, 796.0, 216.0)
        );
        assert_eq!(
            thumbnail_geometry(
                bounds((-3_840, 0, 3_840, 2_048), (-3_840, 0, 3_840, 2_160), 2.0),
                2
            ),
            (-1_920.0, 612.0, 400.0)
        );
    }

    #[test]
    fn keeps_the_thumbnail_stack_inside_the_monitor_work_area() {
        // 1920×1040 work area on a 1920×1080 display (48px taskbar).
        let (_, top, height) =
            thumbnail_geometry(bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0), 1);

        // Window bottom sits system-chrome gap above the work-area bottom.
        assert_eq!(top + height, 1_040.0 - THUMBNAIL_SYSTEM_CHROME_GAP);
        assert!(top + height < 1_040.0);
    }

    #[test]
    fn reserves_space_when_work_area_matches_full_monitor_auto_hide() {
        // Auto-hide taskbar: work area == full 1920×1080 monitor.
        let (_, top, height) =
            thumbnail_geometry(bounds((0, 0, 1_920, 1_080), (0, 0, 1_920, 1_080), 1.0), 1);

        let window_bottom = top + height;
        // Must clear both the auto-hide reserve and the permanent chrome gap.
        assert!(window_bottom <= 1_080.0 - THUMBNAIL_AUTO_HIDE_RESERVE);
        assert_eq!(
            window_bottom,
            1_080.0 - THUMBNAIL_AUTO_HIDE_RESERVE - THUMBNAIL_SYSTEM_CHROME_GAP
        );
    }

    #[test]
    fn reserves_bottom_space_when_top_system_chrome_remains_visible() {
        // macOS keeps the menu bar out of the work area even when an
        // auto-hidden bottom Dock is not reserved. Linux can report the same
        // shape for a top panel plus auto-hidden bottom panel.
        let (_, top, height) =
            thumbnail_geometry(bounds((0, 48, 3_992, 2_112), (0, 0, 3_992, 2_160), 2.0), 1);

        assert_eq!(
            top + height,
            1_080.0 - THUMBNAIL_AUTO_HIDE_RESERVE - THUMBNAIL_SYSTEM_CHROME_GAP
        );
    }

    #[test]
    fn keeps_visible_thumbnail_window_from_shrinking_after_dismiss() {
        assert_eq!(
            thumbnail_visible_window_height(400.0, Some(584.0), true),
            584.0
        );
        assert_eq!(
            thumbnail_visible_window_height(584.0, Some(400.0), true),
            584.0
        );
        assert_eq!(thumbnail_visible_window_height(216.0, None, true), 216.0);
    }

    #[test]
    fn keeps_mini_previews_hidden_when_the_preference_is_disabled() {
        assert!(thumbnail_stack_should_be_visible(1, false, true));
        assert!(!thumbnail_stack_should_be_visible(1, true, true));
        assert!(!thumbnail_stack_should_be_visible(1, false, false));
        assert!(!thumbnail_stack_should_be_visible(0, false, true));
    }

    #[test]
    fn shrinks_non_macos_thumbnail_windows_to_avoid_invisible_click_blockers() {
        assert_eq!(
            thumbnail_visible_window_height(400.0, Some(584.0), false),
            400.0
        );
    }

    #[test]
    fn maps_global_pointer_into_retina_thumbnail_coordinates() {
        let pointer = thumbnail_pointer_position(40.0, 80.0, 48, 120, 600, 352, 2.0);
        assert_eq!(pointer.x, 16.0);
        assert_eq!(pointer.y, 20.0);
        assert!(pointer.inside);

        let outside = thumbnail_pointer_position(10.0, 10.0, 48, 120, 600, 352, 2.0);
        assert!(!outside.inside);
    }

    #[test]
    fn linux_pointer_coordinates_are_scaled_before_monitor_matching() {
        let display = DisplayDescriptor {
            id: "second".to_owned(),
            name: "Second".to_owned(),
            x: 1_920,
            y: 0,
            width: 1_920,
            height: 1_080,
            scale_factor: 2.0,
            is_primary: false,
        };

        assert!(display_contains_pointer(&display, 4_400, 800, 2.0));
        assert!(!display_contains_pointer(&display, 1_000, 800, 2.0));
    }

    #[test]
    fn clipboard_fingerprints_include_dimensions_and_pixels() {
        let original = clipboard_fingerprint(1, 1, &[1, 2, 3, 255]);
        assert_eq!(original, clipboard_fingerprint(1, 1, &[1, 2, 3, 255]));
        assert_ne!(original, clipboard_fingerprint(2, 1, &[1, 2, 3, 255]));
        assert_ne!(original, clipboard_fingerprint(1, 1, &[1, 2, 4, 255]));
    }

    #[test]
    fn background_task_errors_only_show_the_actionable_message() {
        assert_eq!(
            AppError::Task("the encoder stopped unexpectedly".to_owned()).to_string(),
            "the encoder stopped unexpectedly"
        );
    }

    #[test]
    fn app_reopen_prefers_editors_over_utility_windows() {
        assert_eq!(primary_app_window_priority("recording-editor-abc"), Some(1));
        assert_eq!(
            primary_app_window_priority("screenshot-editor-abc"),
            Some(1)
        );
        assert_eq!(primary_app_window_priority("history"), Some(2));
        assert_eq!(primary_app_window_priority("recording-hud"), Some(4));
        assert_eq!(primary_app_window_priority("thumbnail"), None);
    }

    #[test]
    fn screen_rect_hit_test_is_half_open_on_right_and_bottom() {
        assert!(super::screen_rect_contains_point(
            100.0, 200.0, 400.0, 300.0, 100.0, 200.0
        ));
        assert!(super::screen_rect_contains_point(
            100.0, 200.0, 400.0, 300.0, 499.0, 499.0
        ));
        assert!(!super::screen_rect_contains_point(
            100.0, 200.0, 400.0, 300.0, 500.0, 350.0
        ));
        assert!(!super::screen_rect_contains_point(
            100.0, 200.0, 400.0, 300.0, 250.0, 500.0
        ));
        assert!(!super::screen_rect_contains_point(
            100.0, 200.0, 400.0, 300.0, 99.0, 250.0
        ));
    }
}
