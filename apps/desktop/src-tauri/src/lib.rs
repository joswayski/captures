#![deny(unsafe_code)]

use std::process::Command;
#[cfg(not(target_os = "macos"))]
use std::sync::atomic::AtomicIsize;
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tauri::CursorIcon;

use captures_capture::{
    CaptureError, CaptureMode, DisplayFrame, LogicalRect, PhysicalRect, WindowDescriptor,
};
use chrono::{DateTime, Utc};
use image::RgbaImage;
use mouse_position::mouse_position::Mouse;
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, Theme, WebviewUrl, WebviewWindowBuilder,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
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

mod crash_report;
mod feedback;
mod models;
mod recording;
mod screenshot_editor;
mod session_end;
mod state;
mod storage;
mod updates;

use models::{
    ActiveSession, AppSettings, Appearance, ArtifactKind, ArtifactSummary, CaptureArtifact,
    CaptureSelectorMode, CaptureSession, ClipboardCopyStatus, ClipboardState,
    HISTORY_RETENTION_DAYS, HistoryEntry, MiniPreviewPlacement,
};
use screenshot_editor::SCREENSHOT_EDITOR_WINDOW_PREFIX;
use state::{AppState, ClipboardFingerprint, ThumbnailStackAnchor, ThumbnailStackOrigin};

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
const TRAY_ICON_ID: &str = "main";
const ONBOARDING_WINDOW_LABEL: &str = "onboarding";
const RECORDING_EDITOR_WINDOW_PREFIX: &str = "recording-editor-";
const RECORDING_SAVED_NOTICE_LABEL: &str = "recording-saved";
const RECORDING_SAVED_NOTICE_EVENT: &str = "recording-saved-artifact";
const RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX: &str = "recording-controls-hidden-";
const PREFERENCES_TARGET_EVENT: &str = "preferences-target";
const AUTO_START_PREFERENCE_TARGET: &str = "auto-start-on-selection";
const RECORDING_CONTROLS_PREFERENCE_TARGET: &str = "include-recording-controls-in-captures";
/// Mini-preview stack listens for this to clear “In editor” when a window dies.
const EDITOR_LAYERS_CHANGED_EVENT: &str = "editor-layers-changed";
#[cfg(any(target_os = "macos", test))]
const WINDOW_CORNER_MASK_SAMPLES_PER_AXIS: u32 = 4;

/// Payload for `editor-layers-changed` (matches the frontend `EditorLayerPresence`).
#[derive(Clone, Debug, Serialize)]
struct EditorLayerPresenceEvent {
    editor_id: String,
    artifact_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct OnboardingState {
    platform: String,
    screen_recording_required: bool,
    screen_recording_granted: bool,
    screen_recording_can_request: bool,
    screen_recording_requested_this_launch: bool,
    capture_system_audio: bool,
    microphone_enabled: bool,
    microphone_granted: bool,
    microphone_can_request: bool,
}

struct ClipboardWrite {
    revision: isize,
    fingerprint: ClipboardFingerprint,
}

pub fn run() {
    crash_report::install_panic_hook();
    session_end::install();
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
            match event {
                // Clear mini-preview “In editor” before the webview is torn down.
                // React unmount emit is best-effort; native close often skips it.
                tauri::WindowEvent::CloseRequested { .. } => {
                    clear_editor_layer_presence_for_window(window);
                    let Some(artifact_id) =
                        window.label().strip_prefix(RECORDING_EDITOR_WINDOW_PREFIX)
                    else {
                        return;
                    };
                    if let Err(error) =
                        show_recording_saved_notice(window.app_handle(), artifact_id)
                    {
                        eprintln!("failed to show recording saved notice: {error}");
                    }
                }
                // Belt-and-suspenders if CloseRequested was prevented or skipped.
                tauri::WindowEvent::Destroyed => {
                    clear_editor_layer_presence_for_window(window);
                    if let Some(owner_id) =
                        window.label().strip_prefix(SCREENSHOT_EDITOR_WINDOW_PREFIX)
                        && let Some(state) = window.app_handle().try_state::<Arc<AppState>>()
                    {
                        state.drop_editor_artifacts_for_owner(owner_id);
                    }
                    if is_editor_window_label(window.label())
                        || window.label() == "viewer"
                        || window.label().starts_with(VIEWER_WINDOW_PREFIX)
                    {
                        updates::restore_update_notice(window.app_handle());
                    }
                }
                #[cfg(target_os = "macos")]
                tauri::WindowEvent::Focused(focused) if window.label() == "update" => {
                    captures_macos_window::update_notice_focus_changed(*focused);
                }
                _ => {}
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
                    .header("Content-Type", storage::overlay_snapshot_mime_type(&bytes))
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
            commit_display,
            cancel_capture,
            cancel_active_capture,
            dismiss_capture_surface,
            cancel_screenshot_countdown,
            get_screenshot_countdown,
            get_active_session,
            get_pending_session,
            get_settings,
            get_onboarding_state,
            request_onboarding_screen_permission,
            set_onboarding_desktop_audio,
            set_onboarding_microphone,
            request_onboarding_microphone_permission,
            restart_captures_for_permissions,
            complete_onboarding,
            set_shortcut_capture_suppressed,
            update_settings,
            get_artifacts,
            get_artifact,
            prepare_artifact_drag,
            mark_internal_file_drop,
            preview_file_drop_landing,
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
            dismiss_all_artifacts,
            open_artifact_viewer,
            screenshot_editor::open_screenshot_editor,
            screenshot_editor::default_screenshot_edit_path,
            screenshot_editor::copy_screenshot_edit,
            screenshot_editor::estimate_screenshot_export,
            screenshot_editor::preview_screenshot_export,
            screenshot_editor::save_screenshot_edit,
            screenshot_editor::save_screenshot_editor_draft,
            screenshot_editor::load_screenshot_editor_draft,
            screenshot_editor::discard_screenshot_editor_draft,
            show_capture_overlay,
            reveal_capture_overlay,
            sync_capture_cursor,
            thumbnail_ready,
            sync_thumbnail_stack,
            set_mini_previews_collapsed,
            set_mini_preview_stack_position,
            get_thumbnail_pointer_position,
            get_capture_pointer_position,
            thumbnail_pointer_poll_available,
            set_thumbnail_cursor,
            reassert_thumbnail_cursor,
            set_thumbnail_ignore_cursor_events,
            refresh_thumbnail_interactivity,
            open_captures_folder,
            open_capture_history,
            open_preferences,
            open_system_screenshot_shortcut_settings,
            feedback::open_feedback,
            feedback::get_feedback_context,
            feedback::submit_feedback,
            dismiss_recording_saved_notice,
            updates::get_update_status,
            updates::check_for_updates,
            updates::install_update,
            updates::dismiss_update_notice,
            updates::open_update_download_page,
            updates::open_update_changelog_url,
            recording::prepare_recording,
            recording::get_recording_selection,
            recording::select_capture_display,
            recording::show_recording_selector,
            recording::reveal_recording_selector,
            recording::sync_selector_cursor,
            recording::cancel_recording_selection,
            recording::capture_selection_screenshot,
            recording::list_recording_audio_devices,
            recording::recording_controls_are_excluded,
            recording::platform_can_exclude_recording_controls,
            recording::get_recording_snapshot,
            recording::start_recording,
            recording::reveal_recording_region_indicator,
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
            recording::estimate_recording_export,
            recording::preview_recording_export,
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
            register_shortcuts(&handle);
            install_capture_escape_cancel(&handle);
            if let Err(error) = create_overlay_window(&handle) {
                eprintln!("failed to prepare capture overlay: {error}");
            }
            if let Err(error) = recording::ensure_recording_selector_window(&handle) {
                eprintln!("failed to prepare capture menu: {error}");
            }
            if let Err(error) = create_thumbnail_window(&handle, false) {
                eprintln!("failed to prepare capture thumbnail: {error}");
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
            let restarted_after_update = updates::take_update_restart_pending();
            crash_report::initialize(&handle, restarted_after_update);
            if pending_capture.is_none() {
                let onboarding_completed =
                    app.state::<Arc<AppState>>().settings().onboarding_completed;
                match interactive_launch_action(
                    onboarding_completed,
                    restarted_after_update || launched_from_autostart(),
                ) {
                    InteractiveLaunchAction::Onboarding => show_onboarding(&handle),
                    InteractiveLaunchAction::StartupNotice => {
                        show_startup_notice(&handle, STARTUP_NOTICE_AUTOSTART_VISIBLE);
                    }
                    InteractiveLaunchAction::Preferences => show_preferences(&handle),
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
            tauri::RunEvent::ExitRequested { code, api, .. } => {
                if crash_report::should_prevent_exit(code) {
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::Exit => crash_report::mark_clean_exit(),
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => focus_or_show_primary_app_window(_app),
            _ => {}
        });
}

fn launched_from_autostart() -> bool {
    std::env::args_os().any(|argument| argument == std::ffi::OsStr::new(AUTOSTART_ARG))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteractiveLaunchAction {
    Onboarding,
    StartupNotice,
    Preferences,
}

/// First interactive launch opens Preferences, not a capture overlay.
/// Autostart and post-update restarts stay in the tray with the startup notice.
fn interactive_launch_action(
    onboarding_completed: bool,
    launched_quietly: bool,
) -> InteractiveLaunchAction {
    if !onboarding_completed {
        InteractiveLaunchAction::Onboarding
    } else if launched_quietly {
        InteractiveLaunchAction::StartupNotice
    } else {
        InteractiveLaunchAction::Preferences
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppReactivation {
    ShowOnboarding,
    RestoreRecordingControls,
    FocusExisting,
    ShowPreferences,
}

/// Start, Search, Dock, or a second instance should surface a durable window.
/// Capture overlays stay bound to shortcuts and the tray New Capture item.
fn app_reactivation(
    onboarding_completed: bool,
    restore_recording_controls: bool,
    has_visible_primary_window: bool,
) -> AppReactivation {
    if !onboarding_completed {
        AppReactivation::ShowOnboarding
    } else if restore_recording_controls {
        AppReactivation::RestoreRecordingControls
    } else if has_visible_primary_window {
        AppReactivation::FocusExisting
    } else {
        AppReactivation::ShowPreferences
    }
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
    open_capture_controls_with_target(app, initial_mode, CaptureMode::Region);
}

fn open_capture_controls_with_target(
    app: &AppHandle,
    initial_mode: CaptureSelectorMode,
    initial_target: CaptureMode,
) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if !state.settings().onboarding_completed {
        show_onboarding(app);
        return;
    }
    if restore_hidden_recording_controls(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = recording::prepare_capture_selector_inner(
            app.clone(),
            state,
            initial_mode,
            initial_target,
        )
        .await
        {
            abort_prefetched_freeze_capture(&app);
            if matches!(&error, AppError::ScreenshotCancelled) {
                return;
            }
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
    if let Err(error) = ensure_capture_session_available() {
        release_claimed_region_capture_cursor();
        abort_prefetched_freeze_capture(&app);
        disarm_capture_escape_intent(&app);
        return Err(error);
    }
    if updates::install_is_active(&app) {
        release_claimed_region_capture_cursor();
        abort_prefetched_freeze_capture(&app);
        disarm_capture_escape_intent(&app);
        return Err(AppError::UpdateInstalling);
    }
    if recording::screenshot_capture_is_blocked(&state) || screenshot_countdown_is_active(&state) {
        abort_prefetched_freeze_capture(&app);
        disarm_capture_escape_intent(&app);
        return Err(AppError::CaptureInProgress);
    }
    let recapture_ui = should_recapture_visible_capture_ui(&app, &state, mode);
    let overlay_visible = capture_overlay_is_visible(&app);
    // Screenshot shortcuts must work while the capture menu is open. Stay on
    // that overlay and switch target instead of tearing it down — dismissing
    // revealed Preferences and other documents, then recaptured a ghost of the
    // menu into the freeze-frame. Pressing the same screenshot shortcut again
    // freezes the menu or region overlay into a new snapshot instead.
    if !recapture_ui && recording::switch_open_capture_selector_to_screenshot(&app, &state, mode) {
        abort_prefetched_freeze_capture(&app);
        disarm_capture_escape_intent(&app);
        return Ok(None);
    }
    if recapture_ui {
        include_capture_ui_in_snapshot(&app);
        if !freeze_prefetch_is_pending() {
            tokio::time::sleep(std::time::Duration::from_millis(CAPTURE_HUD_HIDE_SETTLE_MS)).await;
        }
    }
    let flow = adopt_or_begin_capture_flow(&app);
    // A failed overlay (image never loaded, webview stuck, etc.) leaves a session
    // behind. CaptureInProgress was silent on the shortcut path, so region mode
    // appeared completely dead until restart. Drop the stale session and retry.
    // When recapturing the visible overlay, keep the window up so its chrome is
    // in the freeze-frame.
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
        if !(recapture_ui && overlay_visible) {
            hide_capture_overlay(&app);
        }
    }
    let thumbnail_capture_generation = match begin_thumbnail_capture(&state) {
        Ok(generation) => generation,
        Err(error) => {
            abort_prefetched_freeze_capture(&app);
            disarm_capture_escape_intent(&app);
            hide_capture_overlay(&app);
            return Err(error);
        }
    };
    hide_capture_huds_before_snapshot(&app).await;

    if capture_flow_was_cancelled(flow) {
        abort_cancelled_prepare(&app, &state, thumbnail_capture_generation);
        return Ok(None);
    }

    let result = prepare_capture(
        app.clone(),
        state.clone(),
        mode,
        thumbnail_capture_generation,
        flow,
    )
    .await;
    restore_capture_ui_snapshot_exclusion(&app);
    if recapture_ui && overlay_visible && matches!(&result, Ok(None)) {
        hide_capture_overlay(&app);
    }
    if matches!(&result, Ok(None)) && capture_flow_was_cancelled(flow) {
        abort_cancelled_prepare(&app, &state, thumbnail_capture_generation);
    }
    if result.is_err() {
        abort_prefetched_freeze_capture(&app);
        set_capture_huds_protected(&app, false);
        state.sessions.lock().clear();
        hide_capture_overlay(&app);
        restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
        reveal_document_windows_after_capture(&app);
        updates::restore_update_notice(&app);
        disarm_capture_escape_intent(&app);
    }
    result
}

fn abort_cancelled_prepare(
    app: &AppHandle,
    state: &Arc<AppState>,
    thumbnail_capture_generation: u64,
) {
    abort_prefetched_freeze_capture(app);
    set_capture_huds_protected(app, false);
    let leftover: Vec<u64> = state
        .sessions
        .lock()
        .drain()
        .map(|(_, session)| session.thumbnail_capture_generation)
        .collect();
    for generation in leftover {
        restore_thumbnail_capture(app, state, generation);
    }
    restore_thumbnail_capture(app, state, thumbnail_capture_generation);
    release_claimed_region_capture_cursor();
    hide_capture_overlay(app);
    restore_capture_ui_snapshot_exclusion(app);
    reveal_document_windows_after_capture(app);
    updates::restore_update_notice(app);
    disarm_capture_escape_intent(app);
}

async fn prepare_capture(
    app: AppHandle,
    state: Arc<AppState>,
    mode: CaptureMode,
    thumbnail_capture_generation: u64,
    flow: u64,
) -> Result<Option<ActiveSession>, AppError> {
    let request_permission = mark_screen_permission_request(&state)?;
    if let Err(error) = state.backend.ensure_permission(request_permission) {
        if matches!(&error, CaptureError::PermissionRequestStarted) {
            *state.screen_permission_requested_this_launch.lock() = true;
        }
        return Err(error.into());
    }
    if mode == CaptureMode::Display {
        let includes_capture_ui = capture_ui_is_visible(&app);
        if includes_capture_ui {
            include_capture_ui_in_snapshot(&app);
        }
        let display = display_under_pointer(&state)?;
        let countdown_seconds = screenshot_countdown_seconds_for_capture_ui(
            state.settings().screenshot_countdown_seconds,
            includes_capture_ui,
        );
        if countdown_seconds > 0
            && !run_screenshot_countdown(
                app.clone(),
                state.clone(),
                &display,
                countdown_seconds,
                thumbnail_capture_generation,
            )
            .await?
        {
            // Cancel already restored the stack, cleared HUD protection, and
            // re-showed any capture-concealed document windows.
            disarm_capture_escape_intent(&app);
            return Ok(None);
        }
        if capture_flow_was_cancelled(flow) {
            return Ok(None);
        }
        ensure_capture_session_available()?;
        let (pointer, mut frame) = if countdown_seconds > 0 {
            discard_prefetched_freeze_frame();
            let pointer = pointer_position();
            (pointer, state.backend.capture_display(&display.id)?)
        } else {
            take_prefetched_freeze_matching_display(&state, &display.id)?
        };
        apply_screenshot_cursor(
            &mut frame.image,
            &frame.descriptor,
            pointer,
            state.settings().show_cursor_in_screenshots,
        );
        if capture_flow_was_cancelled(flow) {
            return Ok(None);
        }
        set_capture_huds_protected(&app, false);
        if includes_capture_ui {
            recording::dismiss_capture_menu_after_nested_snapshot(&app, &state);
        }
        let _ = finish_capture(
            &app,
            &state,
            mode,
            frame.image,
            thumbnail_capture_generation,
        )
        .await?;
        disarm_capture_escape_intent(&app);
        return Ok(None);
    }

    ensure_capture_session_available()?;
    let includes_capture_ui = capture_ui_is_visible(&app);
    if includes_capture_ui {
        include_capture_ui_in_snapshot(&app);
    }
    let freeze_screen = state.settings().freeze_screen;
    let prefetched = take_prefetched_freeze_frame();
    let freeze_screen = freeze_screen || prefetched.is_some() || includes_capture_ui;
    let id = Uuid::new_v4();
    let windows_task =
        (mode == CaptureMode::Window).then(|| take_prefetched_or_spawn_windows(&state));
    let (session, pending_windows) = if freeze_screen {
        let PrefetchedFreezeFrame { pointer, frame } = match prefetched {
            Some(frame) => frame,
            None => {
                let pointer = pointer_position();
                let frame = state.backend.capture_display_at_point(pointer)?;
                PrefetchedFreezeFrame { pointer, frame }
            }
        };
        // The background frame is frozen now, so this capture no longer needs HUD
        // exclusion. Release it before encoding can emit a new preview and allow a
        // rapid follow-up capture to start with its own protection generation.
        set_capture_huds_protected(&app, false);
        // Menu pixels are already in this freeze. Hide it before the overlay
        // covers the display so the two fullscreen surfaces do not stack.
        if includes_capture_ui {
            recording::dismiss_capture_menu_after_nested_snapshot(&app, &state);
        }
        let snapshot_png = encode_overlay_snapshot_with_cursor(
            &frame.image,
            &frame.descriptor,
            pointer,
            state.settings().show_cursor_in_screenshots,
        )?;
        let (targets, pending_windows) =
            take_ready_or_defer_windows(windows_task, &frame.descriptor, Some(&frame.image));
        (
            CaptureSession {
                id,
                mode,
                thumbnail_capture_generation,
                frozen: true,
                display: frame.descriptor,
                image: Some(frame.image),
                snapshot_png,
                windows: targets.windows,
                cursor: pointer,
                shell_chrome: targets.shell_chrome,
                windows_ready: pending_windows.is_none(),
                includes_capture_ui,
            },
            pending_windows,
        )
    } else {
        discard_prefetched_freeze_frame();
        // Live overlay: skip the freeze-frame so hover states can keep changing
        // until commit, then recapture the current desktop.
        let display = display_under_pointer(&state)?;
        let (targets, pending_windows) = take_ready_or_defer_windows(windows_task, &display, None);
        set_capture_huds_protected(&app, false);
        (
            CaptureSession {
                id,
                mode,
                thumbnail_capture_generation,
                frozen: false,
                display,
                image: None,
                snapshot_png: Vec::new(),
                windows: targets.windows,
                cursor: None,
                shell_chrome: targets.shell_chrome,
                windows_ready: pending_windows.is_none(),
                includes_capture_ui: false,
            },
            pending_windows,
        )
    };
    if capture_flow_was_cancelled(flow) {
        return Ok(None);
    }
    if mode == CaptureMode::Region {
        // Live capture already claimed on key-down. Freeze-screen waits until
        // the snapshot exists so the claim panel cannot dismiss tooltips.
        claim_region_capture_cursor(&app);
    }
    if capture_flow_was_cancelled(flow) {
        release_claimed_region_capture_cursor();
        return Ok(None);
    }
    let active = capture_session_to_active(&session);
    state.sessions.lock().insert(id, session);
    if capture_flow_was_cancelled(flow) {
        state.sessions.lock().remove(&id);
        hide_capture_overlay(&app);
        release_claimed_region_capture_cursor();
        return Ok(None);
    }
    // Keep shortcut intent until the overlay is actually on screen. Clearing
    // it here unregisters Escape on Windows/Linux while show() is still queued.
    show_capture_window(&app, &active);
    if capture_flow_was_cancelled(flow) {
        state.sessions.lock().remove(&id);
        hide_capture_overlay(&app);
        release_claimed_region_capture_cursor();
        return Ok(None);
    }
    sync_capture_escape(&app);
    if let Some(task) = pending_windows {
        complete_overlay_windows(app.clone(), state, id, task);
    }
    Ok(Some(active))
}

pub(crate) type WindowListTask = std::thread::JoinHandle<Result<Vec<WindowDescriptor>, AppError>>;

static PENDING_WINDOW_LIST: Mutex<Option<WindowListTask>> = Mutex::new(None);

pub(crate) struct PrefetchedFreezeFrame {
    pub pointer: Option<(i32, i32)>,
    pub frame: DisplayFrame,
}

type FreezeFrameTask = std::thread::JoinHandle<Result<PrefetchedFreezeFrame, CaptureError>>;

static PENDING_FREEZE_FRAME: Mutex<Option<FreezeFrameTask>> = Mutex::new(None);
static FREEZE_PREFETCH_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Start listing windows on shortcut key-down so the window overlay can show
/// targets as soon as the freeze-frame is ready. Capture still waits for key-up
/// so Command-highlighted menu chrome is not frozen into the snapshot.
pub(crate) fn prefetch_capturable_windows(state: &Arc<AppState>) {
    let Ok(mut pending) = PENDING_WINDOW_LIST.lock() else {
        return;
    };
    if pending.as_ref().is_some_and(|task| !task.is_finished()) {
        return;
    }
    let state = state.clone();
    *pending = Some(std::thread::spawn(move || state.windows()));
}

pub(crate) fn take_prefetched_or_spawn_windows(state: &Arc<AppState>) -> WindowListTask {
    PENDING_WINDOW_LIST
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
        .unwrap_or_else(|| spawn_window_list_task(state))
}

pub(crate) fn discard_prefetched_windows() {
    if let Ok(mut pending) = PENDING_WINDOW_LIST.lock() {
        drop(pending.take());
    }
}

fn discard_prefetched_freeze_frame() {
    FREEZE_PREFETCH_GENERATION.fetch_add(1, Ordering::AcqRel);
    if let Ok(mut pending) = PENDING_FREEZE_FRAME.lock() {
        drop(pending.take());
    }
}

fn restore_prefetched_freeze_chrome(app: &AppHandle) {
    restore_capture_ui_snapshot_exclusion(app);
    restore_excluded_recording_chrome(app);
    set_capture_huds_protected(app, false);
    updates::restore_update_notice(app);
    update_thumbnail_stack(app);
}

pub(crate) fn abort_prefetched_freeze_capture(app: &AppHandle) {
    discard_prefetched_freeze_frame();
    restore_prefetched_freeze_chrome(app);
}

/// Capture the freeze-frame on shortcut key-down, before the region cursor-claim
/// panel covers the display. That panel becomes key and eats mouse events, which
/// dismisses tooltips and other hover chrome.
fn prefetch_freeze_frame(app: &AppHandle, state: &Arc<AppState>, claim_region_cursor: bool) {
    discard_prefetched_freeze_frame();
    let generation = FREEZE_PREFETCH_GENERATION.load(Ordering::Acquire);
    let app = app.clone();
    let state = state.clone();
    let Ok(mut pending) = PENDING_FREEZE_FRAME.lock() else {
        return;
    };
    *pending = Some(std::thread::spawn(move || {
        let include_capture_ui = capture_ui_is_visible(&app);
        if include_capture_ui {
            include_capture_ui_in_snapshot(&app);
        }
        let had_visible_hud = conceal_capture_chrome_for_snapshot(&app);
        settle_concealed_capture_chrome(had_visible_hud || include_capture_ui);
        let pointer = pointer_position();
        let frame = match state.backend.capture_display_at_point(pointer) {
            Ok(frame) => frame,
            Err(error) => {
                restore_prefetched_freeze_chrome(&app);
                return Err(error);
            }
        };
        if FREEZE_PREFETCH_GENERATION.load(Ordering::Acquire) != generation {
            restore_prefetched_freeze_chrome(&app);
            return Err(CaptureError::Backend(
                "discarded freeze-frame prefetch".to_owned(),
            ));
        }
        if claim_region_cursor {
            claim_region_capture_cursor(&app);
        }
        Ok(PrefetchedFreezeFrame { pointer, frame })
    }));
}

fn take_prefetched_freeze_frame() -> Option<PrefetchedFreezeFrame> {
    let task = PENDING_FREEZE_FRAME.lock().ok()?.take()?;
    match task.join() {
        Ok(Ok(frame)) => Some(frame),
        Ok(Err(error)) => {
            if !matches!(
                &error,
                CaptureError::Backend(message) if message == "discarded freeze-frame prefetch"
            ) {
                eprintln!("freeze-frame prefetch failed: {error}");
            }
            None
        }
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

pub(crate) fn take_prefetched_or_capture_freeze_frame(
    state: &Arc<AppState>,
) -> Result<PrefetchedFreezeFrame, AppError> {
    if let Some(frame) = take_prefetched_freeze_frame() {
        return Ok(frame);
    }
    let pointer = pointer_position();
    let frame = state.backend.capture_display_at_point(pointer)?;
    Ok(PrefetchedFreezeFrame { pointer, frame })
}

fn take_prefetched_freeze_matching_display(
    state: &Arc<AppState>,
    display_id: &str,
) -> Result<(Option<(i32, i32)>, DisplayFrame), AppError> {
    if let Some(prefetched) = take_prefetched_freeze_frame()
        && prefetched.frame.descriptor.id == display_id
    {
        return Ok((prefetched.pointer, prefetched.frame));
    }
    let pointer = pointer_position();
    Ok((pointer, state.backend.capture_display(display_id)?))
}

pub(crate) fn freeze_prefetch_is_pending() -> bool {
    PENDING_FREEZE_FRAME
        .lock()
        .ok()
        .is_some_and(|pending| pending.is_some())
}

fn capture_surface_window_is_visible(app: &AppHandle, label: &str) -> bool {
    app.get_webview_window(label)
        .is_some_and(|window| window.is_visible().unwrap_or(false))
}

pub(crate) fn capture_overlay_is_visible(app: &AppHandle) -> bool {
    capture_surface_window_is_visible(app, "overlay")
}

fn capture_menu_is_visible(app: &AppHandle) -> bool {
    capture_surface_window_is_visible(app, "recording-selector")
}

fn capture_ui_is_visible(app: &AppHandle) -> bool {
    capture_overlay_is_visible(app) || capture_menu_is_visible(app)
}

/// Overlay is already up: any new screenshot shortcut should freeze that UI.
/// Capture menu still switches region/window/display in place unless the same
/// screenshot target is pressed again.
fn should_freeze_visible_capture_ui(
    overlay_visible: bool,
    menu_visible: bool,
    menu_screenshot_target: Option<CaptureMode>,
    requested: CaptureMode,
) -> bool {
    overlay_visible || (menu_visible && menu_screenshot_target == Some(requested))
}

fn open_menu_screenshot_target(state: &AppState) -> Option<CaptureMode> {
    state
        .recording_selection
        .lock()
        .as_ref()
        .and_then(|selection| {
            (selection.summary.initial_mode == CaptureSelectorMode::Screenshot)
                .then_some(selection.summary.initial_target)
        })
}

fn should_recapture_visible_capture_ui(
    app: &AppHandle,
    state: &AppState,
    requested: CaptureMode,
) -> bool {
    should_freeze_visible_capture_ui(
        capture_overlay_is_visible(app),
        capture_menu_is_visible(app),
        open_menu_screenshot_target(state),
        requested,
    )
}

pub(crate) fn should_recapture_open_capture_menu(
    app: &AppHandle,
    state: &AppState,
    initial_mode: CaptureSelectorMode,
    initial_target: CaptureMode,
) -> bool {
    initial_mode == CaptureSelectorMode::Screenshot
        && capture_menu_is_visible(app)
        && open_menu_screenshot_target(state) == Some(initial_target)
}

const CAPTURE_UI_SNAPSHOT_LABELS: [&str; 2] = ["overlay", "recording-selector"];

pub(crate) fn include_capture_ui_in_snapshot(app: &AppHandle) {
    set_capture_ui_excluded_from_snapshot(app, false);
}

pub(crate) fn restore_capture_ui_snapshot_exclusion(app: &AppHandle) {
    set_capture_ui_excluded_from_snapshot(app, true);
}

fn set_capture_ui_excluded_from_snapshot(app: &AppHandle, excluded: bool) {
    for label in CAPTURE_UI_SNAPSHOT_LABELS {
        let Some(window) = app.get_webview_window(label) else {
            continue;
        };
        let _ = set_window_content_protected(&window, excluded);
        #[cfg(target_os = "macos")]
        if let Err(error) = captures_macos_window::set_excluded_from_capture(&window, excluded) {
            eprintln!("failed to update capture UI sharing for {label}: {error}");
        }
    }
}

pub(crate) fn drain_overlay_sessions_keeping_window(state: &Arc<AppState>) {
    let stale_capture_generations = {
        let mut sessions = state.sessions.lock();
        sessions
            .drain()
            .map(|(_, session)| session.thumbnail_capture_generation)
            .collect::<Vec<_>>()
    };
    if stale_capture_generations.is_empty() {
        return;
    }
    let mut visibility = state.thumbnail_visibility.lock();
    for capture_generation in stale_capture_generations {
        visibility.restore_capture(capture_generation);
    }
}

fn freeze_prefetch_can_start(
    onboarding_completed: bool,
    install_active: bool,
    screenshot_countdown_active: bool,
) -> bool {
    onboarding_completed && !install_active && !screenshot_countdown_active
}

fn freeze_prefetch_is_allowed_for_selector(app: &AppHandle, state: &AppState) -> bool {
    if !freeze_prefetch_can_start(
        state.settings().onboarding_completed,
        updates::install_is_active(app),
        screenshot_countdown_is_active(state),
    ) {
        return false;
    }
    if recording::recording_session_is_active(state) {
        return false;
    }
    if capture_overlay_is_visible(app) {
        return true;
    }
    if capture_menu_is_visible(app) {
        return open_menu_screenshot_target(state) == Some(CaptureMode::Region);
    }
    state.settings().freeze_screen
        && state.sessions.lock().is_empty()
        && state.recording_selection.lock().is_none()
}

fn freeze_prefetch_is_allowed(app: &AppHandle, state: &AppState, mode: CaptureMode) -> bool {
    if !freeze_prefetch_can_start(
        state.settings().onboarding_completed,
        updates::install_is_active(app),
        screenshot_countdown_is_active(state),
    ) {
        return false;
    }
    if should_recapture_visible_capture_ui(app, state, mode) {
        return true;
    }
    state.settings().freeze_screen
        && state.sessions.lock().is_empty()
        && state.recording_selection.lock().is_none()
}

fn should_prefetch_freeze_on_shortcut_press(mode: CaptureMode, freeze_screen: bool) -> bool {
    freeze_screen
        && matches!(
            mode,
            CaptureMode::Region | CaptureMode::Window | CaptureMode::Display
        )
}

fn should_claim_region_cursor_after_freeze(mode: CaptureMode, freeze_after_snapshot: bool) -> bool {
    mode == CaptureMode::Region && freeze_after_snapshot
}

fn should_claim_region_cursor_on_shortcut_press(
    mode: CaptureMode,
    freeze_screen: bool,
    recapture_ui: bool,
) -> bool {
    mode == CaptureMode::Region && !freeze_screen && !recapture_ui
}

pub(crate) const fn screenshot_countdown_seconds_for_capture_ui(
    countdown_seconds: u8,
    includes_capture_ui: bool,
) -> u8 {
    if includes_capture_ui {
        0
    } else {
        countdown_seconds
    }
}

fn prepare_capture_shortcut_press(app: &AppHandle, state: &Arc<AppState>, mode: CaptureMode) {
    let freeze_screen = state.settings().freeze_screen;
    let recapture_ui = should_recapture_visible_capture_ui(app, state, mode);
    let _ = begin_shortcut_capture_flow(app);
    if freeze_prefetch_is_allowed(app, state, mode)
        && (recapture_ui || should_prefetch_freeze_on_shortcut_press(mode, freeze_screen))
    {
        prefetch_freeze_frame(
            app,
            state,
            should_claim_region_cursor_after_freeze(mode, freeze_screen || recapture_ui),
        );
    }
    if should_claim_region_cursor_on_shortcut_press(mode, freeze_screen, recapture_ui) {
        claim_region_capture_cursor(app);
    }
    if mode == CaptureMode::Window {
        prefetch_capturable_windows(state);
    }
}

fn cancel_capture_shortcut_press(app: &AppHandle, mode: CaptureMode) {
    invalidate_capture_flow();
    abort_prefetched_freeze_capture(app);
    if mode == CaptureMode::Region {
        release_claimed_region_capture_cursor();
    }
    if mode == CaptureMode::Window {
        discard_prefetched_windows();
    }
    disarm_capture_escape_intent(app);
}

pub(crate) fn spawn_window_list_task(state: &Arc<AppState>) -> WindowListTask {
    let state = state.clone();
    std::thread::spawn(move || state.windows())
}

pub(crate) fn take_ready_or_defer_windows(
    task: Option<WindowListTask>,
    display: &captures_capture::DisplayDescriptor,
    image: Option<&RgbaImage>,
) -> (WindowSelectionTargets, Option<WindowListTask>) {
    let Some(task) = task else {
        return (WindowSelectionTargets::default(), None);
    };
    if !task.is_finished() {
        return (WindowSelectionTargets::default(), Some(task));
    }
    let listed = match task.join() {
        Ok(windows) => windows,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    (capturable_windows_for_display(listed, display, image), None)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WindowSelectionTargets {
    pub windows: Vec<WindowDescriptor>,
    pub shell_chrome: Vec<WindowDescriptor>,
}

pub(crate) fn capturable_windows_for_display(
    windows: Result<Vec<WindowDescriptor>, AppError>,
    display: &captures_capture::DisplayDescriptor,
    image: Option<&RgbaImage>,
) -> WindowSelectionTargets {
    classify_windows_for_display(
        windows.unwrap_or_else(|error| {
            eprintln!("window targets are unavailable for this capture: {error}");
            Vec::new()
        }),
        display,
        image,
    )
}

fn classify_windows_for_display(
    windows: Vec<WindowDescriptor>,
    display: &captures_capture::DisplayDescriptor,
    image: Option<&RgbaImage>,
) -> WindowSelectionTargets {
    let mut targets = WindowSelectionTargets::default();
    for window in windows {
        match window_pick_role(&window, display) {
            Some(WindowPickRole::Capturable) => targets.windows.push(window),
            Some(WindowPickRole::ShellChrome) => targets.shell_chrome.push(window),
            None => {}
        }
    }
    if let Some(image) = image {
        refine_window_chrome_from_snapshot(
            &mut targets.windows,
            display,
            image,
            window_corner_radius_points(),
        );
    }
    targets
}

fn complete_overlay_windows(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: Uuid,
    task: WindowListTask,
) {
    tauri::async_runtime::spawn_blocking(move || {
        let listed = match task.join() {
            Ok(windows) => windows,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        let mut sessions = state.sessions.lock();
        let Some(session) = sessions.get_mut(&session_id) else {
            return;
        };
        if session.mode != CaptureMode::Window {
            return;
        }
        let targets =
            capturable_windows_for_display(listed, &session.display, session.image.as_ref());
        session.windows = targets.windows;
        session.shell_chrome = targets.shell_chrome;
        session.windows_ready = true;
        let active = capture_session_to_active(session);
        drop(sessions);
        if let Err(error) = app.emit("capture-session-ready", &active) {
            eprintln!("failed to deliver window targets: {error}");
        }
    });
}

fn capture_session_to_active(session: &CaptureSession) -> ActiveSession {
    ActiveSession {
        id: session.id.to_string(),
        mode: session.mode,
        window_coordinate_scale: window_coordinate_scale(&session.display),
        window_corner_radius: window_corner_radius_points(),
        display_corner_radius: display_corner_radius_points(&session.display.id),
        display: session.display.clone(),
        frozen: session.frozen,
        snapshot_url: if session.frozen {
            models::snapshot_url(&session.id.to_string())
        } else {
            String::new()
        },
        windows: session.windows.clone(),
        shell_chrome: session.shell_chrome.clone(),
        windows_ready: session.windows_ready,
    }
}

#[tauri::command]
async fn commit_region(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
    rect: LogicalRect,
) -> CommandResult<Option<CaptureArtifact>> {
    hide_capture_overlay(&app);
    // Keep editors ordered out until this commit finishes or fails. Intermediate
    // frontmost restores (before a countdown) must not re-show them for a frame.
    let _reveal_documents = RevealDocumentWindowsOnDrop::new(&app);
    let state = state.inner().clone();
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let session = state
        .sessions
        .lock()
        .remove(&id)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let thumbnail_capture_generation = session.thumbnail_capture_generation;
    sync_capture_escape(&app);
    let countdown_seconds = screenshot_countdown_seconds_for_capture_ui(
        state.settings().screenshot_countdown_seconds,
        session.includes_capture_ui,
    );
    let image = if countdown_seconds > 0 {
        set_capture_huds_protected(&app, true);
        let completed = match run_screenshot_countdown(
            app.clone(),
            state.clone(),
            &session.display,
            countdown_seconds,
            thumbnail_capture_generation,
        )
        .await
        {
            Ok(completed) => completed,
            Err(error) => {
                set_capture_huds_protected(&app, false);
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        };
        if !completed {
            // Cancel already restored the stack and cleared HUD protection.
            return Ok(None);
        }
        let live_image = crop_live_region(&state, &session.display.id, rect);
        set_capture_huds_protected(&app, false);
        match live_image {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
    } else if session.frozen {
        // Map overlay/CSS DIPs onto the capture buffer. On Windows the display
        // descriptor is physical while the overlay is logical, so do not use the
        // native-geometry scale used for window crops.
        match session
            .image
            .as_ref()
            .ok_or(AppError::SessionUnavailable)
            .and_then(|source| {
                let mut image = crop_region_from_display(&session.display, source, rect)?;
                apply_screenshot_cursor_to_region(
                    &mut image,
                    &session.display,
                    source,
                    rect,
                    session.cursor,
                    state.settings().show_cursor_in_screenshots,
                );
                Ok(image)
            }) {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
    } else {
        set_capture_huds_protected(&app, true);
        let live_image = crop_live_region(&state, &session.display.id, rect);
        set_capture_huds_protected(&app, false);
        match live_image {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
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
    result.map(Some).map_err(|error| error.to_string())
}

#[tauri::command]
async fn commit_window(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
    window_id: String,
) -> CommandResult<Option<CaptureArtifact>> {
    hide_capture_overlay(&app);
    // Keep editors ordered out until this commit finishes or fails. Intermediate
    // frontmost restores (before a countdown) must not re-show them for a frame.
    let _reveal_documents = RevealDocumentWindowsOnDrop::new(&app);
    let state = state.inner().clone();
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let session = state
        .sessions
        .lock()
        .remove(&id)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let thumbnail_capture_generation = session.thumbnail_capture_generation;
    let includes_capture_ui = session.includes_capture_ui;
    sync_capture_escape(&app);

    let selected_window = match session
        .windows
        .iter()
        .find(|window| window.id == window_id)
        .cloned()
    {
        Some(window) => window,
        None => {
            restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
            return Err(AppError::InvalidSelection.to_string());
        }
    };
    let countdown_seconds = screenshot_countdown_seconds_for_capture_ui(
        state.settings().screenshot_countdown_seconds,
        includes_capture_ui,
    );
    let display_crop_is_safe = window_display_crop_is_safe(&selected_window, &session.windows);
    let image = if countdown_seconds > 0 {
        set_capture_huds_protected(&app, true);
        let completed = match run_screenshot_countdown(
            app.clone(),
            state.clone(),
            &session.display,
            countdown_seconds,
            thumbnail_capture_generation,
        )
        .await
        {
            Ok(completed) => completed,
            Err(error) => {
                set_capture_huds_protected(&app, false);
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        };
        if !completed {
            // Cancel already restored the stack and cleared HUD protection.
            return Ok(None);
        }
        let live_image = capture_live_window(&state, &selected_window);
        set_capture_huds_protected(&app, false);
        match live_image {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
    } else if session.frozen {
        // Preserve the exact frozen pixels when the selected bounds are clear or
        // contain only an app-owned transient such as a menu. If another app/window
        // covers the target, use the native surface so that occluder is not saved.
        match resolve_window_capture(
            display_crop_is_safe,
            || {
                let source = session.image.as_ref()?;
                let mut image = crop_window_from_session(&session, &window_id)?;
                apply_screenshot_cursor_to_window_crop(
                    &mut image,
                    &session.display,
                    source,
                    &selected_window,
                    session.cursor,
                    state.settings().show_cursor_in_screenshots,
                );
                Some(image)
            },
            || {
                let mut image = state.backend.capture_window(&window_id)?;
                apply_screenshot_cursor_on_window(
                    &mut image,
                    &selected_window,
                    session.display.scale_factor,
                    session.cursor,
                    state.settings().show_cursor_in_screenshots,
                );
                Ok(image)
            },
        ) {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
    } else {
        set_capture_huds_protected(&app, true);
        let live_image = capture_live_window(&state, &selected_window);
        set_capture_huds_protected(&app, false);
        match live_image {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
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
    result.map(Some).map_err(|error| error.to_string())
}

#[tauri::command]
async fn commit_display(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> CommandResult<Option<CaptureArtifact>> {
    hide_capture_overlay(&app);
    let _reveal_documents = RevealDocumentWindowsOnDrop::new(&app);
    let state = state.inner().clone();
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let session = state
        .sessions
        .lock()
        .remove(&id)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let thumbnail_capture_generation = session.thumbnail_capture_generation;
    sync_capture_escape(&app);
    let countdown_seconds = screenshot_countdown_seconds_for_capture_ui(
        state.settings().screenshot_countdown_seconds,
        session.includes_capture_ui,
    );
    let image = if countdown_seconds > 0 {
        set_capture_huds_protected(&app, true);
        let completed = match run_screenshot_countdown(
            app.clone(),
            state.clone(),
            &session.display,
            countdown_seconds,
            thumbnail_capture_generation,
        )
        .await
        {
            Ok(completed) => completed,
            Err(error) => {
                set_capture_huds_protected(&app, false);
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        };
        if !completed {
            return Ok(None);
        }
        let live_image = capture_live_display(&state, &session.display.id);
        set_capture_huds_protected(&app, false);
        match live_image {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
    } else if session.frozen {
        match session
            .image
            .ok_or(AppError::SessionUnavailable)
            .map(|mut image| {
                apply_screenshot_cursor(
                    &mut image,
                    &session.display,
                    session.cursor,
                    state.settings().show_cursor_in_screenshots,
                );
                image
            }) {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
    } else {
        set_capture_huds_protected(&app, true);
        let live_image = capture_live_display(&state, &session.display.id);
        set_capture_huds_protected(&app, false);
        match live_image {
            Ok(image) => image,
            Err(error) => {
                restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
                return Err(error.to_string());
            }
        }
    };

    let result = finish_capture(
        &app,
        &state,
        CaptureMode::Display,
        image,
        thumbnail_capture_generation,
    )
    .await;
    if result.is_err() {
        restore_thumbnail_capture(&app, &state, thumbnail_capture_generation);
    }
    result.map(Some).map_err(|error| error.to_string())
}

fn capture_live_display(state: &AppState, display_id: &str) -> Result<RgbaImage, AppError> {
    ensure_capture_session_available()?;
    let pointer = pointer_position();
    let mut frame = state.backend.capture_display(display_id)?;
    apply_screenshot_cursor(
        &mut frame.image,
        &frame.descriptor,
        pointer,
        state.settings().show_cursor_in_screenshots,
    );
    Ok(frame.image)
}

#[tauri::command]
fn cancel_capture(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_id: String,
) -> CommandResult<()> {
    invalidate_capture_flow();
    abort_prefetched_freeze_capture(&app);
    release_claimed_region_capture_cursor();
    hide_capture_overlay(&app);
    restore_capture_ui_snapshot_exclusion(&app);
    let id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    if let Some(session) = state.sessions.lock().remove(&id) {
        restore_thumbnail_capture(&app, state.inner(), session.thumbnail_capture_generation);
    }
    reveal_document_windows_after_capture(&app);
    updates::restore_update_notice(&app);
    CAPTURE_ESCAPE_INTENT.store(false, Ordering::Release);
    sync_capture_escape(&app);
    Ok(())
}

/// Cancels every in-progress capture surface. Escape uses this so cancellation
/// does not depend on a focused webview, a freeze-frame having painted, or a
/// known session id.
#[tauri::command]
fn cancel_active_capture(app: AppHandle, state: tauri::State<'_, Arc<AppState>>) {
    cancel_active_capture_ui(&app, state.inner());
}

/// Releases overlay keyboard/cursor grabs and hides the freeze-frame as soon as
/// a selection commits or cancels. The overlay webview calls this in parallel
/// with Tauri `window.hide()` so a leftover key window cannot swallow typing
/// in other apps while the slower capture command hops to Rust.
#[tauri::command]
fn dismiss_capture_surface(app: AppHandle) {
    hide_capture_overlay(&app);
    restore_capture_ui_snapshot_exclusion(&app);
}

/// Cancel screenshot capture UI so an update can restart. Failures are logged
/// and ignored — an open overlay must not block installation.
pub(crate) fn dismiss_capture_ui_for_update(app: &AppHandle, state: &Arc<AppState>) {
    cancel_screenshot_countdown_inner(app, state.clone());
    hide_capture_overlay(app);
    let generations: Vec<u64> = state
        .sessions
        .lock()
        .drain()
        .map(|(_, session)| session.thumbnail_capture_generation)
        .collect();
    for generation in generations {
        restore_thumbnail_capture(app, state, generation);
    }
    recording::dismiss_recording_selection_for_update(app, state);
    restore_capture_ui_snapshot_exclusion(app);
    reveal_document_windows_after_capture(app);
    invalidate_capture_flow();
    CAPTURE_ESCAPE_INTENT.store(false, Ordering::Release);
    sync_capture_escape(app);
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

#[tauri::command]
fn get_screenshot_countdown(
    state: tauri::State<'_, Arc<AppState>>,
) -> Option<ScreenshotCountdownTick> {
    let runtime = state.screenshot_countdown.lock();
    runtime.active.then_some(ScreenshotCountdownTick {
        remaining_seconds: runtime.remaining_seconds,
    })
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
        runtime.remaining_seconds = seconds;
        runtime.generation
    };
    arm_capture_escape(&app);

    if let Err(error) = show_screenshot_countdown(&app, display) {
        let mut runtime = state.screenshot_countdown.lock();
        if runtime.generation == generation {
            runtime.active = false;
            runtime.thumbnail_capture_generation = None;
            runtime.remaining_seconds = 0;
        }
        drop(runtime);
        disarm_capture_escape_intent(&app);
        return Err(error);
    }
    if !screenshot_countdown_is_current(&state, generation) {
        destroy_screenshot_countdown(&app);
        disarm_capture_escape_intent(&app);
        return Ok(false);
    }

    for remaining in (1..=seconds).rev() {
        {
            let mut runtime = state.screenshot_countdown.lock();
            if !runtime.active || runtime.generation != generation {
                drop(runtime);
                destroy_screenshot_countdown(&app);
                disarm_capture_escape_intent(&app);
                return Ok(false);
            }
            runtime.remaining_seconds = remaining;
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
            runtime.remaining_seconds = 0;
        }
    }
    destroy_screenshot_countdown(&app);
    disarm_capture_escape_intent(&app);
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
        runtime.remaining_seconds = 0;
        runtime.thumbnail_capture_generation.take()
    };
    destroy_screenshot_countdown(app);
    set_capture_huds_protected(app, false);
    if let Some(thumbnail_capture_generation) = thumbnail_capture_generation {
        restore_thumbnail_capture(app, &state, thumbnail_capture_generation);
    }
    reveal_document_windows_after_capture(app);
    updates::restore_update_notice(app);
    invalidate_capture_flow();
    CAPTURE_ESCAPE_INTENT.store(false, Ordering::Release);
    sync_capture_escape(app);
}

fn overlay_session_is_live(app: &AppHandle, session_id: &str) -> bool {
    let Ok(id) = Uuid::parse_str(session_id) else {
        return false;
    };
    app.state::<Arc<AppState>>()
        .sessions
        .lock()
        .contains_key(&id)
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
    #[cfg(target_os = "macos")]
    captures_macos_window::set_excluded_from_capture(&window, true)
        .map_err(|error| AppError::Task(error.to_owned()))?;
    window.show()?;
    // Exclude after show: a hidden HWND with this affinity blocks NVIDIA capture.
    set_window_content_protected(&window, true)?;
    #[cfg(target_os = "macos")]
    captures_macos_window::conceal_documents_under_opaque_capture_surface();
    #[cfg(target_os = "macos")]
    recording::focus_recording_window(app, "screenshot-countdown");
    if let Err(error) = window.set_focus() {
        eprintln!("failed to focus screenshot countdown: {error}");
    }
    Ok(())
}

fn destroy_screenshot_countdown(app: &AppHandle) {
    // Restore the previous app while the countdown still covers the display so
    // sibling document windows cannot flash above it for a frame when the timer
    // webview is torn down. Document windows stay ordered out until
    // reveal_document_windows_after_capture runs at session end.
    #[cfg(target_os = "macos")]
    captures_macos_window::restore_frontmost_app_after_capture();
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
    Ok(state
        .sessions
        .lock()
        .get(&id)
        .map(capture_session_to_active))
}

#[tauri::command]
fn get_pending_session(state: tauri::State<'_, Arc<AppState>>) -> Option<ActiveSession> {
    state
        .sessions
        .lock()
        .values()
        .next()
        .map(capture_session_to_active)
}

#[cfg_attr(all(target_os = "macos", not(test)), allow(dead_code))]
fn capture_cursor_icon(mode: CaptureMode) -> CursorIcon {
    if mode == CaptureMode::Region {
        CursorIcon::Crosshair
    } else {
        CursorIcon::Default
    }
}

fn claim_region_capture_cursor(app: &AppHandle) {
    arm_capture_escape(app);
    #[cfg(target_os = "macos")]
    {
        let cursor = captures_macos_window::CaptureCursor::overlay_region();
        if let Some(window) = app.get_webview_window("overlay") {
            if let Err(error) = captures_macos_window::activate_capture_cursor(&window, cursor) {
                eprintln!("failed to claim the region capture cursor: {error}");
            }
        } else {
            captures_macos_window::claim_capture_cursor(cursor);
        }
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.set_cursor_icon(CursorIcon::Crosshair);
    }
}

fn release_claimed_region_capture_cursor() {
    #[cfg(target_os = "macos")]
    captures_macos_window::release_capture_cursor();
}

fn apply_overlay_capture_cursor(
    window: &tauri::WebviewWindow,
    mode: CaptureMode,
) -> CommandResult<()> {
    #[cfg(not(target_os = "macos"))]
    window
        .set_cursor_icon(capture_cursor_icon(mode))
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    captures_macos_window::activate_capture_cursor(
        window,
        captures_macos_window::CaptureCursor::overlay(mode == CaptureMode::Region),
    )
    .map_err(str::to_owned)?;
    Ok(())
}

/// GTK creates the underlying GDK window only when a window is first shown, and
/// tao unwraps it while handling a cursor-ignore request, so asking a window
/// that has never been shown to become click-through takes down the whole event
/// loop. macOS and Windows accept the call while hidden, which the capture
/// surfaces and notices rely on so their first frame cannot steal desktop
/// clicks.
const fn click_through_applies(visible: bool) -> bool {
    !cfg!(target_os = "linux") || visible
}

/// Pass pointer events through a window, tolerating platforms that cannot apply
/// click-through before the window exists onscreen. Callers that build a hidden
/// window repeat the call after `show()` so Linux still gets the state.
pub(crate) fn set_click_through(window: &tauri::WebviewWindow, ignore: bool) -> tauri::Result<()> {
    if !click_through_applies(window.is_visible().unwrap_or(false)) {
        return Ok(());
    }
    window.set_ignore_cursor_events(ignore)
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
        apply_overlay_capture_cursor(&window, mode)?;
        // Keep the overlay click-through until the frozen snapshot is painted.
        // Otherwise an early wake (needed so WKWebView will load the image)
        // steals pointer events from the desktop.
        set_click_through(&window, true).map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        {
            // Focusing the overlay activates Captures and would otherwise leave
            // open editors/history windows frontmost after the overlay hides.
            captures_macos_window::remember_frontmost_app_before_activation();
            // Do not call Tauri `show()` here: it uses `makeKeyAndOrderFront:`
            // and can flash a black unpainted WKWebView. Present at a tiny
            // alpha without taking key focus until reveal_capture_overlay.
            captures_macos_window::present_capture_overlay(&window).map_err(str::to_owned)?;
            let _ = mode;
        }
        #[cfg(not(target_os = "macos"))]
        {
            window.show().map_err(|error| error.to_string())?;
            set_click_through(&window, true).map_err(|error| error.to_string())?;
            window.set_focus().map_err(|error| error.to_string())?;
            let _ = mode;
        }
        if !overlay_session_is_live(&app, &session_id) {
            hide_capture_overlay(&app);
            return Err(AppError::SessionUnavailable.to_string());
        }
        handoff_capture_escape_from_intent(&app, window.is_visible().unwrap_or(false));
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
    set_click_through(&window, false).map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    {
        // Window capture keeps the frozen frame opaque while sibling documents
        // deactivate. Region capture cuts through that frame, so its live
        // desktop pixels must keep those documents visible underneath.
        captures_macos_window::reveal_capture_overlay(&window, mode == CaptureMode::Region)
            .map_err(str::to_owned)?;
        if let Err(error) = captures_macos_window::elevate_capture_surface(&window) {
            eprintln!("failed to keep the capture overlay above the menu bar: {error}");
        }
        if let Err(error) = captures_macos_window::focus_window(&window) {
            eprintln!("failed to focus capture overlay: {error}");
        }
        apply_overlay_capture_cursor(&window, mode)?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.set_focus().map_err(|error| error.to_string())?;
        apply_overlay_capture_cursor(&window, mode)?;
    }
    if !overlay_session_is_live(&app, &session_id) {
        hide_capture_overlay(&app);
        return Err(AppError::SessionUnavailable.to_string());
    }
    handoff_capture_escape_from_intent(&app, window.is_visible().unwrap_or(false));
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
    apply_overlay_capture_cursor(&window, mode)
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, Arc<AppState>>) -> AppSettings {
    state.settings()
}

#[tauri::command]
fn get_onboarding_state(state: tauri::State<'_, Arc<AppState>>) -> CommandResult<OnboardingState> {
    onboarding_state(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
fn request_onboarding_screen_permission(
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<OnboardingState> {
    #[cfg(target_os = "macos")]
    {
        let request_access =
            mark_screen_permission_request(state.inner()).map_err(|error| error.to_string())?;
        match state.backend.ensure_permission(request_access) {
            Ok(()) => {}
            Err(CaptureError::PermissionRequestStarted) => {
                *state.screen_permission_requested_this_launch.lock() = true;
            }
            Err(CaptureError::PermissionDenied) => {
                *state.screen_permission_requested_this_launch.lock() = true;
                open_macos_screen_recording_settings().map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }

    onboarding_state(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
fn set_onboarding_desktop_audio(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    enabled: bool,
) -> CommandResult<OnboardingState> {
    update_onboarding_recording_audio(&app, state.inner(), Some(enabled), None)
        .map_err(|error| error.to_string())?;
    onboarding_state(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
fn request_onboarding_microphone_permission(
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<OnboardingState> {
    #[cfg(target_os = "macos")]
    {
        if !captures_recording_macos::request_microphone_access()
            && !captures_recording_macos::microphone_can_request()
        {
            open_macos_microphone_settings().map_err(|error| error.to_string())?;
        }
    }

    onboarding_state(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
fn set_onboarding_microphone(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    enabled: bool,
) -> CommandResult<OnboardingState> {
    #[cfg(target_os = "macos")]
    if enabled && !captures_recording_macos::microphone_authorized() {
        return onboarding_state(state.inner()).map_err(|error| error.to_string());
    }
    update_onboarding_recording_audio(&app, state.inner(), None, Some(enabled))
        .map_err(|error| error.to_string())?;
    onboarding_state(state.inner()).map_err(|error| error.to_string())
}

fn update_onboarding_recording_audio(
    app: &AppHandle,
    state: &AppState,
    capture_system_audio: Option<bool>,
    enable_microphone: Option<bool>,
) -> Result<(), AppError> {
    let settings = {
        let mut settings = state.settings.write();
        if let Some(enabled) = capture_system_audio {
            settings.recording.capture_system_audio = enabled;
        }
        if let Some(enabled) = enable_microphone {
            settings.recording.microphone_device_id = enabled.then(|| "default".to_owned());
        }
        storage::save_settings(&settings)?;
        settings.clone()
    };
    if let Err(error) = app.emit("settings-changed", &settings) {
        eprintln!("failed to broadcast onboarding audio settings: {error}");
    }
    Ok(())
}

#[tauri::command]
fn restart_captures_for_permissions(app: AppHandle) {
    crash_report::mark_clean_exit();
    app.request_restart();
}

#[tauri::command]
fn complete_onboarding(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> CommandResult<()> {
    let permission = onboarding_state(state.inner()).map_err(|error| error.to_string())?;
    if permission.screen_recording_required && !permission.screen_recording_granted {
        return Err("screen recording access is required before setup can finish".to_owned());
    }

    let settings = {
        let mut settings = state.settings.write();
        if !settings.onboarding_completed {
            settings.onboarding_completed = true;
            storage::save_settings(&settings).map_err(|error| error.to_string())?;
        }
        settings.clone()
    };
    if let Err(error) = app.emit("settings-changed", &settings) {
        eprintln!("failed to broadcast completed onboarding: {error}");
    }
    hide_window(&app, ONBOARDING_WINDOW_LABEL);
    show_startup_notice(&app, STARTUP_NOTICE_AFTER_SETUP_VISIBLE);
    Ok(())
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
    {
        let visibility = state.thumbnail_visibility.lock();
        if visibility.is_suppressed() {
            return None;
        }
    }
    let window = app.get_webview_window("thumbnail")?;
    if !thumbnail_window_is_presented(&window) {
        return None;
    }
    #[cfg(target_os = "macos")]
    captures_macos_window::note_thumbnail_pointer_poll();
    webview_pointer_position(&window)
}

#[tauri::command]
fn get_capture_pointer_position(window: tauri::WebviewWindow) -> Option<ThumbnailPointerPosition> {
    webview_pointer_position(&window)
}

/// False on Wayland-only Linux, where `mouse_position` cannot sample the cursor.
#[tauri::command]
fn thumbnail_pointer_poll_available() -> bool {
    thumbnail_global_pointer_poll_available(cfg!(target_os = "linux"), x11_display_is_present())
}

const fn thumbnail_global_pointer_poll_available(is_linux: bool, x11_display: bool) -> bool {
    !is_linux || x11_display
}

fn x11_display_is_present() -> bool {
    #[cfg(target_os = "linux")]
    return x11_display_available();
    #[cfg(not(target_os = "linux"))]
    false
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThumbnailCursorKind {
    Default,
    Pointer,
    Grab,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThumbnailCursorAction {
    Ignore,
    Reset,
    Apply(ThumbnailCursorKind),
}

fn thumbnail_cursor_action(
    suppressed: bool,
    visible: bool,
    kind: ThumbnailCursorKind,
) -> ThumbnailCursorAction {
    if suppressed {
        ThumbnailCursorAction::Ignore
    } else if visible && !matches!(kind, ThumbnailCursorKind::Default) {
        ThumbnailCursorAction::Apply(kind)
    } else {
        ThumbnailCursorAction::Reset
    }
}

#[cfg(not(target_os = "macos"))]
fn thumbnail_tauri_cursor_icon(kind: ThumbnailCursorKind) -> CursorIcon {
    match kind {
        ThumbnailCursorKind::Default => CursorIcon::Default,
        ThumbnailCursorKind::Pointer => CursorIcon::Hand,
        ThumbnailCursorKind::Grab => CursorIcon::Grab,
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
            let presented = thumbnail_window_is_presented(&cursor_window);
            let result = match thumbnail_cursor_action(suppressed, presented, kind) {
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
        let Some(window) = app.get_webview_window("thumbnail") else {
            return Ok(());
        };
        let suppressed = state.thumbnail_visibility.lock().is_suppressed();
        let presented = thumbnail_window_is_presented(&window);
        match thumbnail_cursor_action(suppressed, presented, kind) {
            ThumbnailCursorAction::Ignore => Ok(()),
            ThumbnailCursorAction::Reset => window
                .set_cursor_icon(CursorIcon::Default)
                .map_err(|error| error.to_string()),
            ThumbnailCursorAction::Apply(effective_kind) => window
                .set_cursor_icon(thumbnail_tauri_cursor_icon(effective_kind))
                .map_err(|error| error.to_string()),
        }
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
            let presented = thumbnail_window_is_presented(&window);
            let result = match thumbnail_cursor_action(suppressed, presented, kind) {
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
        set_thumbnail_cursor(app, state, kind)
    }
}

/// Capture-suppressed stacks leave hit testing unchanged.
fn thumbnail_cursor_ignore_update(suppressed: bool, requested_ignore: bool) -> Option<bool> {
    if suppressed {
        None
    } else {
        Some(requested_ignore)
    }
}

#[tauri::command]
fn set_thumbnail_ignore_cursor_events(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    ignore: bool,
) -> CommandResult<()> {
    let suppressed = state.thumbnail_visibility.lock().is_suppressed();
    let Some(effective_ignore) = thumbnail_cursor_ignore_update(suppressed, ignore) else {
        return Ok(());
    };
    let window = app
        .get_webview_window("thumbnail")
        .ok_or_else(|| "capture thumbnail is unavailable".to_owned())?;
    set_click_through(&window, effective_ignore).map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    if captures_macos_window::thumbnail_passthrough_must_resign_key(effective_ignore) {
        let _ = captures_macos_window::resign_panel_key_without_raising_documents(&window);
    }
    Ok(())
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
    if count == 0 {
        // Do not re-arm an empty stack. On Windows a transparent always-on-top
        // window can keep eating clicks after hide() unless click-through is
        // applied first (hide_thumbnail_window does that).
        hide_thumbnail_window(&window);
        state.thumbnail_visibility.lock().reset_session_placement();
        return Ok(());
    }
    // Never force the tall stack hit-testable. Collapsed piles keep the
    // expanded window height; empty chrome would cover other apps and steal
    // clicks and typing until JS polls. Restore z-order only. The pointer poll
    // re-enables hits while the cursor is actually on a live card.
    let _ = window.set_always_on_top(true);
    if !suppressed {
        // Re-apply geometry after display sleep (DPI / work area can change).
        update_thumbnail_stack(&app);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThumbnailPointerSpace {
    /// macOS `CGEvent` locations are logical points; Tauri window origin is physical.
    LogicalMouse,
    /// Windows `GetCursorPos` and X11 `XQueryPointer` match Tauri's physical origin.
    PhysicalMouse,
}

const fn thumbnail_pointer_space() -> ThumbnailPointerSpace {
    if cfg!(target_os = "macos") {
        ThumbnailPointerSpace::LogicalMouse
    } else {
        ThumbnailPointerSpace::PhysicalMouse
    }
}

#[derive(Clone, Copy, Debug)]
struct ThumbnailWindowFrame {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

#[cfg(test)]
fn thumbnail_pointer_position(
    mouse_x: f64,
    mouse_y: f64,
    window_x: i32,
    window_y: i32,
    window_width: u32,
    window_height: u32,
    scale: f64,
) -> ThumbnailPointerPosition {
    thumbnail_pointer_in_space(
        mouse_x,
        mouse_y,
        ThumbnailWindowFrame {
            x: window_x,
            y: window_y,
            width: window_width,
            height: window_height,
            scale,
        },
        ThumbnailPointerSpace::LogicalMouse,
    )
}

fn thumbnail_pointer_in_space(
    mouse_x: f64,
    mouse_y: f64,
    frame: ThumbnailWindowFrame,
    space: ThumbnailPointerSpace,
) -> ThumbnailPointerPosition {
    let scale = frame.scale.max(1.0);
    let window_x = f64::from(frame.x);
    let window_y = f64::from(frame.y);
    let (x, y) = match space {
        ThumbnailPointerSpace::LogicalMouse => {
            (mouse_x - window_x / scale, mouse_y - window_y / scale)
        }
        ThumbnailPointerSpace::PhysicalMouse => {
            ((mouse_x - window_x) / scale, (mouse_y - window_y) / scale)
        }
    };
    let width = f64::from(frame.width) / scale;
    let height = f64::from(frame.height) / scale;
    ThumbnailPointerPosition {
        x,
        y,
        inside: x >= 0.0 && y >= 0.0 && x < width && y < height,
    }
}

fn webview_pointer_position(window: &tauri::WebviewWindow) -> Option<ThumbnailPointerPosition> {
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    let scale = window.scale_factor().ok()?.max(1.0);
    let (mouse_x, mouse_y) = pointer_position().map(|(x, y)| (f64::from(x), f64::from(y)))?;
    Some(thumbnail_pointer_in_space(
        mouse_x,
        mouse_y,
        ThumbnailWindowFrame {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            scale,
        },
        thumbnail_pointer_space(),
    ))
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
        || settings.recording.window_shortcut.trim().is_empty()
        || settings.recording.display_shortcut.trim().is_empty()
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
    let record_region_shortcut =
        parse_shortcut(&settings.recording.video_shortcut).map_err(|error| error.to_string())?;
    let record_window_shortcut =
        parse_shortcut(&settings.recording.window_shortcut).map_err(|error| error.to_string())?;
    let record_display_shortcut =
        parse_shortcut(&settings.recording.display_shortcut).map_err(|error| error.to_string())?;
    let shortcuts = [
        new_capture_shortcut,
        region_shortcut,
        window_shortcut,
        display_shortcut,
        record_region_shortcut,
        record_window_shortcut,
        record_display_shortcut,
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
    settings.onboarding_completed = previous_settings.onboarding_completed;

    let shortcuts_changed = settings.new_capture_shortcut != previous_settings.new_capture_shortcut
        || settings.region_shortcut != previous_settings.region_shortcut
        || settings.window_shortcut != previous_settings.window_shortcut
        || settings.display_shortcut != previous_settings.display_shortcut
        || settings.recording.video_shortcut != previous_settings.recording.video_shortcut
        || settings.recording.window_shortcut != previous_settings.recording.window_shortcut
        || settings.recording.display_shortcut != previous_settings.recording.display_shortcut;
    if shortcuts_changed && let Err(error) = register_shortcuts_with(&app, &settings) {
        let _ = register_shortcuts_with(&app, &previous_settings);
        return Err(error.to_string());
    }
    if shortcuts_changed {
        refresh_tray_menu(&app);
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
    let show_mini_previews_changed =
        settings.show_mini_previews != previous_settings.show_mini_previews;
    let mini_preview_setting_changed = show_mini_previews_changed
        || settings.include_mini_previews_in_captures
            != previous_settings.include_mini_previews_in_captures
        || settings.mini_preview_placement != previous_settings.mini_preview_placement;
    let recording_controls_setting_changed = settings.include_recording_controls_in_captures
        != previous_settings.include_recording_controls_in_captures;
    *state.settings.write() = settings.clone();
    if mini_preview_setting_changed {
        if !settings.show_mini_previews {
            let mut visibility = state.thumbnail_visibility.lock();
            visibility.stop_waiting_for_artifact();
            visibility.reset_session_placement();
        } else if settings.mini_preview_placement != previous_settings.mini_preview_placement {
            state.thumbnail_visibility.lock().clear_stack_origin();
        }
        // When including mini previews in captures, keep the stack shareable.
        if settings.include_mini_previews_in_captures
            && let Some(window) = app.get_webview_window("thumbnail")
        {
            let _ = set_window_content_protected(&window, false);
        }
        update_thumbnail_stack(&app);
    }
    if show_mini_previews_changed
        || settings.show_update_changelog != previous_settings.show_update_changelog
    {
        updates::refresh_update_notice(&app);
    }
    if recording_controls_setting_changed
        && let Some(window) = app.get_webview_window("recording-hud")
    {
        let excluded = !settings.include_recording_controls_in_captures;
        let _ = set_window_content_protected(&window, excluded);
        #[cfg(target_os = "macos")]
        if let Err(error) = captures_macos_window::set_excluded_from_capture(&window, excluded) {
            eprintln!("failed to update recording controls capture sharing: {error}");
        }
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
    state.find_artifact(&artifact_id)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum PreviewFileDropLanding {
    /// Dropped back on the mini-preview stack, including the source card.
    PreviewStack,
    /// Dropped on another Captures window (screenshot editor, history, …).
    AppWindow,
    /// Finder, Slack, a browser, the desktop, or any other external target.
    External,
}

fn classify_preview_file_drop(
    internal_drop_recent: bool,
    over_preview_stack: bool,
    over_app_window: bool,
) -> PreviewFileDropLanding {
    if internal_drop_recent {
        return PreviewFileDropLanding::AppWindow;
    }
    if over_preview_stack {
        return PreviewFileDropLanding::PreviewStack;
    }
    if over_app_window {
        return PreviewFileDropLanding::AppWindow;
    }
    PreviewFileDropLanding::External
}

/// Where a mini-preview file drag ended. Self-drops keep the card; only
/// external targets dismiss it.
#[tauri::command]
fn preview_file_drop_landing(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    x: f64,
    y: f64,
) -> PreviewFileDropLanding {
    let internal_drop_recent = state
        .last_internal_file_drop
        .lock()
        .is_some_and(|at| at.elapsed() <= std::time::Duration::from_millis(1_500));
    let (x, y) = preview_file_drop_cursor(&app, x, y);
    classify_preview_file_drop(
        internal_drop_recent,
        named_captures_window_contains_point(&app, "thumbnail", x, y),
        captures_window_contains_point(&app, x, y),
    )
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
/// Coordinates match the live pointer used by thumbnail hit testing (logical
/// points on macOS, physical pixels on Windows/Linux). The drag crate's
/// `cursorPos` can disagree with Tauri window frames on Retina displays, so
/// callers should prefer `pointer_position()` when it is available.
fn captures_window_contains_point(app: &AppHandle, x: f64, y: f64) -> bool {
    app.webview_windows()
        .into_iter()
        .any(|(_label, window)| webview_contains_screen_point(&window, x, y))
}

fn named_captures_window_contains_point(app: &AppHandle, label: &str, x: f64, y: f64) -> bool {
    app.get_webview_window(label)
        .is_some_and(|window| webview_contains_screen_point(&window, x, y))
}

/// Map `tauri-plugin-drag` / drag-rs `cursorPos` into thumbnail pointer space.
///
/// On macOS the plugin flips AppKit’s bottom-left *points* with
/// `CGDisplay::pixels_high()` (device pixels). Hover hit-testing expects
/// logical top-left, like `CGEvent`. Windows and Linux already match Tauri’s
/// physical origin, so those values pass through.
fn drag_plugin_cursor_to_pointer_space(
    x: f64,
    y: f64,
    primary_pixel_height: f64,
    primary_scale: f64,
    space: ThumbnailPointerSpace,
) -> (f64, f64) {
    match space {
        ThumbnailPointerSpace::LogicalMouse => {
            let scale = primary_scale.max(1.0);
            let pixel_height = primary_pixel_height.max(0.0);
            let point_height = pixel_height / scale;
            (x, y + point_height - pixel_height)
        }
        ThumbnailPointerSpace::PhysicalMouse => (x, y),
    }
}

fn preview_file_drop_cursor(app: &AppHandle, x: f64, y: f64) -> (f64, f64) {
    let Some(monitor) = app.primary_monitor().ok().flatten() else {
        return (x, y);
    };
    drag_plugin_cursor_to_pointer_space(
        x,
        y,
        f64::from(monitor.size().height),
        monitor.scale_factor(),
        thumbnail_pointer_space(),
    )
}

fn webview_contains_screen_point(window: &tauri::WebviewWindow, x: f64, y: f64) -> bool {
    let Ok(true) = window.is_visible() else {
        return false;
    };
    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };
    let scale = window.scale_factor().ok().unwrap_or(1.0).max(1.0);
    thumbnail_pointer_in_space(
        x,
        y,
        ThumbnailWindowFrame {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            scale,
        },
        thumbnail_pointer_space(),
    )
    .inside
}

#[cfg(test)]
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
        // Prefer a permanent Captures-folder save when history still knows it.
        // That keeps editor overwrite / filename defaults aligned with the
        // original export instead of forcing a fresh “-edited” name.
        let path = entry
            .saved_path
            .as_ref()
            .filter(|saved| Path::new(saved).is_file())
            .cloned();
        let artifact = CaptureArtifact {
            id: entry.id,
            path,
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
    let _ = screenshot_editor::discard_screenshot_editor_draft_files(&artifact_id);
    state.history.lock().retain(|entry| entry.id != artifact_id);
    state
        .recording_artifacts
        .lock()
        .retain(|artifact| artifact.summary.id != artifact_id);
    state.forget_editor_artifacts_for_ids(
        std::slice::from_ref(&artifact_id),
        &open_screenshot_editor_owner_ids(&app),
    );
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
            let _ = screenshot_editor::discard_screenshot_editor_draft_files(artifact_id);
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
    state.forget_editor_artifacts_for_ids(&ids, &open_screenshot_editor_owner_ids(&app));
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

    let was_unsaved = existing_path.is_none();
    if was_unsaved {
        let settings = state.settings();
        let format = settings.screenshot_format;
        let path = tauri::async_runtime::spawn_blocking(move || {
            let bytes = screenshot_editor::encode_saved_screenshot(&png, format)?;
            storage::save_encoded_capture(&bytes, &settings, format.extension())
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
    if was_unsaved {
        updates::refresh_update_notice(&app);
    }
    Ok(artifact)
}

#[tauri::command]
fn reveal_artifact(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    let artifact = state
        .find_artifact(&artifact_id)
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
    let (viewer_theme, viewer_background) = document_window_chrome(&app);
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
    .theme(viewer_theme)
    .background_color(viewer_background)
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

fn open_screenshot_editor_owner_ids(app: &AppHandle) -> Vec<String> {
    app.webview_windows()
        .into_keys()
        .filter_map(|label| {
            label
                .strip_prefix(SCREENSHOT_EDITOR_WINDOW_PREFIX)
                .map(str::to_owned)
        })
        .collect()
}

fn remove_artifact(app: &AppHandle, state: &Arc<AppState>, artifact_id: &str) -> CommandResult<()> {
    let removed = {
        let mut artifacts = state.artifacts.lock();
        let original_len = artifacts.len();
        artifacts.retain(|artifact| artifact.id != artifact_id);
        artifacts.len() != original_len
    };
    if !removed {
        // Dismiss is idempotent so an in-flight card exit can finish after
        // the artifact was already taken off the stack.
        return Ok(());
    }
    app.emit("artifact-removed", artifact_id)
        .map_err(|error| error.to_string())?;
    updates::restore_update_notice(app);
    Ok(())
}

#[tauri::command]
fn dismiss_all_artifacts(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_ids: Vec<String>,
) -> CommandResult<Vec<String>> {
    let ids: Vec<String> = state
        .take_preview_stack(&artifact_ids)
        .into_iter()
        .map(|artifact| artifact.id)
        .collect();
    for id in &ids {
        app.emit("artifact-removed", id)
            .map_err(|error| error.to_string())?;
    }
    updates::restore_update_notice(&app);
    // Leave the native window up so the frontend Close streak can finish.
    // Hiding here blanks WKWebView and the cards vanish with no motion.
    // The stack syncs after the last card unmounts.
    Ok(ids)
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
fn open_preferences(app: AppHandle, target: Option<String>) -> CommandResult<()> {
    show_preferences_target(&app, target.as_deref());
    Ok(())
}

#[tauri::command]
fn open_system_screenshot_shortcut_settings(app: AppHandle) -> CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        for url in [
            "x-apple.systempreferences:com.apple.Keyboard-Settings.extension?Screenshots",
            "x-apple.systempreferences:com.apple.Keyboard-Settings.extension?Shortcuts",
            "x-apple.systempreferences:com.apple.preference.keyboard?Shortcuts",
        ] {
            if app.opener().open_url(url, None::<&str>).is_ok() {
                return Ok(());
            }
        }
        Err("could not open Keyboard settings".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        app.opener()
            .open_url("ms-settings:easeofaccess-keyboard", None::<&str>)
            .map_err(|error| error.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        for args in [
            &["gnome-control-center", "keyboard"][..],
            &["systemsettings", "kcm_keys"][..],
        ] {
            if Command::new(args[0]).args(&args[1..]).spawn().is_ok() {
                return Ok(());
            }
        }
        Err("could not open Keyboard settings".to_owned())
    }
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
    restore_excluded_recording_chrome(app);
    // Re-show capture-concealed editors only after the capture session ends so
    // they cannot flash above the restored frontmost app for a few frames.
    let _reveal_documents = RevealDocumentWindowsOnDrop::new(app);

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
    #[cfg(target_os = "macos")]
    {
        if !captures_macos_window::is_main_thread() {
            let app = app.clone();
            return run_on_appkit_main(move || write_image_to_clipboard_inner(&app, image))
                .ok_or_else(|| {
                    AppError::Clipboard("clipboard write did not run on the main thread".to_owned())
                })?;
        }
    }
    write_image_to_clipboard_inner(app, image)
}

fn write_image_to_clipboard_inner(
    app: &AppHandle,
    image: RgbaImage,
) -> Result<ClipboardWrite, AppError> {
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

pub(crate) fn display_under_pointer(
    state: &AppState,
) -> Result<captures_capture::DisplayDescriptor, AppError> {
    pick_display_under_pointer(&state.monitors()?).ok_or(CaptureError::TargetUnavailable.into())
}

pub(crate) fn pick_display_under_pointer(
    displays: &[captures_capture::DisplayDescriptor],
) -> Option<captures_capture::DisplayDescriptor> {
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
}

fn apply_screenshot_cursor(
    image: &mut RgbaImage,
    display: &captures_capture::DisplayDescriptor,
    pointer: Option<(i32, i32)>,
    enabled: bool,
) {
    apply_screenshot_cursor_in_crop(
        image,
        display,
        0,
        0,
        image.width(),
        image.height(),
        pointer,
        enabled,
    );
}

/// Encode an overlay-only copy so the frozen selector reflects the cursor
/// setting without baking that glyph into the source used for the final crop.
pub(crate) fn encode_overlay_snapshot_with_cursor(
    image: &RgbaImage,
    display: &captures_capture::DisplayDescriptor,
    pointer: Option<(i32, i32)>,
    enabled: bool,
) -> Result<Vec<u8>, AppError> {
    let mut snapshot = image.clone();
    apply_screenshot_cursor(&mut snapshot, display, pointer, enabled);
    storage::encode_overlay_snapshot(&snapshot)
}

#[allow(clippy::too_many_arguments)]
fn apply_screenshot_cursor_in_crop(
    image: &mut RgbaImage,
    display: &captures_capture::DisplayDescriptor,
    crop_x: u32,
    crop_y: u32,
    source_width: u32,
    source_height: u32,
    pointer: Option<(i32, i32)>,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let Some(pointer) = pointer else {
        return;
    };
    captures_capture::overlay_pointer_cursor_in_crop(
        image,
        display,
        crop_x,
        crop_y,
        source_width,
        source_height,
        pointer,
        captures_capture::screenshot_pointer_scale(display.scale_factor),
    );
}

fn apply_screenshot_cursor_to_region(
    cropped: &mut RgbaImage,
    display: &captures_capture::DisplayDescriptor,
    source: &RgbaImage,
    rect: LogicalRect,
    pointer: Option<(i32, i32)>,
    enabled: bool,
) {
    let Ok(physical) = region_physical_rect(display, source, rect) else {
        return;
    };
    apply_screenshot_cursor_in_crop(
        cropped,
        display,
        physical.x,
        physical.y,
        source.width(),
        source.height(),
        pointer,
        enabled,
    );
}

fn apply_screenshot_cursor_to_window_crop(
    cropped: &mut RgbaImage,
    display: &captures_capture::DisplayDescriptor,
    source: &RgbaImage,
    window: &captures_capture::WindowDescriptor,
    pointer: Option<(i32, i32)>,
    enabled: bool,
) {
    let Some(physical) = window_physical_rect(display, source, window) else {
        return;
    };
    apply_screenshot_cursor_in_crop(
        cropped,
        display,
        physical.x,
        physical.y,
        source.width(),
        source.height(),
        pointer,
        enabled,
    );
}

fn apply_screenshot_cursor_on_window(
    image: &mut RgbaImage,
    window: &captures_capture::WindowDescriptor,
    display_scale_factor: f64,
    pointer: Option<(i32, i32)>,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let Some(pointer) = pointer else {
        return;
    };
    captures_capture::overlay_pointer_cursor_on_window(
        image,
        window,
        pointer,
        captures_capture::screenshot_pointer_scale(display_scale_factor),
    );
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

fn onboarding_state(state: &AppState) -> Result<OnboardingState, AppError> {
    let settings = state.settings();
    let microphone_enabled = settings.recording.microphone_device_id.is_some();
    let capture_system_audio = settings.recording.capture_system_audio;

    #[cfg(target_os = "macos")]
    {
        let can_request = screen_permission_request_available(state)?;
        let requested_this_launch = *state.screen_permission_requested_this_launch.lock();
        // After the first prompt, `preflight()` can stay false even once the
        // user enables Screen Recording in System Settings. `request()` returns
        // the live TCC answer and does not show another dialog.
        let request_access = !can_request || requested_this_launch;
        let screen_recording_granted = state.backend.ensure_permission(request_access).is_ok();
        let microphone_granted = captures_recording_macos::microphone_authorized();
        Ok(OnboardingState {
            platform: std::env::consts::OS.to_owned(),
            screen_recording_required: true,
            screen_recording_granted,
            screen_recording_can_request: !screen_recording_granted && can_request,
            screen_recording_requested_this_launch: requested_this_launch,
            capture_system_audio,
            microphone_enabled: microphone_enabled && microphone_granted,
            microphone_granted,
            microphone_can_request: !microphone_granted
                && captures_recording_macos::microphone_can_request(),
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(OnboardingState {
            platform: std::env::consts::OS.to_owned(),
            screen_recording_required: false,
            screen_recording_granted: true,
            screen_recording_can_request: false,
            screen_recording_requested_this_launch: false,
            capture_system_audio,
            microphone_enabled,
            microphone_granted: true,
            microphone_can_request: false,
        })
    }
}

#[cfg(target_os = "macos")]
fn screen_permission_request_id() -> Result<String, AppError> {
    let executable = std::env::current_exe()?;
    let metadata = executable.metadata()?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(format!(
        "{}:{}:{modified}",
        executable.to_string_lossy(),
        metadata.len()
    ))
}

#[cfg(target_os = "macos")]
fn screen_permission_request_available(state: &AppState) -> Result<bool, AppError> {
    let request_id = screen_permission_request_id()?;
    Ok(state
        .settings()
        .last_screen_permission_request_id
        .as_deref()
        != Some(&request_id))
}

fn mark_screen_permission_request(state: &AppState) -> Result<bool, AppError> {
    #[cfg(target_os = "macos")]
    {
        let request_id = screen_permission_request_id()?;
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

pub(crate) fn ensure_capture_session_available() -> Result<(), AppError> {
    if !captures_session::capture_session_available() {
        return Err(CaptureError::SessionUnavailable.into());
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn restart_and_retry_capture(app: &AppHandle, mode: CaptureMode) -> Result<(), AppError> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    {
        let mut settings = state.settings.write();
        settings.pending_capture_after_restart = Some(mode);
        storage::save_settings(&settings)?;
    }
    crash_report::mark_clean_exit();
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

pub(crate) fn window_corner_radius_points() -> f64 {
    #[cfg(target_os = "macos")]
    {
        captures_macos_window::standard_window_corner_radius_points()
    }
    #[cfg(not(target_os = "macos"))]
    {
        0.0
    }
}

pub(crate) fn display_corner_radius_points(display_id: &str) -> f64 {
    #[cfg(target_os = "macos")]
    {
        captures_macos_window::display_corner_radius_points(display_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = display_id;
        0.0
    }
}

/// Prefer AppKit `NSWindow.frame` over xcap's `kCGWindowBounds`.
/// Quartz window bounds include the drop shadow, which makes the window
/// selector overlay miss the real chrome (especially Settings/Preferences).
pub(crate) fn apply_native_window_frames(windows: &mut [captures_capture::WindowDescriptor]) {
    #[cfg(target_os = "macos")]
    {
        let frames = captures_macos_window::visible_window_frames();
        if frames.is_empty() {
            return;
        }
        for window in windows {
            let Ok(window_number) = window.id.parse::<u32>() else {
                continue;
            };
            let Some(frame) = frames
                .iter()
                .find(|frame| frame.window_number == window_number)
            else {
                continue;
            };
            window.x = frame.x;
            window.y = frame.y;
            window.width = frame.width;
            window.height = frame.height;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = windows;
    }
}

static CAPTURE_ESCAPE_APP: OnceLock<AppHandle> = OnceLock::new();
/// True between shortcut key-down / capture start and the first painted
/// overlay, selector, or countdown. Real window and session state take over
/// after that so Escape is not stolen during an in-progress recording.
static CAPTURE_ESCAPE_INTENT: AtomicBool = AtomicBool::new(false);
static CAPTURE_ESCAPE_HOTKEY_REGISTERED: AtomicBool = AtomicBool::new(false);
static CAPTURE_ESCAPE_CANCELING: AtomicBool = AtomicBool::new(false);
/// Serializes deferred global-shortcut mutations. The plugin invokes handlers
/// while holding its shortcut registry mutex, so registering or unregistering
/// Escape synchronously from one of those handlers deadlocks the event loop.
static CAPTURE_ESCAPE_HOTKEY_SYNC: Mutex<()> = Mutex::new(());
static CAPTURE_ESCAPE_HOTKEY_SYNC_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Bumped on shortcut press and on Escape so an in-flight freeze-frame cannot
/// present after the user already cancelled.
static CAPTURE_FLOW_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Generation armed on shortcut key-down. Key-up must not start a new capture
/// after Escape invalidated this token while the chord was still held.
static SHORTCUT_CAPTURE_FLOW: AtomicU64 = AtomicU64::new(0);

fn install_capture_escape_cancel(app: &AppHandle) {
    let _ = CAPTURE_ESCAPE_APP.set(app.clone());
    captures_session::set_capture_escape_handler(Some(on_native_capture_escape));
    if let Err(error) = captures_session::ensure_capture_escape_hook() {
        eprintln!("could not install capture Escape hook: {error}");
    }
    #[cfg(target_os = "macos")]
    {
        captures_macos_window::set_capture_escape_handler(Some(on_native_capture_escape));
        captures_macos_window::ensure_capture_escape_monitors();
    }
}

fn on_native_capture_escape() {
    let Some(app) = CAPTURE_ESCAPE_APP.get() else {
        return;
    };
    handle_native_capture_escape(app);
}

fn handle_native_capture_escape(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if !capture_escape_ui_is_active(app, &state) {
        return;
    }
    cancel_active_capture_ui(app, &state);
}

pub(crate) fn arm_capture_escape(app: &AppHandle) {
    CAPTURE_ESCAPE_INTENT.store(true, Ordering::Release);
    sync_capture_escape(app);
}

pub(crate) fn disarm_capture_escape_intent(app: &AppHandle) {
    CAPTURE_ESCAPE_INTENT.store(false, Ordering::Release);
    sync_capture_escape(app);
}

fn next_capture_flow_generation() -> u64 {
    loop {
        let next = CAPTURE_FLOW_GENERATION
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        if next != 0 {
            return next;
        }
    }
}

/// Starts a new capture flow and arms Escape immediately, including during
/// freeze-frame prefetch before any overlay exists.
pub(crate) fn begin_capture_flow(app: &AppHandle) -> u64 {
    let generation = next_capture_flow_generation();
    arm_capture_escape(app);
    generation
}

fn begin_shortcut_capture_flow(app: &AppHandle) -> u64 {
    let generation = begin_capture_flow(app);
    SHORTCUT_CAPTURE_FLOW.store(generation, Ordering::Release);
    generation
}

fn clear_shortcut_capture_flow() {
    SHORTCUT_CAPTURE_FLOW.store(0, Ordering::Release);
}

/// True when Escape already cancelled the matching shortcut key-down. The
/// key-up must not call `adopt_or_begin_capture_flow`, which would start a
/// fresh generation and show the freeze overlay again.
fn shortcut_release_was_cancelled(app: &AppHandle) -> bool {
    let generation = SHORTCUT_CAPTURE_FLOW.swap(0, Ordering::AcqRel);
    if captures_session::shortcut_release_should_start_capture(
        generation,
        CAPTURE_FLOW_GENERATION.load(Ordering::Acquire),
    ) {
        return false;
    }
    abort_prefetched_freeze_capture(app);
    true
}

/// Reuses the shortcut-press flow when Escape is already armed; otherwise
/// starts a new one for menu-driven capture.
pub(crate) fn adopt_or_begin_capture_flow(app: &AppHandle) -> u64 {
    let current = CAPTURE_FLOW_GENERATION.load(Ordering::Acquire);
    if CAPTURE_ESCAPE_INTENT.load(Ordering::Acquire) && current != 0 {
        return current;
    }
    begin_capture_flow(app)
}

pub(crate) fn invalidate_capture_flow() {
    let _ = next_capture_flow_generation();
}

#[must_use]
pub(crate) fn capture_flow_was_cancelled(generation: u64) -> bool {
    !captures_session::capture_flow_is_current(
        generation,
        CAPTURE_FLOW_GENERATION.load(Ordering::Acquire),
    )
}

/// Drop shortcut intent only after a capture surface is visible so Windows and
/// Linux keep the Escape hook registered through the async overlay show.
fn handoff_capture_escape_from_intent(app: &AppHandle, surface_visible: bool) {
    if captures_session::capture_escape_may_drop_intent(surface_visible) {
        CAPTURE_ESCAPE_INTENT.store(false, Ordering::Release);
    }
    sync_capture_escape(app);
}

pub(crate) fn sync_capture_escape(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let active = capture_escape_ui_is_active(app, &state);
    captures_session::set_capture_escape_enabled(active);
    #[cfg(target_os = "macos")]
    captures_macos_window::set_capture_escape_armed(active);
    schedule_capture_escape_hotkey_sync(app, active);
}

fn capture_escape_ui_is_active(app: &AppHandle, state: &AppState) -> bool {
    captures_session::CaptureEscapeUi::from_live_surfaces(
        CAPTURE_ESCAPE_INTENT.load(Ordering::Acquire),
        window_is_visible(app, "overlay"),
        state.recording_selection.lock().is_some() || window_is_visible(app, "recording-selector"),
        screenshot_countdown_is_active(state) || window_is_visible(app, "screenshot-countdown"),
        recording::recording_countdown_is_active(state)
            || window_is_visible(app, "recording-countdown"),
    )
    .is_armed()
}

fn window_is_visible(app: &AppHandle, label: &str) -> bool {
    app.get_webview_window(label)
        .is_some_and(|window| window.is_visible().unwrap_or(false))
}

fn cancel_active_capture_ui(app: &AppHandle, state: &Arc<AppState>) {
    if CAPTURE_ESCAPE_CANCELING.swap(true, Ordering::AcqRel) {
        return;
    }
    invalidate_capture_flow();
    CAPTURE_ESCAPE_INTENT.store(false, Ordering::Release);
    abort_prefetched_freeze_capture(app);
    hide_capture_overlay(app);
    let generations: Vec<u64> = state
        .sessions
        .lock()
        .drain()
        .map(|(_, session)| session.thumbnail_capture_generation)
        .collect();
    for generation in generations {
        restore_thumbnail_capture(app, state, generation);
    }
    cancel_screenshot_countdown_inner(app, state.clone());
    recording::dismiss_open_recording_selection(app, state);
    recording::discard_recording_countdown_from_escape(app, state);
    release_claimed_region_capture_cursor();
    reveal_document_windows_after_capture(app);
    updates::restore_update_notice(app);
    sync_capture_escape(app);
    CAPTURE_ESCAPE_CANCELING.store(false, Ordering::Release);
}

fn schedule_capture_escape_hotkey_sync(app: &AppHandle, active: bool) {
    let generation = CAPTURE_ESCAPE_HOTKEY_SYNC_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(_guard) = CAPTURE_ESCAPE_HOTKEY_SYNC.lock() else {
            return;
        };
        if CAPTURE_ESCAPE_HOTKEY_SYNC_GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        if active {
            register_capture_escape_hotkey(&app);
        } else {
            unregister_capture_escape_hotkey(&app);
        }
    });
}

fn register_capture_escape_hotkey(app: &AppHandle) {
    if CAPTURE_ESCAPE_HOTKEY_REGISTERED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Ok(parsed) = parse_shortcut("Escape") else {
        CAPTURE_ESCAPE_HOTKEY_REGISTERED.store(false, Ordering::Release);
        eprintln!("could not parse the capture Escape shortcut");
        return;
    };
    if let Err(error) = app
        .global_shortcut()
        .on_shortcut(parsed, |app, _shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            handle_native_capture_escape(app);
        })
    {
        CAPTURE_ESCAPE_HOTKEY_REGISTERED.store(false, Ordering::Release);
        eprintln!("could not register capture Escape: {error}");
    }
}

fn unregister_capture_escape_hotkey(app: &AppHandle) {
    if !CAPTURE_ESCAPE_HOTKEY_REGISTERED.swap(false, Ordering::AcqRel) {
        return;
    }
    if let Ok(parsed) = parse_shortcut("Escape") {
        let _ = app.global_shortcut().unregister(parsed);
    }
}

fn register_shortcuts(app: &AppHandle) {
    let settings = app.state::<Arc<AppState>>().settings();
    if let Err(error) = register_shortcuts_with(app, &settings) {
        eprintln!("failed to register global shortcuts: {error}");
    }
}

fn register_shortcuts_with(app: &AppHandle, settings: &AppSettings) -> Result<(), AppError> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|error| AppError::Shortcut(error.to_string()))?;
    CAPTURE_ESCAPE_HOTKEY_REGISTERED.store(false, Ordering::Release);
    // Unbind overlapping OS screenshot tools before Captures claims the same
    // chords. On macOS, persist through cfprefsd and disable the live
    // WindowServer hotkeys; writing the plist file alone does not stop
    // Screenshot.app.
    let overlapping_macos_screenshot_hotkeys =
        models::macos_screenshot_hotkeys_conflicting_with(settings);
    #[cfg(target_os = "macos")]
    if !overlapping_macos_screenshot_hotkeys.is_empty() {
        disable_overlapping_macos_screenshot_shortcuts(&overlapping_macos_screenshot_hotkeys);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = overlapping_macos_screenshot_hotkeys;
    let overlapping_gnome_screenshot_bindings =
        models::gnome_screenshot_bindings_conflicting_with(settings);
    #[cfg(target_os = "linux")]
    if !overlapping_gnome_screenshot_bindings.is_empty() {
        disable_overlapping_gnome_screenshot_shortcuts(&overlapping_gnome_screenshot_bindings);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = overlapping_gnome_screenshot_bindings;
    #[cfg(target_os = "linux")]
    disable_overlapping_kde_screenshot_shortcuts(settings);
    #[cfg(target_os = "windows")]
    install_windows_screenshot_takeover(app, settings);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = models::settings_use_print_screen(settings);
        let _ = models::settings_use_super_shift_s(settings);
    }
    register_new_capture_shortcut(app, &settings.new_capture_shortcut)?;
    register_shortcut(app, &settings.region_shortcut, CaptureMode::Region)?;
    register_shortcut(app, &settings.window_shortcut, CaptureMode::Window)?;
    register_shortcut(app, &settings.display_shortcut, CaptureMode::Display)?;
    register_recording_shortcut(app, &settings.recording.video_shortcut, CaptureMode::Region)?;
    register_recording_shortcut(
        app,
        &settings.recording.window_shortcut,
        CaptureMode::Window,
    )?;
    register_recording_shortcut(
        app,
        &settings.recording.display_shortcut,
        CaptureMode::Display,
    )?;
    sync_capture_escape(app);
    Ok(())
}

fn register_new_capture_shortcut(app: &AppHandle, shortcut: &str) -> Result<(), AppError> {
    if skip_windows_os_owned_super_shift_s(shortcut) {
        return Ok(());
    }
    let parsed = parse_shortcut(shortcut)?;
    let armed = AtomicBool::new(false);
    app.global_shortcut()
        .on_shortcut(parsed, move |app, _shortcut, event| {
            let state = app.state::<Arc<AppState>>().inner().clone();
            let preferences_or_onboarding_focused = app
                .get_webview_window("preferences")
                .is_some_and(|window| window.is_focused().unwrap_or(false))
                || app
                    .get_webview_window(ONBOARDING_WINDOW_LABEL)
                    .is_some_and(|window| window.is_focused().unwrap_or(false));
            if event.state() == ShortcutState::Pressed {
                clear_shortcut_capture_flow();
                if !preferences_or_onboarding_focused
                    && freeze_prefetch_is_allowed_for_selector(app, &state)
                {
                    let _ = begin_shortcut_capture_flow(app);
                    prefetch_freeze_frame(app, &state, false);
                }
            }
            if !should_trigger_shortcut(&armed, event.state()) {
                return;
            }
            if shortcut_release_was_cancelled(app) {
                return;
            }
            if preferences_or_onboarding_focused {
                abort_prefetched_freeze_capture(app);
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                open_capture_controls(&app, CaptureSelectorMode::Screenshot);
            });
        })
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn register_shortcut(app: &AppHandle, shortcut: &str, mode: CaptureMode) -> Result<(), AppError> {
    if skip_windows_os_owned_super_shift_s(shortcut) {
        // Explorer/Snipping Tool own Win+Shift+S before RegisterHotKey. The
        // low-level hook in captures-session swallows that chord instead.
        return Ok(());
    }
    let parsed = parse_shortcut(shortcut)?;
    let armed = AtomicBool::new(false);
    let suppressed_while_pressed = AtomicBool::new(false);
    app.global_shortcut()
        .on_shortcut(parsed, move |app, _shortcut, event| {
            let state = app.state::<Arc<AppState>>().inner().clone();
            if !state.settings().onboarding_completed {
                if event.state() == ShortcutState::Released {
                    show_onboarding(app);
                }
                return;
            }
            let suppressed = shortcut_capture_is_suppressed(app, &state);
            if event.state() == ShortcutState::Pressed {
                clear_shortcut_capture_flow();
                if !suppressed
                    && !recording::screenshot_capture_is_blocked(&state)
                    && !screenshot_countdown_is_active(&state)
                {
                    prepare_capture_shortcut_press(app, &state, mode);
                }
            }
            let trigger_is_suppressed =
                track_shortcut_suppression(&suppressed_while_pressed, event.state(), suppressed);
            if !should_trigger_shortcut(&armed, event.state()) || trigger_is_suppressed {
                if event.state() == ShortcutState::Released && trigger_is_suppressed {
                    clear_shortcut_capture_flow();
                    cancel_capture_shortcut_press(app, mode);
                }
                return;
            }
            if shortcut_release_was_cancelled(app) {
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if mode == CaptureMode::Display && !recording::recording_session_is_active(&state) {
                    open_capture_controls_with_target(
                        &app,
                        CaptureSelectorMode::Screenshot,
                        CaptureMode::Display,
                    );
                    return;
                }
                if let Err(error) = start_capture_inner(app.clone(), state, mode).await
                    && !matches!(
                        &error,
                        AppError::CaptureInProgress | AppError::ScreenshotCancelled
                    )
                {
                    report_capture_error(&app, &error, mode);
                }
            });
        })
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn register_recording_shortcut(
    app: &AppHandle,
    shortcut: &str,
    target: CaptureMode,
) -> Result<(), AppError> {
    if skip_windows_os_owned_super_shift_s(shortcut) {
        return Ok(());
    }
    let parsed = parse_shortcut(shortcut)?;
    let armed = AtomicBool::new(false);
    let suppressed_while_pressed = AtomicBool::new(false);
    app.global_shortcut()
        .on_shortcut(parsed, move |app, _shortcut, event| {
            let state = app.state::<Arc<AppState>>().inner().clone();
            if !state.settings().onboarding_completed {
                if event.state() == ShortcutState::Released {
                    show_onboarding(app);
                }
                return;
            }
            let suppressed = shortcut_capture_is_suppressed(app, &state);
            if event.state() == ShortcutState::Pressed {
                clear_shortcut_capture_flow();
                if !suppressed
                    && freeze_prefetch_is_allowed_for_selector(app, &state)
                    && !recording::recording_session_is_active(&state)
                {
                    let _ = begin_shortcut_capture_flow(app);
                    prefetch_freeze_frame(app, &state, false);
                }
            }
            let trigger_is_suppressed =
                track_shortcut_suppression(&suppressed_while_pressed, event.state(), suppressed);
            if !should_trigger_shortcut(&armed, event.state()) || trigger_is_suppressed {
                if event.state() == ShortcutState::Released && trigger_is_suppressed {
                    clear_shortcut_capture_flow();
                    abort_prefetched_freeze_capture(app);
                }
                return;
            }
            if shortcut_release_was_cancelled(app) {
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = recording::prepare_capture_selector_inner(
                    app.clone(),
                    state,
                    CaptureSelectorMode::Recording,
                    target,
                )
                .await
                    && !matches!(
                        &error,
                        AppError::CaptureInProgress | AppError::ScreenshotCancelled
                    )
                {
                    report_recording_error(&app, &error);
                }
            });
        })
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn shortcut_capture_is_suppressed(app: &AppHandle, state: &AppState) -> bool {
    if !state.settings().onboarding_completed
        && app
            .get_webview_window(ONBOARDING_WINDOW_LABEL)
            .is_some_and(|window| window.is_focused().unwrap_or(false))
    {
        return true;
    }
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

fn parse_shortcut(shortcut: &str) -> Result<Shortcut, AppError> {
    shortcut
        .parse::<Shortcut>()
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

#[cfg(target_os = "macos")]
fn disable_overlapping_macos_screenshot_shortcuts(ids: &[u32]) {
    if let Err(error) = persist_disabled_macos_screenshot_hotkeys(ids) {
        eprintln!("could not persist disabled macOS Screenshot shortcuts: {error}");
    }
    if let Err(error) = captures_macos_window::disable_symbolic_hotkeys(ids) {
        eprintln!("could not disable overlapping macOS Screenshot shortcuts: {error}");
    }
}

#[cfg(target_os = "macos")]
fn persist_disabled_macos_screenshot_hotkeys(ids: &[u32]) -> Result<(), String> {
    for id in ids {
        let output = Command::new("defaults")
            .args(models::macos_screenshot_hotkey_defaults_write_args(*id))
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            return Err(if stderr.is_empty() { stdout } else { stderr });
        }
    }
    let activate = "/System/Library/PrivateFrameworks/SystemAdministration.framework/Resources/activateSettings";
    if Path::new(activate).is_file() {
        let _ = Command::new(activate).arg("-u").status();
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn disable_overlapping_gnome_screenshot_shortcuts(bindings: &[models::GnomeScreenshotBinding]) {
    if let Err(error) = disable_gnome_screenshot_bindings(bindings) {
        eprintln!("could not disable overlapping GNOME screenshot shortcuts: {error}");
    }
}

#[cfg(target_os = "linux")]
fn disable_gnome_screenshot_bindings(
    bindings: &[models::GnomeScreenshotBinding],
) -> Result<(), String> {
    let mut errors = Vec::new();
    for binding in bindings {
        match clear_gsettings_key(binding.schema, binding.key) {
            Ok(()) => {}
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(target_os = "linux")]
fn clear_gsettings_key(schema: &str, key: &str) -> Result<(), String> {
    let mut last_error = String::new();
    for binary in models::GNOME_GSETTINGS_BINARIES {
        for value in ["[]", "['']"] {
            match Command::new(binary)
                .args(["set", schema, key, value])
                .output()
            {
                Ok(output) if output.status.success() => return Ok(()),
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                    last_error = if stderr.is_empty() { stdout } else { stderr };
                }
                Err(error) => last_error = error.to_string(),
            }
        }
    }
    Err(last_error)
}

#[cfg(target_os = "linux")]
fn disable_overlapping_kde_screenshot_shortcuts(settings: &AppSettings) {
    if !models::settings_use_super_shift_s(settings) {
        return;
    }
    if let Err(error) = disable_kde_spectacle_region_shortcut() {
        eprintln!("could not disable overlapping KDE Spectacle shortcuts: {error}");
    }
}

#[cfg(target_os = "linux")]
fn disable_kde_spectacle_region_shortcut() -> Result<(), String> {
    let mut wrote = false;
    for binary in ["kwriteconfig6", "kwriteconfig5"] {
        match Command::new(binary)
            .args(models::KDE_SPECTACLE_REGION_WRITE_ARGS)
            .output()
        {
            Ok(output) if output.status.success() => {
                wrote = true;
                break;
            }
            _ => {}
        }
    }
    if !wrote {
        return Ok(());
    }
    for reload in [
        &[
            "qdbus6",
            "org.kde.kglobalaccel",
            "/kglobalaccel",
            "org.kde.KGlobalAccel.reloadConfig",
        ][..],
        &[
            "qdbus",
            "org.kde.kglobalaccel",
            "/kglobalaccel",
            "org.kde.KGlobalAccel.reloadConfig",
        ][..],
    ] {
        if Command::new(reload[0]).args(&reload[1..]).status().is_ok() {
            break;
        }
    }
    Ok(())
}

fn skip_windows_os_owned_super_shift_s(shortcut: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        models::shortcut_is_super_shift_s(shortcut)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = shortcut;
        false
    }
}

#[cfg(target_os = "windows")]
fn install_windows_screenshot_takeover(app: &AppHandle, settings: &AppSettings) {
    if models::settings_use_print_screen(settings) {
        disable_windows_print_screen_snipping();
    }
    let action = models::settings_super_shift_s_action(settings);
    let takeover = models::settings_use_super_shift_s(settings);
    let _ = WIN_SHIFT_S_APP.set(app.clone());
    if let Ok(mut slot) = WIN_SHIFT_S_ACTION.lock() {
        *slot = action;
    }
    captures_session::set_win_shift_s_takeover_enabled(takeover);
    if action.is_none() {
        captures_session::set_win_shift_s_handler(None);
        return;
    }
    captures_session::set_win_shift_s_handler(Some(on_win_shift_s));
    if let Err(error) = captures_session::ensure_win_shift_s_takeover() {
        eprintln!("could not take over Win+Shift+S from Snipping Tool: {error}");
    }
}

#[cfg(target_os = "windows")]
static WIN_SHIFT_S_APP: OnceLock<AppHandle> = OnceLock::new();
#[cfg(target_os = "windows")]
static WIN_SHIFT_S_ACTION: Mutex<Option<models::SuperShiftSAction>> = Mutex::new(None);
#[cfg(target_os = "windows")]
static WIN_SHIFT_S_ARMED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static WIN_SHIFT_S_SUPPRESSED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
fn on_win_shift_s(phase: captures_session::WinShiftSPhase) {
    let Some(app) = WIN_SHIFT_S_APP.get() else {
        return;
    };
    let event_state = match phase {
        captures_session::WinShiftSPhase::Pressed => ShortcutState::Pressed,
        captures_session::WinShiftSPhase::Released => ShortcutState::Released,
    };
    let action = WIN_SHIFT_S_ACTION
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .unwrap_or(models::SuperShiftSAction::Region);
    match action {
        models::SuperShiftSAction::NewCapture => dispatch_new_capture_shortcut(app, event_state),
        models::SuperShiftSAction::Region => {
            dispatch_capture_shortcut(app, CaptureMode::Region, event_state)
        }
        models::SuperShiftSAction::Window => {
            dispatch_capture_shortcut(app, CaptureMode::Window, event_state)
        }
        models::SuperShiftSAction::Display => {
            dispatch_capture_shortcut(app, CaptureMode::Display, event_state)
        }
        models::SuperShiftSAction::RecordRegion => {
            dispatch_recording_shortcut(app, CaptureMode::Region, event_state)
        }
        models::SuperShiftSAction::RecordWindow => {
            dispatch_recording_shortcut(app, CaptureMode::Window, event_state)
        }
        models::SuperShiftSAction::RecordDisplay => {
            dispatch_recording_shortcut(app, CaptureMode::Display, event_state)
        }
    }
}

#[cfg(target_os = "windows")]
fn dispatch_new_capture_shortcut(app: &AppHandle, event_state: ShortcutState) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let preferences_or_onboarding_focused = app
        .get_webview_window("preferences")
        .is_some_and(|window| window.is_focused().unwrap_or(false))
        || app
            .get_webview_window(ONBOARDING_WINDOW_LABEL)
            .is_some_and(|window| window.is_focused().unwrap_or(false));
    if event_state == ShortcutState::Pressed {
        clear_shortcut_capture_flow();
        if !preferences_or_onboarding_focused
            && freeze_prefetch_is_allowed_for_selector(app, &state)
        {
            let _ = begin_shortcut_capture_flow(app);
            prefetch_freeze_frame(app, &state, false);
        }
    }
    if !should_trigger_shortcut(&WIN_SHIFT_S_ARMED, event_state) {
        return;
    }
    if shortcut_release_was_cancelled(app) {
        return;
    }
    if preferences_or_onboarding_focused {
        abort_prefetched_freeze_capture(app);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        open_capture_controls(&app, CaptureSelectorMode::Screenshot);
    });
}

#[cfg(target_os = "windows")]
fn dispatch_recording_shortcut(app: &AppHandle, target: CaptureMode, event_state: ShortcutState) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if !state.settings().onboarding_completed {
        if event_state == ShortcutState::Released {
            show_onboarding(app);
        }
        return;
    }
    let suppressed = shortcut_capture_is_suppressed(app, &state);
    if event_state == ShortcutState::Pressed {
        clear_shortcut_capture_flow();
        if !suppressed
            && freeze_prefetch_is_allowed_for_selector(app, &state)
            && !recording::recording_session_is_active(&state)
        {
            let _ = begin_shortcut_capture_flow(app);
            prefetch_freeze_frame(app, &state, false);
        }
    }
    let trigger_is_suppressed =
        track_shortcut_suppression(&WIN_SHIFT_S_SUPPRESSED, event_state, suppressed);
    if !should_trigger_shortcut(&WIN_SHIFT_S_ARMED, event_state) || trigger_is_suppressed {
        if event_state == ShortcutState::Released && trigger_is_suppressed {
            clear_shortcut_capture_flow();
            abort_prefetched_freeze_capture(app);
        }
        return;
    }
    if shortcut_release_was_cancelled(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = recording::prepare_capture_selector_inner(
            app.clone(),
            state,
            CaptureSelectorMode::Recording,
            target,
        )
        .await
            && !matches!(
                &error,
                AppError::CaptureInProgress | AppError::ScreenshotCancelled
            )
        {
            report_recording_error(&app, &error);
        }
    });
}

#[cfg(target_os = "windows")]
fn dispatch_capture_shortcut(app: &AppHandle, mode: CaptureMode, event_state: ShortcutState) {
    let armed = &WIN_SHIFT_S_ARMED;
    let suppressed_while_pressed = &WIN_SHIFT_S_SUPPRESSED;
    let state = app.state::<Arc<AppState>>().inner().clone();
    if !state.settings().onboarding_completed {
        if event_state == ShortcutState::Released {
            show_onboarding(app);
        }
        return;
    }
    let suppressed = shortcut_capture_is_suppressed(app, &state);
    if event_state == ShortcutState::Pressed {
        clear_shortcut_capture_flow();
        if !suppressed
            && !recording::screenshot_capture_is_blocked(&state)
            && !screenshot_countdown_is_active(&state)
        {
            prepare_capture_shortcut_press(app, &state, mode);
        }
    }
    let trigger_is_suppressed =
        track_shortcut_suppression(suppressed_while_pressed, event_state, suppressed);
    if !should_trigger_shortcut(armed, event_state) || trigger_is_suppressed {
        if event_state == ShortcutState::Released && trigger_is_suppressed {
            clear_shortcut_capture_flow();
            cancel_capture_shortcut_press(app, mode);
        }
        return;
    }
    if shortcut_release_was_cancelled(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if mode == CaptureMode::Display && !recording::recording_session_is_active(&state) {
            open_capture_controls_with_target(
                &app,
                CaptureSelectorMode::Screenshot,
                CaptureMode::Display,
            );
            return;
        }
        if let Err(error) = start_capture_inner(app.clone(), state, mode).await
            && !matches!(
                &error,
                AppError::CaptureInProgress | AppError::ScreenshotCancelled
            )
        {
            report_capture_error(&app, &error, mode);
        }
    });
}

#[cfg(target_os = "windows")]
fn disable_windows_print_screen_snipping() {
    if let Err(error) = set_windows_print_screen_snipping_enabled(false) {
        eprintln!("could not disable Windows Print Screen snipping: {error}");
    }
}

#[cfg(target_os = "windows")]
fn set_windows_print_screen_snipping_enabled(enabled: bool) -> Result<(), String> {
    let value = if enabled { "1" } else { "0" };
    for key in [
        r"HKCU\Control Panel\Keyboard",
        r"HKCU\Control Panel\Accessibility",
    ] {
        let output = Command::new("reg")
            .args([
                "add",
                key,
                "/v",
                "PrintScreenKeyForSnippingEnabled",
                "/t",
                "REG_DWORD",
                "/d",
                value,
                "/f",
            ])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            return Err(if stderr.is_empty() { stdout } else { stderr });
        }
    }
    Ok(())
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

fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let settings = handle.state::<Arc<AppState>>().settings();
    let status = updates::current_status(&handle);
    let menu = build_tray_menu(&handle, &settings, &status)?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ICON_ID)
        .menu(&menu)
        .tooltip(updates::tray_tooltip(&status));

    #[cfg(target_os = "macos")]
    if let Some(icon) = macos_tray_icon() {
        tray = tray.icon(icon).icon_as_template(true);
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    // Windows hides the icon in the tray overflow; left-click opens
    // Preferences so Search is not the only way to find settings.
    #[cfg(target_os = "windows")]
    {
        use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
        tray = tray
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_preferences(tray.app_handle());
                }
            });
    }

    tray.on_menu_event(|app, event| match event.id().as_ref() {
        "new-capture" => {
            open_capture_controls(app, CaptureSelectorMode::Screenshot);
        }
        "capture-region" => start_capture_from_tray(app, CaptureMode::Region),
        "capture-window" => start_capture_from_tray(app, CaptureMode::Window),
        "capture-display" => start_capture_from_tray(app, CaptureMode::Display),
        "record-region" => start_recording_from_tray(app, CaptureMode::Region),
        "record-window" => start_recording_from_tray(app, CaptureMode::Window),
        "record-display" => start_recording_from_tray(app, CaptureMode::Display),
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
        "send-feedback" => {
            feedback::show_feedback(app);
        }
        "check-updates" => {
            updates::handle_tray_action(app);
        }
        "quit" => {
            crash_report::mark_clean_exit();
            app.exit(0);
        }
        _ => {}
    })
    .build(app)?;

    Ok(())
}

fn start_capture_from_tray(app: &AppHandle, mode: CaptureMode) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if !state.settings().onboarding_completed {
        show_onboarding(app);
        return;
    }
    let recapture_ui = should_recapture_visible_capture_ui(app, &state, mode);
    if should_claim_region_cursor_on_shortcut_press(
        mode,
        state.settings().freeze_screen,
        recapture_ui,
    ) {
        claim_region_capture_cursor(app);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if mode == CaptureMode::Display && !recording::recording_session_is_active(&state) {
            open_capture_controls_with_target(
                &app,
                CaptureSelectorMode::Screenshot,
                CaptureMode::Display,
            );
            return;
        }
        if let Err(error) = start_capture_inner(app.clone(), state, mode).await
            && !matches!(
                &error,
                AppError::CaptureInProgress | AppError::ScreenshotCancelled
            )
        {
            report_capture_error(&app, &error, mode);
        }
    });
}

fn start_recording_from_tray(app: &AppHandle, target: CaptureMode) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if !state.settings().onboarding_completed {
        show_onboarding(app);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = recording::prepare_capture_selector_inner(
            app.clone(),
            state,
            CaptureSelectorMode::Recording,
            target,
        )
        .await
            && !matches!(
                &error,
                AppError::CaptureInProgress | AppError::ScreenshotCancelled
            )
        {
            report_recording_error(&app, &error);
        }
    });
}

pub(crate) fn refresh_tray_menu(app: &AppHandle) {
    let app = app.clone();
    let dispatch = app.clone();
    if let Err(error) = dispatch.run_on_main_thread(move || {
        if let Err(error) = refresh_tray_menu_on_main(&app) {
            eprintln!("failed to refresh the tray menu: {error}");
        }
    }) {
        eprintln!("failed to schedule tray menu refresh: {error}");
    }
}

fn refresh_tray_menu_on_main(app: &AppHandle) -> Result<(), tauri::Error> {
    let Some(tray) = app.tray_by_id(TRAY_ICON_ID) else {
        return Ok(());
    };
    let settings = app.state::<Arc<AppState>>().settings();
    let status = updates::current_status(app);
    let menu = build_tray_menu(app, &settings, &status)?;
    tray.set_menu(Some(menu))?;
    let _ = tray.set_tooltip(Some(updates::tray_tooltip(&status)));
    Ok(())
}

fn build_tray_menu(
    app: &AppHandle,
    settings: &AppSettings,
    status: &updates::UpdateStatus,
) -> Result<Menu<tauri::Wry>, tauri::Error> {
    let update = updates::tray_update_item(status);
    let new_capture = tray_menu_item(
        app,
        "new-capture",
        "New Capture…",
        Some(&settings.new_capture_shortcut),
    )?;
    let capture_region = tray_menu_item(
        app,
        "capture-region",
        "Screenshot Region",
        Some(&settings.region_shortcut),
    )?;
    let capture_window = tray_menu_item(
        app,
        "capture-window",
        "Screenshot Window",
        Some(&settings.window_shortcut),
    )?;
    let capture_display = tray_menu_item(
        app,
        "capture-display",
        "Screenshot Display",
        Some(&settings.display_shortcut),
    )?;
    let record_region = tray_menu_item(
        app,
        "record-region",
        "Record Region",
        Some(&settings.recording.video_shortcut),
    )?;
    let record_window = tray_menu_item(
        app,
        "record-window",
        "Record Window",
        Some(&settings.recording.window_shortcut),
    )?;
    let record_display = tray_menu_item(
        app,
        "record-display",
        "Record Display",
        Some(&settings.recording.display_shortcut),
    )?;
    let capture_history = tray_menu_item(app, "capture-history", "Capture History…", None)?;
    let open_folder = tray_menu_item(app, "open-folder", "Open Save Location", None)?;
    let preferences = tray_menu_item(app, "preferences", "Preferences", None)?;
    let send_feedback = tray_menu_item(app, "send-feedback", "Send Feedback…", None)?;
    let update_item = MenuItem::with_id(
        app,
        "check-updates",
        &update.label,
        update.enabled,
        None::<&str>,
    )?;
    let quit = tray_menu_item(app, "quit", "Quit Captures", None)?;
    let sep_top = PredefinedMenuItem::separator(app)?;
    let sep_mid = PredefinedMenuItem::separator(app)?;
    let sep_bot = PredefinedMenuItem::separator(app)?;
    let menu = Menu::new(app)?;

    if update.pin_first {
        menu.append(&update_item)?;
        menu.append(&sep_top)?;
    }
    menu.append(&new_capture)?;
    menu.append(&capture_region)?;
    menu.append(&capture_window)?;
    menu.append(&capture_display)?;
    menu.append(&record_region)?;
    menu.append(&record_window)?;
    menu.append(&record_display)?;
    menu.append(if update.pin_first { &sep_mid } else { &sep_top })?;
    menu.append(&capture_history)?;
    menu.append(&open_folder)?;
    menu.append(&preferences)?;
    menu.append(&send_feedback)?;
    if !update.pin_first {
        menu.append(&update_item)?;
    }
    menu.append(&sep_bot)?;
    menu.append(&quit)?;
    Ok(menu)
}

fn tray_menu_item(
    app: &AppHandle,
    id: &str,
    text: &str,
    shortcut: Option<&str>,
) -> Result<MenuItem<tauri::Wry>, tauri::Error> {
    if let Some(accelerator) = shortcut.and_then(tray_accelerator)
        && let Ok(item) = MenuItem::with_id(app, id, text, true, Some(accelerator.as_str()))
    {
        return Ok(item);
    }
    MenuItem::with_id(app, id, text, true, None::<&str>)
}

fn tray_accelerator(shortcut: &str) -> Option<String> {
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        return None;
    }
    Some(
        shortcut
            .split('+')
            .map(|token| {
                let token = token.trim();
                let lower = token.to_ascii_lowercase();
                if lower.starts_with("digit") && token.len() == 6 {
                    token[5..].to_owned()
                } else if lower.starts_with("key") && token.len() == 4 {
                    token[3..].to_owned()
                } else {
                    token.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("+"),
    )
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
        if !overlay_session_is_live(&handle, &session.id) {
            return;
        }
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
            #[cfg(target_os = "macos")]
            if let Err(error) = captures_macos_window::cover_display(&window, &display.id) {
                eprintln!("failed to cover the capture display: {error}");
            }
            #[cfg(target_os = "macos")]
            {
                // Wake WKWebView at an imperceptible alpha before React sets the
                // snapshot src. A hidden or fully transparent webview defers
                // image loading, then the first opaque frame is an unpainted
                // black layer.
                captures_macos_window::remember_frontmost_app_before_activation();
                if let Err(error) = window.set_ignore_cursor_events(true) {
                    eprintln!("failed to ignore overlay cursor events while priming: {error}");
                }
                if let Err(error) = captures_macos_window::present_capture_overlay(&window) {
                    eprintln!("failed to present the capture overlay: {error}");
                }
            }
            #[cfg(target_os = "linux")]
            let _ = window.set_fullscreen(wayland_session());
            if !overlay_session_is_live(&handle, &session.id) {
                hide_capture_overlay_inner(&handle);
                return;
            }
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

const STARTUP_NOTICE_WIDTH: f64 = 240.0;
const STARTUP_NOTICE_HEIGHT: f64 = 36.0;
/// Transparent padding around the rounded card so `--shadow-md` is not clipped.
/// Dark `--shadow-md` is `0 8px 20px`, so the blur plus Y offset needs 28px.
const TRAY_NOTICE_FRAME_PAD: f64 = 28.0;
/// Extra window height reserved for the tray-pointing caret.
const TRAY_NOTICE_CARET_SIZE: f64 = 8.0;
/// Keep the caret off the rounded ends of the notice. The launch pill is 36px
/// tall, so its end-caps are 18px; half the 12px caret span is 6px more.
const TRAY_NOTICE_CARET_INSET: f64 = 24.0;
/// Pull the transparent window over the tray so the caret tip sits on the icon.
const TRAY_NOTICE_TRAY_OVERLAP: f64 = 2.0;
const TRAY_NOTICE_SCREEN_MARGIN: f64 = 10.0;
/// Status items and tray icons live in a thin band along a display edge.
/// After an update restart, AppKit can report the status item at the Cocoa
/// origin (bottom-left) before it has been placed in the menu bar.
const STARTUP_NOTICE_TRAY_EDGE_BAND: f64 = 56.0;
const STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS: u32 = 20;
const STARTUP_NOTICE_TRAY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);
const STARTUP_NOTICE_AUTOSTART_VISIBLE: std::time::Duration = std::time::Duration::from_secs(5);
/// After first-run setup, keep the tray hint up long enough to read.
const STARTUP_NOTICE_AFTER_SETUP_VISIBLE: std::time::Duration = std::time::Duration::from_secs(15);
const ONBOARDING_WINDOW_WIDTH: f64 = 620.0;
const ONBOARDING_WINDOW_HEIGHT: f64 = 560.0;

#[cfg(any(target_os = "macos", test))]
const MACOS_SCREEN_RECORDING_SETTINGS_URLS: &[&str] = &[
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture",
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
];
#[cfg(any(target_os = "macos", test))]
const MACOS_MICROPHONE_SETTINGS_URLS: &[&str] = &[
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone",
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
];

/// `--surface-canvas` for each appearance, used as the pre-paint background so
/// a new window does not flash the opposite theme before the webview loads.
const DOCUMENT_WINDOW_BACKGROUND_DARK: Color = Color(16, 16, 20, 255);
const DOCUMENT_WINDOW_BACKGROUND_LIGHT: Color = Color(245, 245, 247, 255);
/// Theme for the update and startup notices. The native window stays
/// transparent so rounded cards are not boxed by a rectangular fill.
const NOTICE_WINDOW_BACKGROUND: Color = Color(0, 0, 0, 0);

pub(crate) const NOTICE_CARET_EVENT: &str = "notice-caret";

pub(crate) fn resolve_appearance_is_dark(app: &AppHandle) -> bool {
    match app.state::<Arc<AppState>>().settings().appearance {
        Appearance::Light => false,
        Appearance::Dark => true,
        // The webview resolves "system" from `prefers-color-scheme`; mirror it
        // from an existing window where the platform reports one, else assume dark.
        Appearance::System => app
            .webview_windows()
            .values()
            .find_map(|window| window.theme().ok())
            .map(|theme| theme == Theme::Dark)
            .unwrap_or(true),
    }
}

/// Native title-bar theme and pre-paint fill for regular Captures windows.
pub(crate) fn document_window_chrome(app: &AppHandle) -> (Option<Theme>, Color) {
    if resolve_appearance_is_dark(app) {
        (Some(Theme::Dark), DOCUMENT_WINDOW_BACKGROUND_DARK)
    } else {
        (Some(Theme::Light), DOCUMENT_WINDOW_BACKGROUND_LIGHT)
    }
}

/// Transparent fill for the update and startup notices.
pub(crate) fn notice_window_chrome(app: &AppHandle) -> (Option<Theme>, Color) {
    if resolve_appearance_is_dark(app) {
        (Some(Theme::Dark), NOTICE_WINDOW_BACKGROUND)
    } else {
        (Some(Theme::Light), NOTICE_WINDOW_BACKGROUND)
    }
}

fn show_onboarding(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(ONBOARDING_WINDOW_LABEL) {
        if let Err(error) = reveal_and_focus_document_window(&window) {
            eprintln!("failed to reveal onboarding window: {error}");
        }
        return;
    }

    let app = app.clone();
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let (onboarding_theme, onboarding_background) = document_window_chrome(&handle);
        let result = WebviewWindowBuilder::new(
            &handle,
            ONBOARDING_WINDOW_LABEL,
            WebviewUrl::App("index.html?view=onboarding".into()),
        )
        .title("Captures")
        .inner_size(ONBOARDING_WINDOW_WIDTH, ONBOARDING_WINDOW_HEIGHT)
        .min_inner_size(480.0, 440.0)
        .center()
        .resizable(true)
        .theme(onboarding_theme)
        .background_color(onboarding_background)
        .focused(false)
        .visible(false)
        .on_page_load(document_window_page_load_handler(
            "failed to reveal onboarding window",
        ))
        .build();
        if let Err(error) = result {
            eprintln!("failed to show onboarding window: {error}");
        }
    }) {
        eprintln!("failed to schedule onboarding window: {error}");
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupNoticeCaret {
    None,
    Top,
    Bottom,
}

impl StartupNoticeCaret {
    fn as_query_value(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Top => Some("top"),
            Self::Bottom => Some("bottom"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StartupNoticePlacement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub caret: StartupNoticeCaret,
    pub caret_x: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct NoticeCaretPayload {
    pub edge: &'static str,
    pub x: f64,
}

pub(crate) fn notice_caret_payload(
    placement: &StartupNoticePlacement,
) -> Option<NoticeCaretPayload> {
    placement
        .caret
        .as_query_value()
        .map(|edge| NoticeCaretPayload {
            edge,
            x: placement.caret_x,
        })
}

fn show_startup_notice(app: &AppHandle, visible_for: std::time::Duration) {
    let app = app.clone();
    std::thread::spawn(move || {
        // `TrayIcon::rect` hops to the main thread and waits. Calling it from
        // setup() would deadlock, and the status item may not have a screen
        // rect until the event loop has run once. After an update restart the
        // menu bar can take several hundred milliseconds to assign a real frame.
        let _ = startup_notice_placement_with_retry(&app);
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            // Re-read on the main thread so we use the laid-out icon, not the
            // stale bottom-left origin captured while waiting.
            let placement = startup_notice_placement(&handle);
            if let Err(error) = create_startup_notice(&handle, placement, visible_for) {
                eprintln!("failed to show Captures launch notice: {error}");
            }
        }) {
            eprintln!("failed to schedule Captures launch notice: {error}");
        }
    });
}

fn startup_notice_placement_with_retry(app: &AppHandle) -> StartupNoticePlacement {
    let mut last = startup_notice_placement(app);
    if last.caret != StartupNoticeCaret::None || !should_retry_startup_notice_tray() {
        return last;
    }
    for _ in 0..STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS {
        std::thread::sleep(STARTUP_NOTICE_TRAY_RETRY_DELAY);
        last = startup_notice_placement(app);
        if last.caret != StartupNoticeCaret::None {
            return last;
        }
    }
    last
}

fn should_retry_startup_notice_tray() -> bool {
    // Linux AppIndicator never reports a tray rect; waiting would only delay
    // the fallback. macOS and Windows do report one once the icon is laid out.
    cfg!(any(target_os = "macos", target_os = "windows"))
}

fn create_startup_notice(
    app: &AppHandle,
    placement: StartupNoticePlacement,
    visible_for: std::time::Duration,
) -> Result<(), tauri::Error> {
    let (theme, background) = notice_window_chrome(app);
    let window = WebviewWindowBuilder::new(
        app,
        "startup",
        WebviewUrl::App(startup_notice_url(placement).into()),
    )
    .title("Captures is running")
    .inner_size(placement.width, placement.height)
    .position(placement.x, placement.y)
    .decorations(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .transparent(true)
    .theme(theme)
    .background_color(background)
    .focused(false)
    .visible(false)
    .build()?;
    set_click_through(&window, true)?;
    // Builder `.position` is not enough on macOS: a borderless NSWindow is
    // anchored at its bottom-left, and a hidden window can keep the default
    // origin (the bottom-left of the display). Size first, then position.
    apply_tray_notice_position(&window, placement)?;

    #[cfg(target_os = "macos")]
    captures_macos_window::show_without_activating(&window)
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;

    #[cfg(not(target_os = "macos"))]
    window.show()?;

    set_click_through(&window, true)?;
    apply_tray_notice_position(&window, placement)?;

    let timer_app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(visible_for);
        let handle = timer_app.clone();
        let _ = timer_app.run_on_main_thread(move || {
            if let Some(window) = handle.get_webview_window("startup") {
                let _ = window.hide();
            }
        });
    });
    Ok(())
}

pub(crate) fn apply_tray_notice_position(
    window: &tauri::WebviewWindow,
    placement: StartupNoticePlacement,
) -> Result<(), tauri::Error> {
    window.set_size(LogicalSize::new(placement.width, placement.height))?;
    window.set_position(tauri::LogicalPosition::new(placement.x, placement.y))?;
    Ok(())
}

fn startup_notice_url(placement: StartupNoticePlacement) -> String {
    tray_notice_url("startup", placement)
}

pub(crate) fn tray_notice_url(view: &str, placement: StartupNoticePlacement) -> String {
    match placement.caret.as_query_value() {
        Some(caret) => format!(
            "index.html?view={view}&caret={caret}&caret_x={}",
            placement.caret_x.round()
        ),
        None => format!("index.html?view={view}"),
    }
}

fn startup_notice_placement(app: &AppHandle) -> StartupNoticePlacement {
    tray_anchored_notice_placement(app, STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT)
}

pub(crate) fn tray_anchored_notice_placement(
    app: &AppHandle,
    card_width: f64,
    card_height: f64,
) -> StartupNoticePlacement {
    let tray = tray_icon_physical_rect(app);
    let monitor = tray
        .and_then(|(x, y, width, height)| {
            app.monitor_from_point(x + width / 2.0, y + height / 2.0)
                .ok()
                .flatten()
        })
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        let bounds = LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        };
        return resolve_tray_notice_placement(
            bounds,
            bounds,
            None,
            cfg!(target_os = "macos"),
            card_width,
            card_height,
        );
    };
    let scale = monitor.scale_factor().max(1.0);
    let tray = tray.map(|(x, y, width, height)| LogicalRect {
        x: x / scale,
        y: y / scale,
        width: width / scale,
        height: height / scale,
    });
    resolve_tray_notice_placement(
        monitor_logical_rect(&monitor),
        monitor_logical_work_area(&monitor),
        tray,
        cfg!(target_os = "macos"),
        card_width,
        card_height,
    )
}

fn tray_icon_physical_rect(app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
    let tray = app.tray_by_id(TRAY_ICON_ID)?;
    let rect = tray.rect().ok().flatten()?;
    // Already physical; scale is ignored for Physical variants.
    let position = rect.position.to_physical::<f64>(1.0);
    let size = rect.size.to_physical::<f64>(1.0);
    if size.width <= 0.0 || size.height <= 0.0 {
        return None;
    }
    Some((position.x, position.y, size.width, size.height))
}

fn monitor_logical_rect(monitor: &tauri::Monitor) -> LogicalRect {
    let scale = monitor.scale_factor().max(1.0);
    let position = monitor.position();
    let size = monitor.size();
    LogicalRect {
        x: f64::from(position.x) / scale,
        y: f64::from(position.y) / scale,
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
    }
}

fn monitor_logical_work_area(monitor: &tauri::Monitor) -> LogicalRect {
    let scale = monitor.scale_factor().max(1.0);
    let work_area = monitor.work_area();
    LogicalRect {
        x: f64::from(work_area.position.x) / scale,
        y: f64::from(work_area.position.y) / scale,
        width: f64::from(work_area.size.width) / scale,
        height: f64::from(work_area.size.height) / scale,
    }
}

fn tray_notice_window_size(card_width: f64, card_height: f64, with_caret: bool) -> (f64, f64) {
    let width = card_width + TRAY_NOTICE_FRAME_PAD * 2.0;
    let height = if with_caret {
        card_height + TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_SIZE
    } else {
        card_height + TRAY_NOTICE_FRAME_PAD * 2.0
    };
    (width, height)
}

fn tray_notice_caret_x(window_width: f64, window_x: f64, tray_center_x: f64) -> f64 {
    let min = TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_INSET;
    let max = (window_width - TRAY_NOTICE_FRAME_PAD - TRAY_NOTICE_CARET_INSET).max(min);
    (tray_center_x - window_x).clamp(min, max)
}

#[cfg(test)]
fn resolve_startup_notice_placement(
    monitor: LogicalRect,
    work_area: LogicalRect,
    tray: Option<LogicalRect>,
    menu_bar_at_top: bool,
) -> StartupNoticePlacement {
    resolve_tray_notice_placement(
        monitor,
        work_area,
        tray,
        menu_bar_at_top,
        STARTUP_NOTICE_WIDTH,
        STARTUP_NOTICE_HEIGHT,
    )
}

fn resolve_tray_notice_placement(
    monitor: LogicalRect,
    work_area: LogicalRect,
    tray: Option<LogicalRect>,
    menu_bar_at_top: bool,
    card_width: f64,
    card_height: f64,
) -> StartupNoticePlacement {
    let tray = tray.filter(|tray| tray_icon_rect_is_usable(monitor, *tray, menu_bar_at_top));
    match tray {
        Some(tray) => place_tray_notice(monitor, tray, card_width, card_height),
        None => fallback_tray_notice(
            monitor,
            work_area,
            if menu_bar_at_top {
                StartupNoticeCaret::Top
            } else {
                startup_notice_fallback_edge(monitor, work_area)
            },
            card_width,
            card_height,
        ),
    }
}

fn tray_icon_rect_is_usable(
    monitor: LogicalRect,
    tray: LogicalRect,
    menu_bar_at_top: bool,
) -> bool {
    if tray.width <= 0.0 || tray.height <= 0.0 {
        return false;
    }
    let center_x = tray.x + tray.width / 2.0;
    let center_y = tray.y + tray.height / 2.0;
    if center_x < monitor.x
        || center_x > monitor.x + monitor.width
        || center_y < monitor.y
        || center_y > monitor.y + monitor.height
    {
        return false;
    }
    let from_top = center_y - monitor.y;
    let from_bottom = monitor.y + monitor.height - center_y;
    if menu_bar_at_top {
        // macOS extras always sit in the menu bar. Reject the unlaid-out
        // status item at the Cocoa origin, which maps to the bottom-left.
        return from_top <= STARTUP_NOTICE_TRAY_EDGE_BAND;
    }
    let from_left = center_x - monitor.x;
    let from_right = monitor.x + monitor.width - center_x;
    from_top <= STARTUP_NOTICE_TRAY_EDGE_BAND
        || from_bottom <= STARTUP_NOTICE_TRAY_EDGE_BAND
        || from_left <= STARTUP_NOTICE_TRAY_EDGE_BAND
        || from_right <= STARTUP_NOTICE_TRAY_EDGE_BAND
}

fn startup_notice_fallback_edge(
    monitor: LogicalRect,
    work_area: LogicalRect,
) -> StartupNoticeCaret {
    #[cfg(target_os = "macos")]
    {
        let _ = (monitor, work_area);
        StartupNoticeCaret::Top
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (monitor, work_area);
        StartupNoticeCaret::Bottom
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    startup_notice_fallback_edge_from_insets(monitor, work_area)
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
fn startup_notice_fallback_edge_from_insets(
    monitor: LogicalRect,
    work_area: LogicalRect,
) -> StartupNoticeCaret {
    let top_inset = (work_area.y - monitor.y).max(0.0);
    let bottom_inset = (monitor.y + monitor.height - (work_area.y + work_area.height)).max(0.0);
    const DISTINCT_PANEL: f64 = 16.0;
    if bottom_inset >= DISTINCT_PANEL && bottom_inset > top_inset {
        StartupNoticeCaret::Bottom
    } else {
        StartupNoticeCaret::Top
    }
}

#[cfg(test)]
fn fallback_startup_notice(
    monitor: LogicalRect,
    work_area: LogicalRect,
    edge: StartupNoticeCaret,
) -> StartupNoticePlacement {
    fallback_tray_notice(
        monitor,
        work_area,
        edge,
        STARTUP_NOTICE_WIDTH,
        STARTUP_NOTICE_HEIGHT,
    )
}

fn fallback_tray_notice(
    monitor: LogicalRect,
    work_area: LogicalRect,
    edge: StartupNoticeCaret,
    card_width: f64,
    card_height: f64,
) -> StartupNoticePlacement {
    let (width, height) = tray_notice_window_size(card_width, card_height, false);
    let min_x = work_area.x + TRAY_NOTICE_SCREEN_MARGIN;
    let max_x = work_area.x + work_area.width - width - TRAY_NOTICE_SCREEN_MARGIN;
    let x =
        (work_area.x + work_area.width - width - 18.0).clamp(min_x.min(max_x), max_x.max(min_x));
    let unclamped_y = match edge {
        StartupNoticeCaret::Bottom => work_area.y + work_area.height - height - 18.0,
        StartupNoticeCaret::Top | StartupNoticeCaret::None => {
            // Work area already excludes a menu bar / top panel. Otherwise keep
            // the historical inset that clears a 24–30px menu bar.
            if work_area.y > monitor.y + 1.0 {
                work_area.y + TRAY_NOTICE_SCREEN_MARGIN
            } else {
                monitor.y + 30.0
            }
        }
    };
    let min_y = work_area.y + TRAY_NOTICE_SCREEN_MARGIN;
    let max_y = work_area.y + work_area.height - height - TRAY_NOTICE_SCREEN_MARGIN;
    StartupNoticePlacement {
        x,
        y: unclamped_y.clamp(min_y.min(max_y), max_y.max(min_y)),
        width,
        height,
        caret: StartupNoticeCaret::None,
        caret_x: width / 2.0,
    }
}

#[cfg(test)]
fn place_startup_notice(monitor: LogicalRect, tray: Option<LogicalRect>) -> StartupNoticePlacement {
    match tray {
        Some(tray) => place_tray_notice(monitor, tray, STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT),
        None => fallback_startup_notice(monitor, monitor, StartupNoticeCaret::Top),
    }
}

fn place_tray_notice(
    monitor: LogicalRect,
    tray: LogicalRect,
    card_width: f64,
    card_height: f64,
) -> StartupNoticePlacement {
    let fallback = fallback_tray_notice(
        monitor,
        monitor,
        StartupNoticeCaret::Top,
        card_width,
        card_height,
    );
    if tray.width <= 0.0 || tray.height <= 0.0 {
        return fallback;
    }

    let tray_center_x = tray.x + tray.width / 2.0;
    let tray_center_y = tray.y + tray.height / 2.0;
    let caret = if tray_center_y <= monitor.y + monitor.height / 2.0 {
        StartupNoticeCaret::Top
    } else {
        StartupNoticeCaret::Bottom
    };
    let (width, height) = tray_notice_window_size(card_width, card_height, true);
    let min_x = monitor.x + TRAY_NOTICE_SCREEN_MARGIN;
    let max_x = monitor.x + monitor.width - width - TRAY_NOTICE_SCREEN_MARGIN;
    let x = (tray_center_x - width / 2.0).clamp(min_x.min(max_x), max_x.max(min_x));
    let unclamped_y = match caret {
        StartupNoticeCaret::Top => tray.y + tray.height - TRAY_NOTICE_TRAY_OVERLAP,
        StartupNoticeCaret::Bottom => tray.y - height + TRAY_NOTICE_TRAY_OVERLAP,
        StartupNoticeCaret::None => fallback.y,
    };
    let min_y = monitor.y + TRAY_NOTICE_SCREEN_MARGIN;
    let max_y = monitor.y + monitor.height - height - TRAY_NOTICE_SCREEN_MARGIN;
    let y = unclamped_y.clamp(min_y.min(max_y), max_y.max(min_y));

    StartupNoticePlacement {
        x,
        y,
        width,
        height,
        caret,
        caret_x: tray_notice_caret_x(width, x, tray_center_x),
    }
}

const RECORDING_SAVED_NOTICE_WIDTH: f64 = 440.0;
const RECORDING_SAVED_NOTICE_HEIGHT: f64 = 116.0;
const RECORDING_CONTROLS_HIDDEN_NOTICE_CARD_WIDTH: f64 = 418.0;
const RECORDING_CONTROLS_HIDDEN_NOTICE_CARD_HEIGHT: f64 = 74.0;
/// Transparent padding around the notice card so its glass shadow is not clipped
/// by the transparent native window.
const RECORDING_CONTROLS_HIDDEN_NOTICE_FRAME_PAD: f64 = 28.0;
const RECORDING_CONTROLS_HIDDEN_NOTICE_WIDTH: f64 =
    RECORDING_CONTROLS_HIDDEN_NOTICE_CARD_WIDTH + RECORDING_CONTROLS_HIDDEN_NOTICE_FRAME_PAD * 2.0;
const RECORDING_CONTROLS_HIDDEN_NOTICE_HEIGHT: f64 =
    RECORDING_CONTROLS_HIDDEN_NOTICE_CARD_HEIGHT + RECORDING_CONTROLS_HIDDEN_NOTICE_FRAME_PAD * 2.0;

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
    #[cfg(target_os = "macos")]
    {
        captures_macos_window::configure_inactive_hover(&window)
            .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;
        captures_macos_window::show_without_activating(&window)
            .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;
    }

    #[cfg(not(target_os = "macos"))]
    window.show()?;
    let _ = set_window_content_protected(&window, true);

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
                let _ = set_window_content_protected(&window, false);
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
    set_click_through(&window, true)?;

    #[cfg(target_os = "macos")]
    captures_macos_window::show_without_activating(&window)
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;

    #[cfg(not(target_os = "macos"))]
    window.show()?;
    let _ = set_window_content_protected(&window, true);

    set_click_through(&window, true)?;

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
    let placement = app
        .state::<Arc<AppState>>()
        .settings()
        .mini_preview_placement;
    let geometry = thumbnail_window_geometry(app, 1, false, None, placement);
    let window = WebviewWindowBuilder::new(
        app,
        "thumbnail",
        WebviewUrl::App("index.html?view=thumbnail".into()),
    )
    .title("Captures")
    .inner_size(THUMBNAIL_WIDTH, geometry.height)
    .position(geometry.x, geometry.y)
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
    // Keep the browser or desktop app underneath active while the user hovers,
    // clicks actions, or starts a file drag from a mini preview. Mouse events
    // still reach the WebView; this only prevents native window activation.
    .focusable(false)
    .focused(false)
    .visible(false)
    .build()?;

    #[cfg(target_os = "macos")]
    captures_macos_window::configure_thumbnail_inactive_hover(&window)
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
const THUMBNAIL_CONTROL_GUTTER: f64 = 52.0;

fn update_thumbnail_stack(app: &AppHandle) {
    let app = app.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let state = handle.state::<Arc<AppState>>().inner().clone();
        let count = state.artifacts.lock().len();
        let settings = state.settings();
        let show_mini_previews = settings.show_mini_previews;
        let include_mini_previews_in_captures = settings.include_mini_previews_in_captures;
        let placement = settings.mini_preview_placement;
        let (suppressed, collapsed, origin) = {
            let mut visibility = state.thumbnail_visibility.lock();
            if count == 0 || !show_mini_previews {
                visibility.reset_session_placement();
            }
            (
                visibility.is_suppressed(),
                visibility.is_collapsed(),
                visibility.stack_origin(),
            )
        };
        let show_stack = thumbnail_stack_should_be_visible(
            count,
            suppressed,
            show_mini_previews,
            include_mini_previews_in_captures,
        );
        update_thumbnail_stack_window(&handle, count, show_stack, collapsed, origin, placement);
    });
}

fn update_thumbnail_stack_window(
    handle: &AppHandle,
    count: usize,
    show_stack: bool,
    collapsed: bool,
    origin: Option<ThumbnailStackOrigin>,
    placement: MiniPreviewPlacement,
) {
    let Some(window) = handle.get_webview_window("thumbnail") else {
        if let Err(error) = create_thumbnail_window(handle, show_stack) {
            eprintln!("failed to create capture thumbnail stack: {error}");
        }
        return;
    };
    if !show_stack {
        hide_thumbnail_window(&window);
        return;
    }
    let geometry = thumbnail_window_geometry(handle, count, collapsed, origin, placement);
    let visible = window.is_visible().unwrap_or(false);
    let presented = thumbnail_window_is_presented(&window);
    // WKWebView blanks painted cards when its visible NSWindow shrinks. macOS
    // keeps the frame in every mode; Linux keeps it while collapsed. Precise
    // hit testing keeps the retained empty area click-through on both platforms.
    let height = thumbnail_visible_window_height(
        geometry.height,
        visible
            .then(|| thumbnail_window_logical_height(&window))
            .flatten(),
        thumbnail_preserve_current_height(collapsed),
    );
    let y = thumbnail_window_top(geometry.y, height, geometry.height, geometry.anchor);
    if visible {
        #[cfg(target_os = "macos")]
        {
            let resize = if geometry.anchor.is_top() {
                captures_macos_window::resize_from_top(&window, THUMBNAIL_WIDTH, height)
            } else {
                captures_macos_window::resize_from_bottom(&window, THUMBNAIL_WIDTH, height)
            };
            if let Err(error) = resize {
                eprintln!("failed to resize capture thumbnail stack: {error}");
                let _ = window.set_size(LogicalSize::new(THUMBNAIL_WIDTH, height));
            }
            let _ = window.set_position(tauri::LogicalPosition::new(geometry.x, y));
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = window.set_position(tauri::LogicalPosition::new(geometry.x, y));
            let _ = window.set_size(LogicalSize::new(THUMBNAIL_WIDTH, height));
        }
    } else {
        let _ = window.set_size(LogicalSize::new(THUMBNAIL_WIDTH, height));
        let _ = window.set_position(tauri::LogicalPosition::new(geometry.x, y));
    }
    if !presented {
        show_thumbnail_window(&window);
    }
}

fn thumbnail_stack_should_be_visible(
    count: usize,
    suppressed: bool,
    show_mini_previews: bool,
    include_mini_previews_in_captures: bool,
) -> bool {
    // Capture flows suppress the stack so it does not appear in screenshots or
    // recordings. Opting in keeps it visible for self-capture / feedback.
    count > 0 && show_mini_previews && (!suppressed || include_mini_previews_in_captures)
}

fn thumbnail_window_logical_height(window: &tauri::WebviewWindow) -> Option<f64> {
    let scale = window.scale_factor().ok()?.max(1.0);
    let size = window.inner_size().ok()?;
    Some(f64::from(size.height) / scale)
}

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

fn thumbnail_preserve_current_height(collapsed: bool) -> bool {
    cfg!(target_os = "macos") || (cfg!(target_os = "linux") && collapsed)
}

fn thumbnail_webview_needs_tauri_show(is_visible: bool) -> bool {
    !is_visible
}

fn thumbnail_window_is_presented(window: &tauri::WebviewWindow) -> bool {
    if !window.is_visible().unwrap_or(false) {
        return false;
    }
    #[cfg(target_os = "macos")]
    return captures_macos_window::thumbnail_is_presented();
    #[cfg(not(target_os = "macos"))]
    true
}

fn hide_thumbnail_window(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let window = window.clone();
        if run_on_appkit_main(move || hide_thumbnail_window_inner(&window)).is_none() {
            eprintln!("failed to hide the capture thumbnail on the main thread");
        }
    }
    #[cfg(not(target_os = "macos"))]
    hide_thumbnail_window_inner(window);
}

fn hide_thumbnail_window_inner(window: &tauri::WebviewWindow) {
    // Click-through first so a transparent always-on-top window cannot keep
    // eating desktop clicks while hide() is still committing (Windows).
    let _ = set_click_through(window, true);
    // Ordering out a key-capable nonactivating panel donates key status to the
    // next Captures window — usually an open editor — and can activate the app
    // over Chrome. Keep the click-through panel ordered onscreen at zero alpha;
    // showing it again only needs a native reveal, not another focus handoff.
    #[cfg(target_os = "macos")]
    if let Err(error) = captures_macos_window::conceal_thumbnail_without_hiding(window) {
        eprintln!("failed to conceal the capture thumbnail: {error}");
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.hide();
        let _ = set_window_content_protected(window, false);
    }
}

fn show_thumbnail_window(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let window = window.clone();
        if run_on_appkit_main(move || show_thumbnail_window_inner(&window)).is_none() {
            eprintln!("failed to show the capture thumbnail on the main thread");
        }
    }
    #[cfg(not(target_os = "macos"))]
    show_thumbnail_window_inner(window);
}

fn show_thumbnail_window_inner(window: &tauri::WebviewWindow) {
    // Showing after a capture re-arms hit testing because the new card is
    // typically under the pointer. Sleep/resume recovery must not do this:
    // a preserved-height collapsed window would cover other apps. The JS
    // hover poll then re-applies ignore-cursor for empty stack chrome.
    let _ = set_click_through(window, false);
    // Tauri's hide pauses the WebView lifecycle on macOS. Resume it through
    // Tauri before raising the native panel so React hover and IPC polling do
    // not remain frozen after a capture hides the stack. Skip when already
    // visible: `show()` can activate Captures and yank an open editor forward
    // after a mini-preview delete/dismiss.
    if thumbnail_webview_needs_tauri_show(window.is_visible().unwrap_or(false)) {
        #[cfg(target_os = "macos")]
        captures_macos_window::run_without_stealing_activation(|| {
            let _ = window.show();
        });
        #[cfg(not(target_os = "macos"))]
        let _ = window.show();
    }

    #[cfg(target_os = "macos")]
    if let Err(error) = captures_macos_window::show_thumbnail_without_activating(window) {
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
    restore_excluded_recording_chrome(app);
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
    if !state
        .thumbnail_visibility
        .lock()
        .mark_artifact_ready(&artifact_id)
    {
        return Ok(());
    }
    update_thumbnail_stack(&app);
    Ok(())
}

#[tauri::command]
fn sync_thumbnail_stack(app: AppHandle) -> CommandResult<()> {
    update_thumbnail_stack(&app);
    Ok(())
}

#[tauri::command]
fn set_mini_previews_collapsed(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    collapsed: bool,
) -> CommandResult<()> {
    if state.artifacts.lock().is_empty() || !state.settings().show_mini_previews {
        return Ok(());
    }
    if collapsed {
        state.thumbnail_visibility.lock().collapse();
    } else {
        state.thumbnail_visibility.lock().expand();
    }
    update_thumbnail_stack(&app);
    Ok(())
}

fn thumbnail_window_geometry(
    app: &AppHandle,
    count: usize,
    collapsed: bool,
    origin: Option<ThumbnailStackOrigin>,
    placement: MiniPreviewPlacement,
) -> ThumbnailWindowGeometry {
    thumbnail_monitor_bounds(app)
        .map(|bounds| thumbnail_geometry(bounds, count, collapsed, origin, placement))
        .unwrap_or(ThumbnailWindowGeometry {
            x: 20.0,
            y: 20.0,
            height: thumbnail_stack_height(count, collapsed),
            anchor: ThumbnailStackAnchor::from(placement),
        })
}

fn thumbnail_monitor_bounds(app: &AppHandle) -> Option<ThumbnailMonitorBounds> {
    app.primary_monitor().ok().flatten().map(|monitor| {
        // Prefer the usable desktop (work area). Full monitor bounds include
        // reserved UI such as the Windows taskbar, macOS Dock, and Linux panels.
        let work_area = monitor.work_area();
        let full_position = *monitor.position();
        let full_size = *monitor.size();
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
        }
    })
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ThumbnailStackPosition {
    x: f64,
    y: f64,
}

#[tauri::command]
fn set_mini_preview_stack_position(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    x: f64,
    y: f64,
    anchor: Option<ThumbnailStackAnchor>,
) -> CommandResult<ThumbnailStackPosition> {
    if state.artifacts.lock().is_empty() || !state.settings().show_mini_previews {
        return Err("mini previews are not available".to_owned());
    }
    if !state.thumbnail_visibility.lock().is_collapsed() {
        return Err("mini previews are not collapsed".to_owned());
    }
    let Some(window) = app.get_webview_window("thumbnail") else {
        return Err("mini preview window is unavailable".to_owned());
    };
    let Some(bounds) = thumbnail_monitor_bounds(&app) else {
        return Err("display work area is unavailable".to_owned());
    };
    let work = thumbnail_work_area(bounds);
    let count = state.artifacts.lock().len().max(1);
    let content_height = thumbnail_stack_height(count, true);
    let frame_height = thumbnail_window_logical_height(&window).unwrap_or(content_height);
    let anchor = anchor.unwrap_or(ThumbnailStackAnchor::Bottom);
    let (x, y) = thumbnail_clamp_aligned_frame(x, y, frame_height, content_height, work, anchor);
    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    state
        .thumbnail_visibility
        .lock()
        .set_stack_origin(ThumbnailStackOrigin {
            x,
            edge: if anchor.is_top() { y } else { y + frame_height },
            anchor,
        });
    Ok(ThumbnailStackPosition { x, y })
}

fn thumbnail_stack_pose_depth(depth: f64) -> f64 {
    // Keep in sync with thumbnailStackPoseDepth in thumbnailLayout.ts.
    const RECEDE: f64 = 0.55;
    const EASE_K: f64 = 24.0;
    if depth <= 0.0 {
        0.0
    } else {
        depth * (EASE_K + RECEDE * depth) / (depth + EASE_K)
    }
}

fn thumbnail_collapsed_peek(count: usize, hovered: bool) -> f64 {
    let extra = count.saturating_sub(1) as f64;
    let pose = thumbnail_stack_pose_depth(extra);
    // Keep in sync with THUMBNAIL_STACK_IDLE_PEEK_PX / HOVER_PEEK_PX.
    pose * if hovered { 16.0 } else { 13.0 }
}

fn thumbnail_stack_height(count: usize, collapsed: bool) -> f64 {
    if collapsed {
        let peek = thumbnail_collapsed_peek(count.max(1), true);
        let extra_above_padding = (peek - THUMBNAIL_PADDING).max(0.0);
        return THUMBNAIL_PADDING
            + THUMBNAIL_CONTROL_GUTTER
            + THUMBNAIL_CARD_HEIGHT
            + extra_above_padding;
    }
    let cards = count.max(1) as f64;
    THUMBNAIL_PADDING
        + THUMBNAIL_CONTROL_GUTTER
        + cards * THUMBNAIL_CARD_HEIGHT
        + (cards - 1.0) * THUMBNAIL_GAP
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

#[derive(Clone, Copy, Debug)]
struct ThumbnailWorkArea {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    top_gap: f64,
    bottom_gap: f64,
}

fn thumbnail_work_area(bounds: ThumbnailMonitorBounds) -> ThumbnailWorkArea {
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

    ThumbnailWorkArea {
        left,
        top,
        width,
        height,
        top_gap: THUMBNAIL_SYSTEM_CHROME_GAP,
        bottom_gap: THUMBNAIL_SYSTEM_CHROME_GAP,
    }
}

/// Keep the visible pile in the work area.
///
/// Collapsed macOS/Linux windows stay at their expanded height so WebKit does
/// not blank cards. Bottom piles sit at the bottom of that frame (empty chrome
/// may leave the work area above so the stack can reach the top). Top piles
/// sit at the top so peek-down has room; empty chrome may leave below so the
/// stack can still reach the bottom.
fn thumbnail_clamp_aligned_frame(
    x: f64,
    y: f64,
    frame_height: f64,
    content_height: f64,
    work: ThumbnailWorkArea,
    anchor: ThumbnailStackAnchor,
) -> (f64, f64) {
    let content_height = content_height.min(frame_height).max(0.0);
    let slack = (frame_height - content_height).max(0.0);
    let min_x = work.left;
    let max_x = (work.left + work.width - THUMBNAIL_WIDTH).max(min_x);
    let (min_y, max_y) = if anchor.is_top() {
        let min_y = work.top;
        let max_y = (work.top + work.height - work.bottom_gap - content_height).max(min_y);
        (min_y, max_y)
    } else {
        let min_y = work.top - slack;
        let max_y = (work.top + work.height - work.bottom_gap - frame_height).max(min_y);
        (min_y, max_y)
    };
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ThumbnailWindowGeometry {
    x: f64,
    y: f64,
    height: f64,
    anchor: ThumbnailStackAnchor,
}

fn thumbnail_window_top(
    desired_y: f64,
    frame_height: f64,
    content_height: f64,
    anchor: ThumbnailStackAnchor,
) -> f64 {
    if anchor.is_top() {
        desired_y
    } else {
        desired_y - (frame_height - content_height)
    }
}

fn thumbnail_geometry(
    bounds: ThumbnailMonitorBounds,
    count: usize,
    collapsed: bool,
    origin: Option<ThumbnailStackOrigin>,
    placement: MiniPreviewPlacement,
) -> ThumbnailWindowGeometry {
    let work = thumbnail_work_area(bounds);
    let available_height = (work.height - work.bottom_gap - THUMBNAIL_PADDING).max(1.0);
    let stack_height = thumbnail_stack_height(count, collapsed).min(available_height);
    let default_x = if placement.is_right() {
        (work.left + work.width - THUMBNAIL_WIDTH).max(work.left)
    } else {
        work.left
            .min(work.left + work.width - THUMBNAIL_WIDTH)
            .max(work.left)
    };
    let default_anchor = ThumbnailStackAnchor::from(placement);
    let (x, desired_y, anchor) = match origin {
        Some(origin) => {
            let desired_y = if origin.anchor.is_top() {
                origin.edge
            } else {
                origin.edge - stack_height
            };
            (origin.x, desired_y, origin.anchor)
        }
        None => {
            let desired_y = if default_anchor.is_top() {
                work.top + work.top_gap
            } else {
                work.top + work.height - work.bottom_gap - stack_height
            };
            (default_x, desired_y, default_anchor)
        }
    };
    let (x, y) =
        thumbnail_clamp_aligned_frame(x, desired_y, stack_height, stack_height, work, anchor);
    ThumbnailWindowGeometry {
        x,
        y,
        height: stack_height,
        anchor,
    }
}

fn report_capture_error(app: &AppHandle, error: &AppError, mode: CaptureMode) {
    if matches!(error, AppError::ScreenshotCancelled) {
        return;
    }
    eprintln!("capture failed: {error}");
    if matches!(error, AppError::Capture(CaptureError::SessionUnavailable)) {
        // Capture shortcuts and restored app launches can arrive while the desktop
        // is locked. Do not put capture UI or an error dialog over the lock screen.
        return;
    }
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
    if matches!(error, AppError::ScreenshotCancelled) {
        return;
    }
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
fn open_macos_privacy_settings(urls: &[&str]) -> Result<(), AppError> {
    let mut last_error = None;
    for url in urls {
        match Command::new("/usr/bin/open").arg(url).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => last_error = Some(format!("open {url} exited with {status}")),
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    Err(AppError::Task(last_error.unwrap_or_else(|| {
        "could not open System Settings".to_owned()
    })))
}

#[cfg(target_os = "macos")]
fn open_macos_screen_recording_settings() -> Result<(), AppError> {
    open_macos_privacy_settings(MACOS_SCREEN_RECORDING_SETTINGS_URLS)
}

#[cfg(target_os = "macos")]
fn open_macos_microphone_settings() -> Result<(), AppError> {
    open_macos_privacy_settings(MACOS_MICROPHONE_SETTINGS_URLS)
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

/// How long after the first `PageLoadEvent::Finished` a follow-up load may
/// still show and focus a new document window. Covers `about:blank` → app URL.
/// Later loads (failed capture-protocol media, WebView recovery) must not
/// yank an already-open editor over the user's other apps.
const DOCUMENT_WINDOW_LOAD_FOCUS_GRACE: Duration = Duration::from_millis(1_500);

pub(crate) fn should_focus_document_window_on_page_load(
    first_finished_at: Option<Instant>,
    now: Instant,
) -> bool {
    match first_finished_at {
        None => true,
        Some(at) => now.saturating_duration_since(at) < DOCUMENT_WINDOW_LOAD_FOCUS_GRACE,
    }
}

pub(crate) fn document_window_page_load_handler(
    failed_log: &'static str,
) -> impl Fn(tauri::WebviewWindow, tauri::webview::PageLoadPayload<'_>) + Send + Sync + 'static {
    let first_finished = std::sync::Mutex::new(None::<Instant>);
    move |window, payload| {
        if payload.event() != PageLoadEvent::Finished {
            return;
        }
        let now = Instant::now();
        let mut first = first_finished
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !should_focus_document_window_on_page_load(*first, now) {
            return;
        }
        if first.is_none() {
            *first = Some(now);
        }
        drop(first);
        if let Err(error) = reveal_and_focus_document_window(&window) {
            eprintln!("{failed_log}: {error}");
        }
    }
}

/// Show, unminimize, and focus a window so hover and cursor styles work
/// immediately after opening.
///
/// On macOS, Tauri `set_focus` calls `activateIgnoringOtherApps:`, which raises
/// every Captures window. Activate only the target window instead so a second
/// Edit click or an update notice does not also lift an already-open editor
/// over the user's other apps.
pub(crate) fn reveal_and_focus_document_window(
    window: &tauri::WebviewWindow,
) -> Result<(), tauri::Error> {
    window.show()?;
    window.unminimize()?;
    focus_single_window(window);
    Ok(())
}

/// Makes a visible window key without showing it again.
pub(crate) fn focus_single_window(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    schedule_document_window_activation(window);
    #[cfg(not(target_os = "macos"))]
    if let Err(error) = window.set_focus() {
        eprintln!("failed to focus window: {error}");
    }
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
        let (history_theme, history_background) = document_window_chrome(&handle);
        let result = WebviewWindowBuilder::new(
            &handle,
            "history",
            WebviewUrl::App("index.html?view=history".into()),
        )
        .title("Capture History")
        .inner_size(1_020.0, 720.0)
        .min_inner_size(640.0, 440.0)
        .center()
        .resizable(true)
        .theme(history_theme)
        .background_color(history_background)
        .focused(false)
        .visible(false)
        .on_page_load(document_window_page_load_handler(
            "failed to reveal capture history window",
        ))
        .build();
        if let Err(error) = result {
            eprintln!("failed to show capture history window: {error}");
        }
    });
}

/// True when the window label belongs to a screenshot or recording editor.
fn is_editor_window_label(label: &str) -> bool {
    label.starts_with(SCREENSHOT_EDITOR_WINDOW_PREFIX)
        || label.starts_with(RECORDING_EDITOR_WINDOW_PREFIX)
}

/// Tell mini-previews this editor no longer holds any layers (window closed).
///
/// `editor_id` matches the Tauri window label and the frontend presence id
/// (`screenshot-editor-{id}` / `recording-editor-{id}`).
fn clear_editor_layer_presence_for_window(window: &tauri::Window) {
    let label = window.label();
    if !is_editor_window_label(label) {
        return;
    }
    let payload = EditorLayerPresenceEvent {
        editor_id: label.to_owned(),
        artifact_ids: Vec::new(),
    };
    if let Err(error) = window
        .app_handle()
        .emit(EDITOR_LAYERS_CHANGED_EVENT, payload)
    {
        eprintln!("failed to clear editor layer presence for {label}: {error}");
    }
}

fn primary_app_window_priority(label: &str) -> Option<u8> {
    if label == ONBOARDING_WINDOW_LABEL {
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
    None
}

fn focus_or_show_primary_app_window(app: &AppHandle) {
    focus_primary_app_window(app);
}

fn focus_primary_app_window(app: &AppHandle) {
    let onboarding_completed = app
        .try_state::<Arc<AppState>>()
        .is_none_or(|state| state.settings().onboarding_completed);
    let restore_recording = restore_hidden_recording_controls_are_needed(app);
    let primary = app
        .webview_windows()
        .into_iter()
        .filter(|(_, window)| window.is_visible().unwrap_or(false))
        .filter_map(|(label, window)| {
            primary_app_window_priority(&label).map(|priority| (priority, window))
        })
        .min_by_key(|(priority, _)| *priority);
    match app_reactivation(onboarding_completed, restore_recording, primary.is_some()) {
        AppReactivation::ShowOnboarding => show_onboarding(app),
        AppReactivation::RestoreRecordingControls => {
            let _ = restore_hidden_recording_controls(app);
        }
        AppReactivation::FocusExisting => {
            if let Some((_, window)) = primary {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }
        AppReactivation::ShowPreferences => show_preferences(app),
    }
}

fn restore_hidden_recording_controls_are_needed(app: &AppHandle) -> bool {
    let recording_is_active = app
        .try_state::<Arc<AppState>>()
        .is_some_and(|state| recording::recording_controls_are_available(state.inner()));
    recording_is_active
        && app
            .get_webview_window("recording-hud")
            .is_some_and(|window| !window.is_visible().unwrap_or(false))
}

fn restore_hidden_recording_controls(app: &AppHandle) -> bool {
    if !restore_hidden_recording_controls_are_needed(app) {
        return false;
    }
    if let Some(window) = app.get_webview_window("recording-hud") {
        hide_recording_controls_hidden_notices(app);
        let _ = window.show();
        let _ = window.unminimize();
        // Hide cleared WDA_EXCLUDEFROMCAPTURE so NVIDIA Instant Replay can run.
        // Reapply after show so the restored HUD is not burned into recordings.
        let excluded = !include_recording_controls_in_captures(app);
        #[cfg(target_os = "macos")]
        if let Err(error) = captures_macos_window::set_excluded_from_capture(&window, excluded) {
            eprintln!("failed to restore recording controls capture sharing: {error}");
        }
        let _ = set_window_content_protected(&window, excluded);
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
    show_preferences_target(app, None);
}

fn preferences_url(target: Option<&str>) -> String {
    match known_preference_target(target) {
        Some(target) => format!("index.html?view=preferences&target={target}"),
        None => "index.html?view=preferences".to_owned(),
    }
}

fn known_preference_target(target: Option<&str>) -> Option<&str> {
    match target {
        Some(AUTO_START_PREFERENCE_TARGET) => Some(AUTO_START_PREFERENCE_TARGET),
        Some(RECORDING_CONTROLS_PREFERENCE_TARGET) => Some(RECORDING_CONTROLS_PREFERENCE_TARGET),
        _ => None,
    }
}

fn show_preferences_target(app: &AppHandle, target: Option<&str>) {
    let target = known_preference_target(target);
    if let Some(window) = app.get_webview_window("preferences") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(target) = target {
            let _ = window.emit(PREFERENCES_TARGET_EVENT, target);
        }
        return;
    }
    let app = app.clone();
    let handle = app.clone();
    let url = preferences_url(target);
    let _ = app.run_on_main_thread(move || {
        let (preferences_theme, preferences_background) = document_window_chrome(&handle);
        let result = WebviewWindowBuilder::new(&handle, "preferences", WebviewUrl::App(url.into()))
            .title("Captures Preferences")
            .inner_size(880.0, 660.0)
            .min_inner_size(560.0, 440.0)
            .center()
            .resizable(true)
            .theme(preferences_theme)
            .background_color(preferences_background)
            .focused(false)
            .visible(false)
            .on_page_load(document_window_page_load_handler(
                "failed to reveal preferences window",
            ))
            .build();
        if let Err(error) = result {
            eprintln!("failed to show preferences window: {error}");
        }
    });
}

pub(crate) fn hide_window(app: &AppHandle, label: &str) {
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        let hop_label = label.to_owned();
        if run_on_appkit_main(move || hide_window_inner(&app, &hop_label)).is_none() {
            eprintln!("failed to hide {label} on the main thread");
        }
    }
    #[cfg(not(target_os = "macos"))]
    hide_window_inner(app, label);
}

fn hide_window_inner(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        if label == "thumbnail" {
            hide_thumbnail_window_inner(&window);
            return;
        }
        let _ = window.hide();
        // Hidden HWNDs with WDA_EXCLUDEFROMCAPTURE still block NVIDIA Instant Replay.
        let _ = set_window_content_protected(&window, false);
    }
}

/// NVIDIA Instant Replay / ShadowPlay refuse desktop capture when any HWND in
/// the process has `WDA_EXCLUDEFROMCAPTURE` (Tauri `set_content_protected(true)`),
/// including hidden and off-screen windows. Only keep that affinity while the
/// window is actually on screen.
pub(crate) const fn windows_display_affinity_excludes_capture(
    excluded: bool,
    visible: bool,
) -> bool {
    excluded && visible
}

pub(crate) fn set_window_content_protected(
    window: &tauri::WebviewWindow,
    excluded: bool,
) -> tauri::Result<()> {
    // macOS/Linux keep exclusion on hidden windows. Windows must not: NVIDIA
    // Instant Replay treats WDA_EXCLUDEFROMCAPTURE on any HWND as DRM.
    let visible = {
        #[cfg(target_os = "windows")]
        {
            window.is_visible().unwrap_or(false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = window;
            true
        }
    };
    window.set_content_protected(windows_display_affinity_excludes_capture(excluded, visible))
}

pub(crate) const CAPTURE_HUD_HIDE_SETTLE_MS: u64 = 40;
static RESTORE_RECORDING_HUD_AFTER_CAPTURE: AtomicBool = AtomicBool::new(false);
static RESTORE_HIDDEN_CONTROLS_NOTICE_AFTER_CAPTURE: AtomicBool = AtomicBool::new(false);

const fn recording_chrome_should_restore_after_snapshot(
    already_marked: bool,
    chrome_visible: bool,
) -> bool {
    already_marked || chrome_visible
}

pub(crate) async fn hide_capture_huds_before_snapshot(app: &AppHandle) {
    let had_visible_hud = conceal_capture_chrome_for_snapshot(app);
    // Native hide/content-protection calls return before every compositor has
    // necessarily presented the new window state. Give a previously visible
    // HUD two frames to disappear before freezing the desktop background.
    if had_visible_hud {
        tokio::time::sleep(std::time::Duration::from_millis(CAPTURE_HUD_HIDE_SETTLE_MS)).await;
    }

    // A capture shortcut can fire while Start / Search are still on screen.
    // Wait until those flyouts have left so they do not freeze into the
    // screenshot. No-op on other platforms.
    let _ =
        tokio::task::spawn_blocking(captures_session::dismiss_transient_shell_ui_before_capture)
            .await;
}

fn conceal_capture_chrome_for_snapshot(app: &AppHandle) -> bool {
    let include_mini_previews = include_mini_previews_in_captures(app);
    let include_recording_controls = include_recording_controls_in_captures(app);
    let hide_update = updates::should_hide_update_notice_for_capture(app);
    set_capture_huds_protected(app, true);
    let mut hud_labels = if include_mini_previews {
        vec!["startup", "update", RECORDING_SAVED_NOTICE_LABEL]
    } else {
        vec![
            "thumbnail",
            "startup",
            "update",
            RECORDING_SAVED_NOTICE_LABEL,
        ]
    };
    if !include_recording_controls {
        hud_labels.push("recording-hud");
    }
    if !hide_update {
        hud_labels.retain(|label| *label != "update");
    }
    let had_visible_hud = hide_capture_huds(
        app,
        include_mini_previews,
        include_recording_controls,
        &hud_labels,
        hide_update,
    );
    if hide_update {
        updates::defer_visible_notice(app);
    }
    had_visible_hud
}

fn settle_concealed_capture_chrome(had_visible_hud: bool) {
    if had_visible_hud {
        std::thread::sleep(std::time::Duration::from_millis(CAPTURE_HUD_HIDE_SETTLE_MS));
    }
    captures_session::dismiss_transient_shell_ui_before_capture();
}

fn hide_capture_huds(
    app: &AppHandle,
    include_mini_previews: bool,
    include_recording_controls: bool,
    hud_labels: &[&str],
    hide_update: bool,
) -> bool {
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        let labels: Vec<String> = hud_labels.iter().map(|label| (*label).to_owned()).collect();
        run_on_appkit_main(move || {
            let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            hide_capture_huds_inner(
                &app,
                include_mini_previews,
                include_recording_controls,
                &label_refs,
                hide_update,
            )
        })
        .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    hide_capture_huds_inner(
        app,
        include_mini_previews,
        include_recording_controls,
        hud_labels,
        hide_update,
    )
}

fn hide_capture_huds_inner(
    app: &AppHandle,
    include_mini_previews: bool,
    include_recording_controls: bool,
    hud_labels: &[&str],
    hide_update: bool,
) -> bool {
    let had_visible_hud = hud_labels.iter().any(|label| {
        app.get_webview_window(label)
            .is_some_and(|window| window.is_visible().unwrap_or(false))
    }) || (!include_recording_controls
        && app.webview_windows().iter().any(|(label, window)| {
            label.starts_with(RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX)
                && window.is_visible().unwrap_or(false)
        }));
    if !include_mini_previews {
        hide_window_inner(app, "thumbnail");
    }
    if !include_recording_controls {
        let hud_visible = app
            .get_webview_window("recording-hud")
            .is_some_and(|window| window.is_visible().unwrap_or(false));
        let notice_visible = app.webview_windows().iter().any(|(label, window)| {
            label.starts_with(RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX)
                && window.is_visible().unwrap_or(false)
        });
        // Prefetch conceals on key-down; key-up conceals again after the HUD is
        // already hidden. Keep the first restore mark instead of overwriting it
        // with false, or recording controls stay gone after the snapshot.
        if recording_chrome_should_restore_after_snapshot(
            RESTORE_RECORDING_HUD_AFTER_CAPTURE.load(Ordering::SeqCst),
            hud_visible,
        ) {
            RESTORE_RECORDING_HUD_AFTER_CAPTURE.store(true, Ordering::SeqCst);
        }
        if recording_chrome_should_restore_after_snapshot(
            RESTORE_HIDDEN_CONTROLS_NOTICE_AFTER_CAPTURE.load(Ordering::SeqCst),
            notice_visible,
        ) {
            RESTORE_HIDDEN_CONTROLS_NOTICE_AFTER_CAPTURE.store(true, Ordering::SeqCst);
        }
        hide_window_inner(app, "recording-hud");
        hide_recording_controls_hidden_notices(app);
    }
    hide_window_inner(app, "startup");
    hide_recording_saved_notices_inner(app);
    if hide_update {
        hide_window_inner(app, "update");
    }
    had_visible_hud
}

fn include_mini_previews_in_captures(app: &AppHandle) -> bool {
    app.try_state::<Arc<AppState>>()
        .is_some_and(|state| state.settings().include_mini_previews_in_captures)
}

fn include_recording_controls_in_captures(app: &AppHandle) -> bool {
    app.try_state::<Arc<AppState>>()
        .is_some_and(|state| state.settings().include_recording_controls_in_captures)
}

fn hide_recording_saved_notices_inner(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(RECORDING_SAVED_NOTICE_LABEL) {
        let _ = window.hide();
        let _ = set_window_content_protected(&window, false);
    }
}

fn hide_recording_controls_hidden_notices(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX) {
            let _ = window.hide();
            let _ = set_window_content_protected(&window, false);
        }
    }
}

fn set_capture_huds_protected(app: &AppHandle, protected: bool) {
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        if run_on_appkit_main(move || set_capture_huds_protected_inner(&app, protected)).is_none() {
            eprintln!("failed to update capture HUD protection on the main thread");
        }
    }
    #[cfg(not(target_os = "macos"))]
    set_capture_huds_protected_inner(app, protected);
}

fn set_capture_huds_protected_inner(app: &AppHandle, protected: bool) {
    // The window server may still composite a just-hidden HUD into an
    // immediate display capture. Exclude Captures HUDs until the frozen background
    // frame has been read so they cannot reappear as pixels during fade-in.
    let include_mini_previews = include_mini_previews_in_captures(app);
    let include_recording_controls = include_recording_controls_in_captures(app);
    for (label, window) in app.webview_windows() {
        if !matches!(
            label.as_str(),
            "thumbnail"
                | "startup"
                | "update"
                | "recording-hud"
                | recording::RECORDING_REGION_INDICATOR_LABEL
                | RECORDING_SAVED_NOTICE_LABEL
        ) && !label.starts_with(RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX)
        {
            continue;
        }
        // Keep opted-in chrome capturable when the matching preference is on.
        // Update notices stay capturable so the changelog or an error can be
        // screenshotted. Recording controls stay excluded unless the user opted
        // them in — releasing HUD protection after a freeze-frame used to make
        // Cmd+Shift+4 capture them.
        let next_protected = if label == recording::RECORDING_REGION_INDICATOR_LABEL {
            true
        } else if label == "thumbnail" && include_mini_previews {
            false
        } else if label == "recording-hud" {
            !include_recording_controls
        } else if label == "update" && !updates::should_hide_update_notice_for_capture(app) {
            false
        } else {
            protected
        };
        if let Err(error) = set_window_content_protected(&window, next_protected) {
            eprintln!("failed to update {label} capture protection: {error}");
        }
        #[cfg(target_os = "macos")]
        if let Err(error) =
            captures_macos_window::set_excluded_from_capture(&window, next_protected)
        {
            eprintln!("failed to update {label} capture sharing: {error}");
        }
    }
}

/// Re-show recording chrome that was hidden so it would not freeze into a snapshot.
///
/// Does not focus the HUD. Skip restore when the user had already hidden the
/// controls (the hidden-controls notice was showing instead).
pub(crate) fn restore_excluded_recording_chrome(app: &AppHandle) {
    let restore_notice = RESTORE_HIDDEN_CONTROLS_NOTICE_AFTER_CAPTURE.swap(false, Ordering::SeqCst);
    let restore_hud = RESTORE_RECORDING_HUD_AFTER_CAPTURE.swap(false, Ordering::SeqCst);
    if restore_notice {
        for (label, window) in app.webview_windows() {
            if !label.starts_with(RECORDING_CONTROLS_HIDDEN_NOTICE_PREFIX) {
                continue;
            }
            #[cfg(target_os = "macos")]
            if let Err(error) = captures_macos_window::show_without_activating(&window) {
                eprintln!("failed to restore hidden recording controls notice: {error}");
            }
            #[cfg(not(target_os = "macos"))]
            if let Err(error) = window.show() {
                eprintln!("failed to restore hidden recording controls notice: {error}");
            }
            let _ = set_window_content_protected(&window, true);
        }
        return;
    }
    if !restore_hud {
        return;
    }
    let recording_is_active = app
        .try_state::<Arc<AppState>>()
        .is_some_and(|state| recording::recording_controls_are_available(state.inner()));
    if !recording_is_active {
        return;
    }
    let Some(hud) = app.get_webview_window("recording-hud") else {
        return;
    };
    if hud.is_visible().unwrap_or(false) {
        return;
    }
    let excluded = !include_recording_controls_in_captures(app);
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = captures_macos_window::set_excluded_from_capture(&hud, excluded) {
            eprintln!("failed to restore recording controls capture sharing: {error}");
        }
        if let Err(error) = captures_macos_window::show_without_activating(&hud) {
            eprintln!("failed to restore recording controls: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    if let Err(error) = hud.show() {
        eprintln!("failed to restore recording controls: {error}");
    }
    let _ = set_window_content_protected(&hud, excluded);
}

/// Runs `work` on AppKit's main thread and waits for it.
///
/// Tauri `run_on_main_thread` posts and does not wait. Async capture commands
/// need hide/show and pasteboard writes to finish before the next snapshot.
#[cfg(target_os = "macos")]
fn run_on_appkit_main<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    captures_macos_window::run_on_main(work)
}

pub(crate) fn hide_capture_overlay(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        if run_on_appkit_main(move || hide_capture_overlay_inner(&app)).is_none() {
            eprintln!("failed to hide the capture overlay on the main thread");
        }
    }
    #[cfg(not(target_os = "macos"))]
    hide_capture_overlay_inner(app);
}

fn hide_capture_overlay_inner(app: &AppHandle) {
    // Hide before handing activation back. macOS focus restoration can take a
    // variable amount of time, and doing it first left the completed marquee on
    // screen after pointer release. Titled document windows stay ordered out
    // until reveal_document_windows_after_capture, so this ordering cannot flash
    // an editor during the overlay → countdown handoff.
    if let Some(window) = app.get_webview_window("overlay") {
        #[cfg(target_os = "macos")]
        captures_macos_window::dismiss_capture_overlay_input(Some(&window));
        let _ = set_click_through(&window, false);
        let _ = window.hide();
        let _ = window.set_cursor_icon(CursorIcon::Default);
        #[cfg(target_os = "macos")]
        if let Err(error) = captures_macos_window::reset_capture_overlay(&window) {
            eprintln!("failed to reset capture overlay: {error}");
        }
    } else {
        #[cfg(target_os = "macos")]
        captures_macos_window::dismiss_capture_overlay_input(None);
    }
    #[cfg(target_os = "macos")]
    captures_macos_window::restore_frontmost_app_after_capture();
    sync_capture_escape(app);
}

/// Re-shows document windows ordered out while a capture surface was active.
///
/// Safe to call multiple times; a second call is a no-op when nothing is stashed.
pub(crate) fn reveal_document_windows_after_capture(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        if let Err(error) = app.run_on_main_thread(|| {
            captures_macos_window::reveal_concealed_document_windows();
        }) {
            eprintln!("failed to reveal document windows after capture: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// Ensures [`reveal_document_windows_after_capture`] runs when a capture commit
/// path returns (success, cancel, or error).
pub(crate) struct RevealDocumentWindowsOnDrop {
    app: AppHandle,
}

impl RevealDocumentWindowsOnDrop {
    pub(crate) fn new(app: &AppHandle) -> Self {
        Self { app: app.clone() }
    }
}

impl Drop for RevealDocumentWindowsOnDrop {
    fn drop(&mut self) {
        reveal_document_windows_after_capture(&self.app);
    }
}

fn resolve_asset(state: &AppState, path: &str) -> Option<Vec<u8>> {
    if let Some(bytes) = screenshot_editor::resolve_editor_draft_asset(path) {
        return Some(bytes);
    }
    let mut segments = path.split('/');
    match (segments.next(), segments.next()) {
        (Some("session"), Some(id)) => Uuid::parse_str(id).ok().and_then(|id| {
            state
                .sessions
                .lock()
                .get(&id)
                .map(|session| session.snapshot_png.clone())
        }),
        (Some("artifact"), Some(id)) => {
            state.find_artifact(id).map(|artifact| artifact.preview_png)
        }
        (Some("artifact-full"), Some(id)) => {
            state.find_artifact(id).map(|artifact| artifact.image_png)
        }
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

fn crop_live_region(
    state: &AppState,
    display_id: &str,
    rect: LogicalRect,
) -> Result<RgbaImage, AppError> {
    ensure_capture_session_available()?;
    let pointer = pointer_position();
    let frame = state.backend.capture_display(display_id)?;
    let mut image = crop_region_from_display(&frame.descriptor, &frame.image, rect)?;
    apply_screenshot_cursor_to_region(
        &mut image,
        &frame.descriptor,
        &frame.image,
        rect,
        pointer,
        state.settings().show_cursor_in_screenshots,
    );
    Ok(image)
}

fn crop_region_from_display(
    display: &captures_capture::DisplayDescriptor,
    image: &RgbaImage,
    rect: LogicalRect,
) -> Result<RgbaImage, AppError> {
    let physical = region_physical_rect(display, image, rect)?;
    Ok(image::imageops::crop_imm(
        image,
        physical.x,
        physical.y,
        physical.width,
        physical.height,
    )
    .to_image())
}

fn region_physical_rect(
    display: &captures_capture::DisplayDescriptor,
    image: &RgbaImage,
    rect: LogicalRect,
) -> Result<PhysicalRect, AppError> {
    let scale = display.overlay_to_buffer_scale(image.width(), image.height());
    let physical = rect.to_physical(scale, image.width(), image.height());
    if physical.width == 0 || physical.height == 0 {
        return Err(AppError::InvalidSelection);
    }
    Ok(physical)
}

fn capture_live_window(
    state: &AppState,
    selected_window: &captures_capture::WindowDescriptor,
) -> Result<RgbaImage, AppError> {
    ensure_capture_session_available()?;
    let current_windows = state.windows().ok();
    let current_window = current_windows
        .as_ref()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window.id == selected_window.id)
                .cloned()
        })
        .unwrap_or_else(|| selected_window.clone());
    let display_crop_is_safe = current_windows
        .as_ref()
        .is_some_and(|windows| window_display_crop_is_safe(&current_window, windows));

    resolve_window_capture(
        display_crop_is_safe,
        || {
            let pointer = pointer_position();
            state
                .backend
                .capture_display(&current_window.display_id)
                .ok()
                .and_then(|frame| {
                    let mut image =
                        crop_window_from_display(&frame.descriptor, &frame.image, &current_window)?;
                    apply_screenshot_cursor_to_window_crop(
                        &mut image,
                        &frame.descriptor,
                        &frame.image,
                        &current_window,
                        pointer,
                        state.settings().show_cursor_in_screenshots,
                    );
                    Some(image)
                })
        },
        || {
            let pointer = pointer_position();
            let mut image = state.backend.capture_window(&current_window.id)?;
            apply_screenshot_cursor_on_window(
                &mut image,
                &current_window,
                state
                    .monitors()
                    .ok()
                    .and_then(|displays| {
                        displays
                            .into_iter()
                            .find(|display| display.id == current_window.display_id)
                    })
                    .map(|display| display.scale_factor)
                    .unwrap_or(1.0),
                pointer,
                state.settings().show_cursor_in_screenshots,
            );
            Ok(image)
        },
    )
}

fn resolve_window_capture(
    display_crop_is_safe: bool,
    display_crop: impl FnOnce() -> Option<RgbaImage>,
    native_capture: impl FnOnce() -> Result<RgbaImage, CaptureError>,
) -> Result<RgbaImage, AppError> {
    if display_crop_is_safe
        && let Some(image) = display_crop()
        && !image_is_effectively_blank(&image)
    {
        return Ok(image);
    }

    let native_error = match native_capture() {
        Ok(image) if !image_is_effectively_blank(&image) => return Ok(image),
        Ok(_) => None,
        Err(error) => Some(error),
    };

    if !display_crop_is_safe {
        return Err(AppError::Task(
            "Could not isolate that window while another window was covering it. Bring the window forward and try again."
                .to_owned(),
        ));
    }

    match native_error {
        None => Err(AppError::Task(
            "Could not capture that window (empty frame). Try Region capture.".to_owned(),
        )),
        Some(error) => Err(error.into()),
    }
}

fn window_display_crop_is_safe(
    selected: &captures_capture::WindowDescriptor,
    windows: &[captures_capture::WindowDescriptor],
) -> bool {
    windows.iter().any(|candidate| candidate.id == selected.id)
        && !windows.iter().any(|candidate| {
            candidate.id != selected.id
                && candidate.display_id == selected.display_id
                && candidate.z_order > selected.z_order
                && window_rects_overlap(selected, candidate)
                && !window_is_associated_transient(selected, candidate)
        })
}

fn window_is_associated_transient(
    selected: &captures_capture::WindowDescriptor,
    candidate: &captures_capture::WindowDescriptor,
) -> bool {
    // xcap exposes app-owned menus, popovers, and similar transient surfaces as
    // separate untitled windows. Keeping those in an otherwise safe display crop
    // preserves frozen/countdown states without admitting another document window.
    candidate.title.trim().is_empty()
        && selected
            .app_name
            .as_deref()
            .zip(candidate.app_name.as_deref())
            .is_some_and(|(selected_app, candidate_app)| {
                !selected_app.trim().is_empty()
                    && selected_app
                        .trim()
                        .eq_ignore_ascii_case(candidate_app.trim())
            })
}

fn window_rects_overlap(
    left: &captures_capture::WindowDescriptor,
    right: &captures_capture::WindowDescriptor,
) -> bool {
    let left_x = i64::from(left.x);
    let left_y = i64::from(left.y);
    let left_right = left_x + i64::from(left.width);
    let left_bottom = left_y + i64::from(left.height);
    let right_x = i64::from(right.x);
    let right_y = i64::from(right.y);
    let right_right = right_x + i64::from(right.width);
    let right_bottom = right_y + i64::from(right.height);

    left_x < right_right && right_x < left_right && left_y < right_bottom && right_y < left_bottom
}

fn crop_window_from_session(session: &CaptureSession, window_id: &str) -> Option<RgbaImage> {
    let image = session.image.as_ref()?;
    let window = session
        .windows
        .iter()
        .find(|window| window.id == window_id)?;
    crop_window_from_display(&session.display, image, window)
}

fn window_physical_rect(
    display: &captures_capture::DisplayDescriptor,
    image: &RgbaImage,
    window: &captures_capture::WindowDescriptor,
) -> Option<PhysicalRect> {
    let scale = capture_buffer_scale(display, image);
    let rect = LogicalRect {
        x: f64::from(window.x - display.x),
        y: f64::from(window.y - display.y),
        width: f64::from(window.width),
        height: f64::from(window.height),
    };
    let physical = rect.to_physical(scale, image.width(), image.height());
    if physical.width == 0 || physical.height == 0 {
        None
    } else {
        Some(physical)
    }
}

fn crop_window_from_display(
    display: &captures_capture::DisplayDescriptor,
    image: &RgbaImage,
    window: &captures_capture::WindowDescriptor,
) -> Option<RgbaImage> {
    let physical = window_physical_rect(display, image, window)?;
    #[cfg(target_os = "macos")]
    let scale = capture_buffer_scale(display, image);
    let image = image::imageops::crop_imm(
        image,
        physical.x,
        physical.y,
        physical.width,
        physical.height,
    )
    .to_image();
    #[cfg(target_os = "macos")]
    let image = {
        let mut image = image;
        mask_macos_window_corners(
            &mut image,
            window,
            display,
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

enum WindowPickRole {
    Capturable,
    ShellChrome,
}

fn window_pick_role(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
) -> Option<WindowPickRole> {
    if window.display_id != display.id {
        return None;
    }
    if window.width == 0 || window.height == 0 {
        return None;
    }
    if captures_window_is_internal(window) {
        return None;
    }
    #[cfg(target_os = "macos")]
    if macos_window_is_capture_overlay(window) {
        return None;
    }
    #[cfg(target_os = "windows")]
    if windows_window_is_capture_overlay(window) {
        return None;
    }
    if window_is_screen_edge_chrome(window, display) {
        return Some(WindowPickRole::ShellChrome);
    }
    if window_is_desktop_backdrop(window, display) {
        return None;
    }
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
        return None;
    }
    if window.width < 48 || window.height < 48 {
        return None;
    }
    Some(WindowPickRole::Capturable)
}

#[cfg_attr(not(test), allow(dead_code))]
fn window_is_capturable(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
) -> bool {
    matches!(
        window_pick_role(window, display),
        Some(WindowPickRole::Capturable)
    )
}

fn window_overlap_area(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
) -> u64 {
    let left = i64::from(window.x).max(i64::from(display.x));
    let top = i64::from(window.y).max(i64::from(display.y));
    let right = (i64::from(window.x) + i64::from(window.width))
        .min(i64::from(display.x) + i64::from(display.width));
    let bottom = (i64::from(window.y) + i64::from(window.height))
        .min(i64::from(display.y) + i64::from(display.height));
    let width = (right - left).max(0);
    let height = (bottom - top).max(0);
    u64::try_from(width * height).unwrap_or(0)
}

fn window_covers_display(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
) -> bool {
    let display_area = u64::from(display.width) * u64::from(display.height);
    if display_area == 0 {
        return false;
    }
    window_overlap_area(window, display) * 100 >= display_area * 95
}

/// Menu bar, taskbar, and dock/panel strips that span a display edge.
fn window_is_screen_edge_chrome(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
) -> bool {
    const MAX_THICKNESS: i32 = 96;
    let display_right = i64::from(display.x) + i64::from(display.width);
    let display_bottom = i64::from(display.y) + i64::from(display.height);
    let window_left = i64::from(window.x);
    let window_top = i64::from(window.y);
    let window_right = window_left + i64::from(window.width);
    let window_bottom = window_top + i64::from(window.height);
    let spans_width = window_left <= i64::from(display.x) + 8
        && window_right >= display_right - 8
        && i32::try_from(window.width).unwrap_or(i32::MAX)
            >= display.width.saturating_sub(16) as i32;
    let spans_height = window_top <= i64::from(display.y) + 8
        && window_bottom >= display_bottom - 8
        && i32::try_from(window.height).unwrap_or(i32::MAX)
            >= display.height.saturating_sub(16) as i32;
    let thickness_h = i32::try_from(window.height).unwrap_or(i32::MAX);
    let thickness_w = i32::try_from(window.width).unwrap_or(i32::MAX);
    let top_bar =
        spans_width && thickness_h <= MAX_THICKNESS && window_top <= i64::from(display.y) + 8;
    let bottom_bar =
        spans_width && thickness_h <= MAX_THICKNESS && window_bottom >= display_bottom - 8;
    let left_bar =
        spans_height && thickness_w <= MAX_THICKNESS && window_left <= i64::from(display.x) + 8;
    let right_bar =
        spans_height && thickness_w <= MAX_THICKNESS && window_right >= display_right - 8;
    top_bar || bottom_bar || left_bar || right_bar
}

/// Wallpaper / desktop windows that fill the display and steal hits under the
/// menu bar or taskbar. Named document windows from the same apps stay selectable.
fn window_is_desktop_backdrop(
    window: &captures_capture::WindowDescriptor,
    display: &captures_capture::DisplayDescriptor,
) -> bool {
    if !window_covers_display(window, display) {
        return false;
    }
    let title = window.title.trim();
    if title.eq_ignore_ascii_case("Desktop") || title.eq_ignore_ascii_case("Program Manager") {
        return true;
    }
    let Some(app) = window.app_name.as_deref().map(str::trim) else {
        return false;
    };
    const BACKDROP_APPS: &[&str] = &[
        "Finder",
        "explorer",
        "explorer.exe",
        "Progman",
        "WorkerW",
        "Nautilus",
        "nemo",
        "caja",
        "pcmanfm",
        "pcmanfm-qt",
        "dolphin",
        "plasmashell",
        "gnome-shell",
    ];
    if !BACKDROP_APPS
        .iter()
        .any(|excluded| app.eq_ignore_ascii_case(excluded))
    {
        return false;
    }
    title.is_empty() || title.eq_ignore_ascii_case("Desktop")
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
        recording::RECORDING_REGION_INDICATOR_TITLE,
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
        AppError, AppReactivation, CaptureMode, InteractiveLaunchAction, LogicalRect,
        PreviewFileDropLanding, STARTUP_NOTICE_AFTER_SETUP_VISIBLE,
        STARTUP_NOTICE_AUTOSTART_VISIBLE, STARTUP_NOTICE_HEIGHT, STARTUP_NOTICE_WIDTH,
        StartupNoticeCaret, THUMBNAIL_AUTO_HIDE_RESERVE, THUMBNAIL_SYSTEM_CHROME_GAP,
        TRAY_NOTICE_CARET_INSET, TRAY_NOTICE_CARET_SIZE, TRAY_NOTICE_FRAME_PAD,
        TRAY_NOTICE_SCREEN_MARGIN, TRAY_NOTICE_TRAY_OVERLAP, ThumbnailCursorAction,
        ThumbnailCursorKind, ThumbnailMonitorBounds, ThumbnailPointerSpace, ThumbnailStackAnchor,
        ThumbnailStackOrigin, ThumbnailWindowFrame, ThumbnailWindowGeometry, app_reactivation,
        capturable_windows_for_display, capture_cursor_icon, classify_preview_file_drop,
        click_through_applies, clipboard_fingerprint, display_contains_pointer,
        drag_plugin_cursor_to_pointer_space, encode_overlay_snapshot_with_cursor,
        fallback_startup_notice, freeze_prefetch_can_start, interactive_launch_action,
        mask_macos_window_corners, parse_shortcut, place_startup_notice, preferences_url,
        primary_app_window_priority, recording::RECORDING_REGION_INDICATOR_TITLE,
        recording_chrome_should_restore_after_snapshot, refine_window_chrome_from_snapshot,
        resolve_startup_notice_placement, resolve_window_capture,
        screenshot_countdown_seconds_for_capture_ui, should_claim_region_cursor_after_freeze,
        should_claim_region_cursor_on_shortcut_press, should_freeze_visible_capture_ui,
        should_prefetch_freeze_on_shortcut_press, should_trigger_shortcut,
        startup_notice_fallback_edge_from_insets, startup_notice_url, take_ready_or_defer_windows,
        thumbnail_clamp_aligned_frame, thumbnail_cursor_action, thumbnail_cursor_ignore_update,
        thumbnail_geometry, thumbnail_pointer_in_space, thumbnail_pointer_position,
        thumbnail_preserve_current_height, thumbnail_stack_height,
        thumbnail_stack_should_be_visible, thumbnail_visible_window_height, thumbnail_window_top,
        track_shortcut_suppression, tray_accelerator, tray_icon_rect_is_usable,
        tray_notice_window_size, viewer_window_label, window_display_crop_is_safe,
        window_is_capturable, windows_display_affinity_excludes_capture,
        windows_window_is_capture_overlay,
    };

    #[test]
    fn preferences_url_only_accepts_known_targets() {
        assert_eq!(
            preferences_url(Some("auto-start-on-selection")),
            "index.html?view=preferences&target=auto-start-on-selection"
        );
        assert_eq!(
            preferences_url(Some("include-recording-controls-in-captures")),
            "index.html?view=preferences&target=include-recording-controls-in-captures"
        );
        assert_eq!(
            preferences_url(Some("unknown")),
            "index.html?view=preferences"
        );
        assert_eq!(preferences_url(None), "index.html?view=preferences");
    }

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

    fn stack_geometry(
        bounds: ThumbnailMonitorBounds,
        count: usize,
        collapsed: bool,
        origin: Option<ThumbnailStackOrigin>,
    ) -> ThumbnailWindowGeometry {
        thumbnail_geometry(
            bounds,
            count,
            collapsed,
            origin,
            crate::models::MiniPreviewPlacement::BottomLeft,
        )
    }

    fn stack_xyh(
        bounds: ThumbnailMonitorBounds,
        count: usize,
        collapsed: bool,
        origin: Option<ThumbnailStackOrigin>,
    ) -> (f64, f64, f64) {
        let geometry = stack_geometry(bounds, count, collapsed, origin);
        (geometry.x, geometry.y, geometry.height)
    }

    use captures_capture::{DisplayDescriptor, WindowDescriptor};

    fn patterned_window_image(primary: [u8; 4], secondary: [u8; 4]) -> RgbaImage {
        RgbaImage::from_fn(8, 8, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba(primary)
            } else {
                Rgba(secondary)
            }
        })
    }

    #[test]
    fn safe_window_capture_preserves_the_composited_display_crop() {
        let native = patterned_window_image([10, 20, 30, 255], [200, 210, 220, 255]);
        let display_crop = patterned_window_image([80, 10, 10, 255], [90, 20, 20, 255]);
        let mut native_capture_used = false;

        let captured = resolve_window_capture(
            true,
            || Some(display_crop.clone()),
            || {
                native_capture_used = true;
                Ok(native)
            },
        )
        .expect("the unobstructed display crop should succeed");

        assert_eq!(captured, display_crop);
        assert!(!native_capture_used);
    }

    #[test]
    fn window_capture_falls_back_to_native_pixels_when_the_display_crop_is_unsafe_or_blank() {
        let blank = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255]));
        let native = patterned_window_image([10, 90, 30, 255], [180, 40, 220, 255]);
        let captured = resolve_window_capture(true, || Some(blank.clone()), || Ok(native.clone()))
            .expect("a blank safe crop should fall back to the native window");
        assert_eq!(captured, native);

        let mut display_crop_used = false;
        let captured = resolve_window_capture(
            false,
            || {
                display_crop_used = true;
                Some(patterned_window_image(
                    [240, 240, 240, 255],
                    [10, 10, 10, 255],
                ))
            },
            || Ok(native.clone()),
        )
        .expect("an unsafe crop should use the native window");

        assert_eq!(captured, native);
        assert!(!display_crop_used);

        let error = resolve_window_capture(false, || None, || Ok(blank))
            .expect_err("an empty native capture must not fall back to occluded pixels");
        assert!(error.to_string().contains("another window was covering it"));
    }

    #[test]
    fn display_crop_allows_app_transients_but_rejects_unrelated_occluders() {
        let window = |id: &str, app_name: &str, title: &str, z_order: i32, x: i32, y: i32| {
            WindowDescriptor {
                id: id.to_owned(),
                title: title.to_owned(),
                app_name: Some(app_name.to_owned()),
                z_order,
                x,
                y,
                width: 100,
                height: 80,
                display_id: "display".to_owned(),
                corner_radius: None,
            }
        };
        let selected = window("selected", "Browser", "Page", 5, 100, 100);
        let covering = window("covering", "Editor", "Code", 6, 150, 120);
        let behind = window("behind", "Editor", "Code", 4, 120, 110);
        let adjacent = window("adjacent", "Editor", "Code", 7, 200, 100);
        let transient = window("menu", "Browser", "", 7, 130, 110);
        let same_app_document = window("other-page", "Browser", "Other page", 6, 140, 120);

        assert!(!window_display_crop_is_safe(
            &selected,
            &[selected.clone(), covering]
        ));
        assert!(window_display_crop_is_safe(
            &selected,
            &[selected.clone(), behind, adjacent]
        ));
        assert!(window_display_crop_is_safe(
            &selected,
            &[selected.clone(), transient]
        ));
        assert!(!window_display_crop_is_safe(
            &selected,
            &[selected.clone(), same_app_document]
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

    #[test]
    fn windows_display_affinity_is_not_left_on_hidden_windows() {
        assert!(windows_display_affinity_excludes_capture(true, true));
        assert!(
            !windows_display_affinity_excludes_capture(true, false),
            "hidden capture chrome must not keep WDA_EXCLUDEFROMCAPTURE; NVIDIA Instant Replay treats that as blocking desktop capture"
        );
        assert!(!windows_display_affinity_excludes_capture(false, true));
        assert!(!windows_display_affinity_excludes_capture(false, false));
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
            RECORDING_REGION_INDICATOR_TITLE,
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
    fn treats_desktop_backdrop_and_taskbar_as_display_not_windows() {
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
        let finder_desktop = WindowDescriptor {
            id: "desktop".to_owned(),
            title: String::new(),
            app_name: Some("Finder".to_owned()),
            z_order: 0,
            x: 0,
            y: 0,
            width: 1_440,
            height: 900,
            display_id: display.id.clone(),
            corner_radius: None,
        };
        assert!(!window_is_capturable(&finder_desktop, &display));

        let mut finder_folder = finder_desktop.clone();
        finder_folder.id = "folder".to_owned();
        finder_folder.title = "Documents".to_owned();
        finder_folder.x = 80;
        finder_folder.y = 80;
        finder_folder.width = 640;
        finder_folder.height = 480;
        assert!(window_is_capturable(&finder_folder, &display));

        let mut fullscreen_app = finder_desktop.clone();
        fullscreen_app.id = "safari".to_owned();
        fullscreen_app.title = "Safari".to_owned();
        fullscreen_app.app_name = Some("Safari".to_owned());
        assert!(window_is_capturable(&fullscreen_app, &display));

        let taskbar = WindowDescriptor {
            id: "taskbar".to_owned(),
            title: String::new(),
            app_name: Some("explorer.exe".to_owned()),
            z_order: 40,
            x: 0,
            y: 852,
            width: 1_440,
            height: 48,
            display_id: display.id.clone(),
            corner_radius: None,
        };
        assert!(!window_is_capturable(&taskbar, &display));

        let menu_bar = WindowDescriptor {
            id: "menubar".to_owned(),
            title: String::new(),
            app_name: Some("Control Center".to_owned()),
            z_order: 50,
            x: 0,
            y: 0,
            width: 1_440,
            height: 24,
            display_id: display.id.clone(),
            corner_radius: None,
        };
        assert!(!window_is_capturable(&menu_bar, &display));

        let maximized = WindowDescriptor {
            id: "app".to_owned(),
            title: "Browser".to_owned(),
            app_name: Some("Safari".to_owned()),
            z_order: 10,
            x: 0,
            y: 0,
            width: 1_440,
            height: 900,
            display_id: display.id.clone(),
            corner_radius: None,
        };
        let targets = capturable_windows_for_display(
            Ok(vec![
                menu_bar.clone(),
                taskbar.clone(),
                maximized,
                finder_desktop,
            ]),
            &display,
            None,
        );
        assert_eq!(
            targets
                .shell_chrome
                .iter()
                .map(|window| window.id.as_str())
                .collect::<Vec<_>>(),
            vec!["menubar", "taskbar"]
        );
        assert_eq!(
            targets
                .windows
                .iter()
                .map(|window| window.id.as_str())
                .collect::<Vec<_>>(),
            vec!["app"]
        );
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
    fn capture_overlay_can_hide_itself_after_a_region_selection() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/capture-overlay.json"))
                .expect("capture overlay capability should be valid JSON");
        let permissions = capability["permissions"]
            .as_array()
            .expect("capture overlay capability should grant permissions");

        assert_eq!(capability["windows"], serde_json::json!(["overlay"]));
        assert!(
            permissions
                .iter()
                .any(|granted| granted == "core:window:allow-hide"),
            "capture overlay fast hide requires core:window:allow-hide"
        );
    }

    #[test]
    fn update_notice_cannot_hide_itself_through_the_window_api() {
        // Later/Close used Window.hide, which is a no-op without allow-hide.
        // Dismiss goes through the dismiss_update_notice command instead.
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json"))
                .expect("desktop capability should be valid JSON");
        let windows = capability["windows"]
            .as_array()
            .expect("desktop capability should target windows");
        let permissions = capability["permissions"]
            .as_array()
            .expect("desktop capability should grant permissions");

        assert!(windows.iter().any(|window| window == "update"));
        assert!(
            windows
                .iter()
                .any(|window| window == "screenshot-countdown"),
            "screenshot countdown needs IPC so Escape can cancel it"
        );
        assert!(
            !permissions
                .iter()
                .any(|granted| granted == "core:window:allow-hide"),
            "update notice must not rely on Window.hide from the webview"
        );
    }

    #[test]
    fn editor_windows_can_complete_close_requests() {
        // Screenshot editors listen to onCloseRequested so they can flush a
        // draft. Tauri then destroy()s the window; without these permissions
        // the native close button is a no-op. Recording editors share the
        // same window family so a future close listener cannot regress.
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/editors.json"))
                .expect("editors capability should be valid JSON");
        let windows = capability["windows"]
            .as_array()
            .expect("editors capability should target windows");
        let permissions = capability["permissions"]
            .as_array()
            .expect("editors capability should grant permissions");

        for window in ["screenshot-editor-*", "recording-editor-*"] {
            assert!(
                windows.iter().any(|granted| granted == window),
                "editors capability should target {window}"
            );
        }
        for permission in ["core:window:allow-close", "core:window:allow-destroy"] {
            assert!(
                permissions.iter().any(|granted| granted == permission),
                "editors capability should grant {permission}"
            );
        }
    }

    #[test]
    fn document_window_backgrounds_match_the_appearance_canvases() {
        assert_eq!(
            super::DOCUMENT_WINDOW_BACKGROUND_DARK,
            super::Color(16, 16, 20, 255)
        );
        assert_eq!(
            super::DOCUMENT_WINDOW_BACKGROUND_LIGHT,
            super::Color(245, 245, 247, 255)
        );
        assert_eq!(super::NOTICE_WINDOW_BACKGROUND, super::Color(0, 0, 0, 0));
    }

    #[test]
    fn macos_bundle_requests_microphone_audio_input() {
        let entitlements = include_str!("../Entitlements.plist");
        assert!(
            entitlements.contains("com.apple.security.device.audio-input"),
            "Hardened Runtime blocks microphone access unless the audio-input entitlement is signed in"
        );

        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("tauri.conf.json should be valid JSON");
        assert_eq!(
            config["bundle"]["macOS"]["entitlements"],
            "./Entitlements.plist"
        );
    }

    #[test]
    fn macos_privacy_settings_prefer_current_system_settings_urls() {
        assert_eq!(
            super::MACOS_MICROPHONE_SETTINGS_URLS[0],
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone"
        );
        assert_eq!(
            super::MACOS_SCREEN_RECORDING_SETTINGS_URLS[0],
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture"
        );
        assert!(
            super::MACOS_MICROPHONE_SETTINGS_URLS[1].contains("Privacy_Microphone"),
            "keep the pre-Ventura microphone URL as a fallback"
        );
        assert!(
            super::MACOS_SCREEN_RECORDING_SETTINGS_URLS[1].contains("Privacy_ScreenCapture"),
            "keep the pre-Ventura Screen Recording URL as a fallback"
        );
    }

    #[test]
    fn onboarding_window_can_close_after_setup() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/onboarding.json"))
                .expect("onboarding capability should be valid JSON");

        assert_eq!(capability["windows"], serde_json::json!(["onboarding"]));
        assert!(
            capability["permissions"]
                .as_array()
                .is_some_and(|permissions| permissions
                    .iter()
                    .any(|permission| permission == "core:window:allow-close"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn region_shortcut_claims_the_crosshair_on_press() {
        assert!(captures_macos_window::region_shortcut_claims_cursor_on_press());
        assert!(captures_macos_window::region_cursor_claim_waits_for_freeze_frame(true));
        assert!(!captures_macos_window::region_cursor_claim_waits_for_freeze_frame(false));
        assert!(captures_macos_window::overlay_prepare_keeps_native_cursor(
            captures_macos_window::CaptureCursor::overlay_region().native_owned
        ));
        assert_eq!(
            captures_macos_window::thumbnail_unpolled_hover(
                false,
                captures_macos_window::ThumbnailHoverCursor::Default,
            ),
            captures_macos_window::ThumbnailHoverCursor::Default
        );
        assert_eq!(
            captures_macos_window::thumbnail_unpolled_hover(
                true,
                captures_macos_window::ThumbnailHoverCursor::Default,
            ),
            captures_macos_window::ThumbnailHoverCursor::Default
        );
        assert!(captures_macos_window::cursor_claim_panel_should_show(
            true, true, false, false, false
        ));
        assert!(!captures_macos_window::cursor_claim_panel_should_show(
            true, true, true, false, false
        ));
        assert!(captures_macos_window::cursor_claim_panel_should_resign_key(
            true
        ));
        assert!(!captures_macos_window::capture_surface_focus_retry_allowed(
            1, 2, true
        ));
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
        assert_eq!(
            thumbnail_cursor_action(false, true, ThumbnailCursorKind::Default),
            ThumbnailCursorAction::Reset
        );
        assert_eq!(
            thumbnail_cursor_action(false, false, ThumbnailCursorKind::Pointer),
            ThumbnailCursorAction::Reset
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn maps_preview_cursor_kinds_to_tauri_icons() {
        use tauri::CursorIcon;

        assert_eq!(
            super::thumbnail_tauri_cursor_icon(ThumbnailCursorKind::Default),
            CursorIcon::Default
        );
        assert_eq!(
            super::thumbnail_tauri_cursor_icon(ThumbnailCursorKind::Pointer),
            CursorIcon::Hand
        );
        assert_eq!(
            super::thumbnail_tauri_cursor_icon(ThumbnailCursorKind::Grab),
            CursorIcon::Grab
        );
    }

    #[test]
    fn treats_legacy_and_recorded_shortcut_formats_as_the_same_combination() {
        assert_eq!(
            parse_shortcut("Ctrl+Shift+4").expect("legacy shortcut should parse"),
            parse_shortcut("Control+Shift+Digit4").expect("recorded shortcut should parse")
        );
        assert!(parse_shortcut("Ctrl+Shift+Space").is_ok());
        assert!(parse_shortcut("CommandOrControl+Shift+4").is_ok());
        assert!(parse_shortcut("PrintScreen").is_ok());
        assert!(parse_shortcut("Shift+PrintScreen").is_ok());
        assert!(parse_shortcut("Alt+PrintScreen").is_ok());
        assert!(parse_shortcut("Super+Shift+S").is_ok());
        assert!(parse_shortcut("Super+Alt+R").is_ok());
        assert!(parse_shortcut("Control+Shift+Alt+R").is_ok());
        assert_eq!(
            parse_shortcut("CommandOrControl+Shift+4")
                .expect("cross-platform default should parse"),
            parse_shortcut(if cfg!(target_os = "macos") {
                "Command+Shift+4"
            } else {
                "Ctrl+Shift+4"
            })
            .expect("platform default should parse")
        );
        assert_eq!(
            parse_shortcut("Super+Shift+S").expect("windows/linux region shortcut should parse"),
            parse_shortcut("Super+Shift+KeyS").expect("recorded super-shift-s should parse")
        );
        assert!(parse_shortcut("Escape").is_ok());
    }

    #[test]
    fn freeze_region_shortcut_snapshots_before_claiming_the_crosshair() {
        assert!(should_prefetch_freeze_on_shortcut_press(
            CaptureMode::Region,
            true,
        ));
        assert!(should_claim_region_cursor_after_freeze(
            CaptureMode::Region,
            true,
        ));
        assert!(!should_claim_region_cursor_on_shortcut_press(
            CaptureMode::Region,
            true,
            false,
        ));
    }

    #[test]
    fn live_region_shortcut_still_claims_the_crosshair_immediately() {
        assert!(!should_prefetch_freeze_on_shortcut_press(
            CaptureMode::Region,
            false,
        ));
        assert!(!should_claim_region_cursor_after_freeze(
            CaptureMode::Region,
            false,
        ));
        assert!(should_claim_region_cursor_on_shortcut_press(
            CaptureMode::Region,
            false,
            false,
        ));
    }

    #[test]
    fn freeze_window_shortcut_prefetches_without_a_cursor_claim() {
        assert!(should_prefetch_freeze_on_shortcut_press(
            CaptureMode::Window,
            true,
        ));
        assert!(!should_claim_region_cursor_after_freeze(
            CaptureMode::Window,
            true,
        ));
        assert!(!should_claim_region_cursor_on_shortcut_press(
            CaptureMode::Window,
            true,
            false,
        ));
    }

    #[test]
    fn recapture_region_overlay_waits_to_claim_the_crosshair() {
        assert!(!should_claim_region_cursor_on_shortcut_press(
            CaptureMode::Region,
            false,
            true,
        ));
        assert!(should_claim_region_cursor_after_freeze(
            CaptureMode::Region,
            true,
        ));
        assert_eq!(screenshot_countdown_seconds_for_capture_ui(3, true), 0);
        assert_eq!(screenshot_countdown_seconds_for_capture_ui(3, false), 3);
    }

    #[test]
    fn freeze_visible_capture_ui_recaptures_overlay_for_any_screenshot() {
        assert!(should_freeze_visible_capture_ui(
            true,
            false,
            None,
            CaptureMode::Region,
        ));
        assert!(should_freeze_visible_capture_ui(
            true,
            false,
            None,
            CaptureMode::Window,
        ));
        assert!(should_freeze_visible_capture_ui(
            true,
            false,
            None,
            CaptureMode::Display,
        ));
        assert!(should_freeze_visible_capture_ui(
            false,
            true,
            Some(CaptureMode::Region),
            CaptureMode::Region,
        ));
        assert!(!should_freeze_visible_capture_ui(
            false,
            true,
            Some(CaptureMode::Region),
            CaptureMode::Window,
        ));
        assert!(!should_freeze_visible_capture_ui(
            false,
            true,
            Some(CaptureMode::Display),
            CaptureMode::Region,
        ));
        assert!(should_freeze_visible_capture_ui(
            false,
            true,
            Some(CaptureMode::Display),
            CaptureMode::Display,
        ));
        assert!(!should_freeze_visible_capture_ui(
            false,
            false,
            None,
            CaptureMode::Region,
        ));
    }

    #[test]
    fn a_second_conceal_keeps_the_recording_hud_restore_mark() {
        assert!(recording_chrome_should_restore_after_snapshot(false, true));
        assert!(recording_chrome_should_restore_after_snapshot(true, false));
        assert!(recording_chrome_should_restore_after_snapshot(true, true));
        assert!(!recording_chrome_should_restore_after_snapshot(
            false, false
        ));
    }

    #[test]
    fn freeze_prefetch_does_not_start_during_an_update_install() {
        assert!(freeze_prefetch_can_start(true, false, false));
        assert!(!freeze_prefetch_can_start(true, true, false));
        assert!(!freeze_prefetch_can_start(true, false, true));
        assert!(!freeze_prefetch_can_start(false, false, false));
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
            stack_xyh(
                bounds((0, 0, 3_992, 2_048), (0, 0, 3_992, 2_160), 2.0),
                1,
                false,
                None,
            ),
            (0.0, 772.0, 240.0)
        );
        assert_eq!(
            stack_xyh(
                bounds((-3_840, 0, 3_840, 2_048), (-3_840, 0, 3_840, 2_160), 2.0),
                2,
                false,
                None,
            ),
            (-1_920.0, 588.0, 424.0)
        );
        assert_eq!(
            stack_xyh(
                bounds((0, 0, 3_992, 2_048), (0, 0, 3_992, 2_160), 2.0),
                1,
                true,
                None,
            ),
            (0.0, 772.0, 240.0)
        );
    }

    #[test]
    fn keeps_the_thumbnail_stack_inside_the_monitor_work_area() {
        // 1920×1040 work area on a 1920×1080 display (48px taskbar).
        let (_, top, height) = stack_xyh(
            bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0),
            1,
            false,
            None,
        );

        // Window bottom sits system-chrome gap above the work-area bottom.
        assert_eq!(top + height, 1_040.0 - THUMBNAIL_SYSTEM_CHROME_GAP);
        assert!(top + height < 1_040.0);
    }

    #[test]
    fn reserves_space_when_work_area_matches_full_monitor_auto_hide() {
        // Auto-hide taskbar: work area == full 1920×1080 monitor.
        let (_, top, height) = stack_xyh(
            bounds((0, 0, 1_920, 1_080), (0, 0, 1_920, 1_080), 1.0),
            1,
            false,
            None,
        );

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
        let (_, top, height) = stack_xyh(
            bounds((0, 48, 3_992, 2_112), (0, 0, 3_992, 2_160), 2.0),
            1,
            false,
            None,
        );

        assert_eq!(
            top + height,
            1_080.0 - THUMBNAIL_AUTO_HIDE_RESERVE - THUMBNAIL_SYSTEM_CHROME_GAP
        );
    }

    #[test]
    fn places_a_dragged_stack_at_the_stored_origin() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let (x, y, height) = stack_xyh(
            work,
            1,
            true,
            Some(ThumbnailStackOrigin {
                x: 420.0,
                edge: 520.0,
                anchor: ThumbnailStackAnchor::Bottom,
            }),
        );
        assert_eq!(height, 240.0);
        assert_eq!((x, y), (420.0, 280.0));
        assert_eq!(y + height, 520.0);
    }

    #[test]
    fn keeps_a_dragged_collapsed_pile_on_its_origin_after_another_capture() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let origin = ThumbnailStackOrigin {
            x: 420.0,
            edge: 640.0,
            anchor: ThumbnailStackAnchor::Bottom,
        };
        let three = stack_geometry(work, 3, true, Some(origin));
        let four = stack_geometry(work, 4, true, Some(origin));
        assert_eq!(three.x, 420.0);
        assert_eq!(four.x, 420.0);
        assert_eq!(three.y + three.height, 640.0);
        assert_eq!(four.y + four.height, 640.0);

        // Auto-expanding that parked pile would size the window as the
        // expanded bar and clamp it to the work-area top — the stuck-drag
        // position users hit when the webview stayed collapsed.
        let expanded = stack_geometry(work, 4, false, Some(origin));
        assert_eq!(expanded.y, 0.0);
        assert!(expanded.height > four.height);
        assert_eq!(
            thumbnail_window_top(
                four.y,
                expanded.height,
                four.height,
                ThumbnailStackAnchor::Bottom,
            ),
            640.0 - expanded.height,
        );
    }

    #[test]
    fn restores_a_top_aligned_pile_without_consuming_preserved_slack() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let geometry = thumbnail_geometry(
            work,
            1,
            true,
            Some(ThumbnailStackOrigin {
                x: 420.0,
                edge: 0.0,
                anchor: ThumbnailStackAnchor::Top,
            }),
            crate::models::MiniPreviewPlacement::BottomLeft,
        );
        assert_eq!(geometry.height, 240.0);
        assert_eq!((geometry.x, geometry.y), (420.0, 0.0));
        assert_eq!(geometry.anchor, ThumbnailStackAnchor::Top);
        assert_eq!(
            thumbnail_window_top(
                geometry.y,
                792.0,
                geometry.height,
                ThumbnailStackAnchor::Top
            ),
            0.0
        );
        assert_eq!(
            thumbnail_window_top(
                geometry.y,
                792.0,
                geometry.height,
                ThumbnailStackAnchor::Bottom
            ),
            -552.0
        );
    }

    #[test]
    fn clamps_a_dragged_stack_to_the_work_area() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        assert_eq!(
            stack_xyh(
                work,
                1,
                true,
                Some(ThumbnailStackOrigin {
                    x: 8_000.0,
                    edge: 8_000.0,
                    anchor: ThumbnailStackAnchor::Bottom,
                }),
            ),
            (1_580.0, 788.0, 240.0)
        );
        assert_eq!(
            thumbnail_clamp_aligned_frame(
                -40.0,
                -20.0,
                240.0,
                240.0,
                super::thumbnail_work_area(work),
                ThumbnailStackAnchor::Bottom,
            ),
            (0.0, 0.0)
        );
    }

    #[test]
    fn lets_a_collapsed_pile_reach_the_top_when_the_window_stays_tall() {
        // 4-card expanded frame kept after collapse; pile is the bottom 240px.
        let work =
            super::thumbnail_work_area(bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0));
        assert_eq!(
            thumbnail_clamp_aligned_frame(
                -40.0,
                -800.0,
                792.0,
                240.0,
                work,
                ThumbnailStackAnchor::Bottom
            ),
            (0.0, -552.0)
        );
        assert_eq!(
            thumbnail_clamp_aligned_frame(
                420.0,
                400.0,
                792.0,
                240.0,
                work,
                ThumbnailStackAnchor::Bottom
            ),
            (420.0, 236.0)
        );
        // Visible pile top sits at the work-area top when slack is consumed.
        assert_eq!(-552.0 + 792.0 - 240.0, 0.0);
        assert_eq!(
            thumbnail_clamp_aligned_frame(
                -40.0,
                -800.0,
                792.0,
                240.0,
                work,
                ThumbnailStackAnchor::Top
            ),
            (0.0, 0.0)
        );
        assert_eq!(
            thumbnail_clamp_aligned_frame(
                420.0,
                2_000.0,
                792.0,
                240.0,
                work,
                ThumbnailStackAnchor::Top
            ),
            (420.0, 788.0)
        );
        // Top-aligned slack hangs below the work area so the pile can still
        // reach the bottom chrome gap.
        assert_eq!(788.0 + 240.0, 1_040.0 - THUMBNAIL_SYSTEM_CHROME_GAP);
    }

    #[test]
    fn places_the_stack_in_the_chosen_screen_corner() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let top_right = thumbnail_geometry(
            work,
            1,
            false,
            None,
            crate::models::MiniPreviewPlacement::TopRight,
        );
        assert_eq!(top_right.x, 1_580.0);
        assert_eq!(top_right.y, THUMBNAIL_SYSTEM_CHROME_GAP);
        assert_eq!(top_right.height, 240.0);
        assert_eq!(top_right.anchor, ThumbnailStackAnchor::Top);

        let bottom_right = thumbnail_geometry(
            work,
            1,
            false,
            None,
            crate::models::MiniPreviewPlacement::BottomRight,
        );
        assert_eq!(bottom_right.x, 1_580.0);
        assert_eq!(
            bottom_right.y + bottom_right.height,
            1_040.0 - THUMBNAIL_SYSTEM_CHROME_GAP
        );
        assert_eq!(bottom_right.anchor, ThumbnailStackAnchor::Bottom);
    }

    #[test]
    fn expands_a_top_anchored_pile_downward() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let collapsed = thumbnail_geometry(
            work,
            3,
            true,
            Some(ThumbnailStackOrigin {
                x: 80.0,
                edge: 24.0,
                anchor: ThumbnailStackAnchor::Top,
            }),
            crate::models::MiniPreviewPlacement::BottomLeft,
        );
        let expanded = thumbnail_geometry(
            work,
            3,
            false,
            Some(ThumbnailStackOrigin {
                x: 80.0,
                edge: 24.0,
                anchor: ThumbnailStackAnchor::Top,
            }),
            crate::models::MiniPreviewPlacement::BottomLeft,
        );
        assert_eq!(collapsed.y, 24.0);
        assert_eq!(expanded.y, 24.0);
        assert!(expanded.height > collapsed.height);
        assert_eq!(expanded.anchor, ThumbnailStackAnchor::Top);
        assert_eq!(
            thumbnail_window_top(24.0, 792.0, 240.0, ThumbnailStackAnchor::Top),
            24.0
        );
        assert_eq!(
            thumbnail_window_top(24.0, 792.0, 240.0, ThumbnailStackAnchor::Bottom),
            24.0 - (792.0 - 240.0)
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
    fn hides_mini_previews_when_the_preference_is_disabled() {
        assert!(thumbnail_stack_should_be_visible(1, false, true, false));
        assert!(!thumbnail_stack_should_be_visible(1, true, true, false));
        assert!(!thumbnail_stack_should_be_visible(1, false, false, false));
        assert!(!thumbnail_stack_should_be_visible(0, false, true, false));
    }

    #[test]
    fn keeps_mini_previews_visible_during_capture_when_included() {
        assert!(thumbnail_stack_should_be_visible(1, true, true, true));
        assert!(!thumbnail_stack_should_be_visible(1, true, false, true));
        assert!(!thumbnail_stack_should_be_visible(0, true, true, true));
    }

    #[test]
    fn keeps_the_minimized_stack_visible() {
        assert!(thumbnail_stack_should_be_visible(2, false, true, false));
    }

    #[test]
    fn collapsed_stack_window_fits_the_receding_pile() {
        assert_eq!(thumbnail_stack_height(1, true), 240.0);
        assert_eq!(thumbnail_stack_height(1, false), 240.0);
        assert!(thumbnail_stack_height(8, true) > thumbnail_stack_height(4, true));
        assert!(thumbnail_stack_height(8, true) < thumbnail_stack_height(8, false));
        let pose_3 = 3.0 * (24.0 + 0.55 * 3.0) / (3.0 + 24.0);
        let peek = pose_3 * 16.0;
        let extra_above_padding = f64::max(peek - 28.0, 0.0);
        assert!((thumbnail_stack_height(4, true) - (240.0 + extra_above_padding)).abs() < 1e-9);
    }

    #[test]
    fn shrinks_non_macos_thumbnail_windows_to_avoid_invisible_click_blockers() {
        assert_eq!(
            thumbnail_visible_window_height(400.0, Some(584.0), false),
            400.0
        );
    }

    #[test]
    fn preserves_thumbnail_height_while_collapsed_on_macos_and_linux() {
        assert_eq!(
            thumbnail_preserve_current_height(true),
            cfg!(target_os = "macos") || cfg!(target_os = "linux")
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
    fn maps_physical_pointer_into_scaled_thumbnail_coordinates() {
        let pointer = thumbnail_pointer_in_space(
            448.0,
            280.0,
            ThumbnailWindowFrame {
                x: 400,
                y: 200,
                width: 600,
                height: 352,
                scale: 2.0,
            },
            ThumbnailPointerSpace::PhysicalMouse,
        );
        assert_eq!(pointer.x, 24.0);
        assert_eq!(pointer.y, 40.0);
        assert!(pointer.inside);

        let outside = thumbnail_pointer_in_space(
            10.0,
            10.0,
            ThumbnailWindowFrame {
                x: 400,
                y: 200,
                width: 600,
                height: 352,
                scale: 2.0,
            },
            ThumbnailPointerSpace::PhysicalMouse,
        );
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
    fn click_through_waits_for_a_realized_window_only_on_linux() {
        assert!(click_through_applies(true));
        assert_eq!(
            click_through_applies(false),
            !cfg!(target_os = "linux"),
            "GTK panics when a window that was never shown is asked to pass clicks through"
        );
    }

    #[test]
    fn minimized_preview_stack_uses_the_requested_click_through_state() {
        assert_eq!(thumbnail_cursor_ignore_update(false, false), Some(false));
        assert_eq!(thumbnail_cursor_ignore_update(false, true), Some(true));
        assert_eq!(thumbnail_cursor_ignore_update(true, false), None);
    }

    #[test]
    fn leftover_capture_sessions_do_not_keep_escape_armed() {
        assert!(
            !captures_session::CaptureEscapeUi::from_live_surfaces(
                false, false, false, false, false
            )
            .is_armed()
        );
        assert!(
            captures_session::CaptureEscapeUi::from_live_surfaces(true, false, false, false, false)
                .is_armed()
        );
        assert!(!captures_session::windows_escape_hook_should_swallow(false));
        assert!(!captures_session::capture_escape_may_drop_intent(false));
        assert!(captures_session::capture_escape_may_drop_intent(true));
        assert!(captures_session::capture_escape_arms_on_shortcut_press());
        assert!(captures_session::capture_flow_is_current(3, 3));
        assert!(!captures_session::capture_flow_is_current(3, 4));
        assert!(!captures_session::capture_flow_is_current(0, 0));
        assert!(captures_session::capture_surface_must_revalidate_after_present());
        assert!(captures_session::shortcut_release_should_start_capture(
            0, 9
        ));
        assert!(captures_session::shortcut_release_should_start_capture(
            3, 3
        ));
        assert!(!captures_session::shortcut_release_should_start_capture(
            3, 4
        ));
        assert!(captures_session::recording_prep_must_disarm_escape_intent());
    }

    #[test]
    fn wayland_only_sessions_cannot_poll_the_thumbnail_pointer() {
        assert!(super::thumbnail_global_pointer_poll_available(false, false));
        assert!(super::thumbnail_global_pointer_poll_available(true, true));
        assert!(!super::thumbnail_global_pointer_poll_available(true, false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn leftover_mini_preview_chrome_must_not_steal_desktop_input() {
        assert!(captures_macos_window::thumbnail_refresh_must_not_force_hit_testing());
        assert!(captures_macos_window::thumbnail_passthrough_must_resign_key(true));
        assert!(!captures_macos_window::thumbnail_stale_poll_may_take_key_window());
        assert!(!captures_macos_window::thumbnail_stale_poll_may_disable_click_through());
        assert!(captures_macos_window::thumbnail_stale_poll_must_resign_key());
        assert!(!captures_macos_window::thumbnail_resign_active_may_retake_key());
        assert!(captures_macos_window::thumbnail_foreign_mouse_click_must_resign_key());
        assert!(!captures_macos_window::capture_escape_should_dispatch(
            false, false, true
        ));
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
    fn app_reopen_prioritizes_onboarding_then_editors_over_utility_windows() {
        assert_eq!(primary_app_window_priority("onboarding"), Some(0));
        assert_eq!(primary_app_window_priority("recording-editor-abc"), Some(1));
        assert_eq!(
            primary_app_window_priority("screenshot-editor-abc"),
            Some(1)
        );
        assert_eq!(primary_app_window_priority("history"), Some(2));
        assert_eq!(primary_app_window_priority("preferences"), Some(3));
        assert_eq!(primary_app_window_priority("recording-hud"), None);
        assert_eq!(primary_app_window_priority("recording-selector"), None);
        assert_eq!(primary_app_window_priority("thumbnail"), None);
    }

    #[test]
    fn interactive_launch_opens_preferences_instead_of_a_capture() {
        assert_eq!(
            interactive_launch_action(false, false),
            InteractiveLaunchAction::Onboarding
        );
        assert_eq!(
            interactive_launch_action(true, true),
            InteractiveLaunchAction::StartupNotice
        );
        assert_eq!(
            interactive_launch_action(true, false),
            InteractiveLaunchAction::Preferences
        );
    }

    #[test]
    fn reactivating_the_app_opens_preferences_when_no_durable_window_is_open() {
        assert_eq!(
            app_reactivation(false, false, false),
            AppReactivation::ShowOnboarding
        );
        assert_eq!(
            app_reactivation(true, true, false),
            AppReactivation::RestoreRecordingControls
        );
        assert_eq!(
            app_reactivation(true, false, true),
            AppReactivation::FocusExisting
        );
        assert_eq!(
            app_reactivation(true, false, false),
            AppReactivation::ShowPreferences
        );
    }

    #[test]
    fn identifies_editor_windows_for_presence_clear() {
        assert!(super::is_editor_window_label("screenshot-editor-capture-1"));
        assert!(super::is_editor_window_label("recording-editor-vid-9"));
        assert!(!super::is_editor_window_label("thumbnail"));
        assert!(!super::is_editor_window_label("history"));
        assert!(!super::is_editor_window_label("screenshot-countdown"));
    }

    #[test]
    fn late_document_page_loads_do_not_steal_focus() {
        let start = std::time::Instant::now();
        assert!(super::should_focus_document_window_on_page_load(
            None, start
        ));
        assert!(super::should_focus_document_window_on_page_load(
            Some(start),
            start + std::time::Duration::from_millis(400)
        ));
        assert!(!super::should_focus_document_window_on_page_load(
            Some(start),
            start + std::time::Duration::from_secs(5)
        ));
        assert!(super::DOCUMENT_WINDOW_LOAD_FOCUS_GRACE < std::time::Duration::from_secs(5));
    }

    #[test]
    fn already_visible_thumbnail_does_not_need_tauri_show() {
        assert!(super::thumbnail_webview_needs_tauri_show(false));
        assert!(!super::thumbnail_webview_needs_tauri_show(true));
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

    #[test]
    fn preview_file_drop_on_the_stack_is_a_rejected_self_drop() {
        assert_eq!(
            classify_preview_file_drop(false, true, true),
            PreviewFileDropLanding::PreviewStack
        );
        assert_eq!(
            classify_preview_file_drop(false, true, false),
            PreviewFileDropLanding::PreviewStack
        );
    }

    #[test]
    fn preview_file_drop_into_another_captures_window_keeps_the_card() {
        assert_eq!(
            classify_preview_file_drop(true, false, false),
            PreviewFileDropLanding::AppWindow
        );
        assert_eq!(
            classify_preview_file_drop(false, false, true),
            PreviewFileDropLanding::AppWindow
        );
        assert_eq!(
            classify_preview_file_drop(true, true, true),
            PreviewFileDropLanding::AppWindow
        );
    }

    #[test]
    fn preview_file_drop_outside_captures_is_external() {
        assert_eq!(
            classify_preview_file_drop(false, false, false),
            PreviewFileDropLanding::External
        );
    }

    #[test]
    fn retina_logical_pointer_still_hits_the_preview_stack_frame() {
        // macOS: logical mouse vs physical window origin (scale 2).
        let pointer = thumbnail_pointer_in_space(
            40.0,
            80.0,
            ThumbnailWindowFrame {
                x: 48,
                y: 120,
                width: 680,
                height: 480,
                scale: 2.0,
            },
            ThumbnailPointerSpace::LogicalMouse,
        );
        assert!(pointer.inside);
    }

    #[test]
    fn macos_drag_plugin_cursor_undoes_the_pixel_height_y_flip() {
        // drag-rs: y = pixels_high - cocoa_y. cocoa_y = 100pt from the bottom of
        // a 900pt / 1800px Retina display → reported 1700, logical top-left 800.
        let (x, y) = drag_plugin_cursor_to_pointer_space(
            40.0,
            1_700.0,
            1_800.0,
            2.0,
            ThumbnailPointerSpace::LogicalMouse,
        );
        assert_eq!(x, 40.0);
        assert_eq!(y, 800.0);
    }

    #[test]
    fn physical_drag_plugin_cursor_is_unchanged() {
        let (x, y) = drag_plugin_cursor_to_pointer_space(
            448.0,
            280.0,
            1_800.0,
            2.0,
            ThumbnailPointerSpace::PhysicalMouse,
        );
        assert_eq!((x, y), (448.0, 280.0));
    }

    fn notice_monitor(width: f64, height: f64) -> LogicalRect {
        LogicalRect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        }
    }

    fn notice_tray(x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
        LogicalRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn startup_notice_after_setup_lasts_three_times_as_long_as_autostart() {
        assert_eq!(
            STARTUP_NOTICE_AFTER_SETUP_VISIBLE,
            STARTUP_NOTICE_AUTOSTART_VISIBLE * 3
        );
    }

    #[test]
    fn startup_notice_falls_back_to_the_top_right_without_a_tray_rect() {
        let placement = place_startup_notice(notice_monitor(1440.0, 900.0), None);
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(placement.x, 1440.0 - window_width - 18.0);
        assert_eq!(placement.y, 30.0);
        assert_eq!(placement.width, window_width);
        assert_eq!(placement.height, window_height);
        assert_eq!(placement.caret, StartupNoticeCaret::None);
        assert_eq!(startup_notice_url(placement), "index.html?view=startup");
    }

    #[test]
    fn startup_notice_centers_under_a_macos_menu_bar_icon() {
        let placement = place_startup_notice(
            notice_monitor(1440.0, 900.0),
            Some(notice_tray(800.0, 0.0, 28.0, 24.0)),
        );
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        assert_eq!(placement.caret, StartupNoticeCaret::Top);
        assert_eq!(placement.width, window_width);
        assert_eq!(placement.height, window_height);
        assert_eq!(placement.x, 814.0 - window_width / 2.0);
        assert_eq!(placement.y, 24.0 - TRAY_NOTICE_TRAY_OVERLAP);
        assert_eq!(placement.caret_x, window_width / 2.0);
        assert_eq!(
            startup_notice_url(placement),
            format!(
                "index.html?view=startup&caret=top&caret_x={}",
                (window_width / 2.0).round()
            )
        );
    }

    #[test]
    fn startup_notice_keeps_a_right_edge_icon_caret_on_the_card() {
        let placement = place_startup_notice(
            notice_monitor(1440.0, 900.0),
            Some(notice_tray(1410.0, 0.0, 28.0, 24.0)),
        );
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        let caret_max = window_width - TRAY_NOTICE_FRAME_PAD - TRAY_NOTICE_CARET_INSET;
        assert_eq!(placement.caret, StartupNoticeCaret::Top);
        assert_eq!(
            placement.x,
            1440.0 - window_width - TRAY_NOTICE_SCREEN_MARGIN
        );
        assert_eq!(placement.caret_x, caret_max);
    }

    #[test]
    fn startup_notice_sits_above_a_windows_taskbar_tray_icon() {
        let placement = place_startup_notice(
            notice_monitor(1920.0, 1080.0),
            Some(notice_tray(1860.0, 1048.0, 24.0, 24.0)),
        );
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        let caret_max = window_width - TRAY_NOTICE_FRAME_PAD - TRAY_NOTICE_CARET_INSET;
        assert_eq!(placement.caret, StartupNoticeCaret::Bottom);
        assert_eq!(
            placement.x,
            1920.0 - window_width - TRAY_NOTICE_SCREEN_MARGIN
        );
        assert_eq!(
            placement.y,
            1048.0 - window_height + TRAY_NOTICE_TRAY_OVERLAP
        );
        assert_eq!(placement.caret_x, caret_max);
        assert_eq!(
            startup_notice_url(placement),
            format!(
                "index.html?view=startup&caret=bottom&caret_x={}",
                caret_max.round()
            )
        );
    }

    #[test]
    fn macos_rejects_an_unlaid_out_status_item_at_the_bottom_left() {
        let monitor = notice_monitor(1440.0, 900.0);
        assert!(!tray_icon_rect_is_usable(
            monitor,
            notice_tray(0.0, 876.0, 28.0, 24.0),
            true
        ));
        assert!(tray_icon_rect_is_usable(
            monitor,
            notice_tray(800.0, 0.0, 28.0, 24.0),
            true
        ));
        let placement = resolve_startup_notice_placement(
            monitor,
            monitor,
            Some(notice_tray(0.0, 876.0, 28.0, 24.0)),
            true,
        );
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(placement.caret, StartupNoticeCaret::None);
        assert_eq!(placement.x, 1440.0 - window_width - 18.0);
        assert_eq!(placement.y, 30.0);
    }

    #[test]
    fn macos_still_anchors_to_a_laid_out_menu_bar_icon() {
        let monitor = notice_monitor(1440.0, 900.0);
        let placement = resolve_startup_notice_placement(
            monitor,
            monitor,
            Some(notice_tray(800.0, 0.0, 28.0, 24.0)),
            true,
        );
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        assert_eq!(placement.caret, StartupNoticeCaret::Top);
        assert_eq!(placement.x, 814.0 - window_width / 2.0);
        assert_eq!(placement.y, 24.0 - TRAY_NOTICE_TRAY_OVERLAP);
    }

    #[test]
    fn windows_fallback_without_a_tray_rect_sits_above_the_taskbar() {
        let monitor = notice_monitor(1920.0, 1080.0);
        let work_area = LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        };
        let placement = fallback_startup_notice(monitor, work_area, StartupNoticeCaret::Bottom);
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(placement.caret, StartupNoticeCaret::None);
        assert_eq!(placement.x, 1920.0 - window_width - 18.0);
        assert_eq!(placement.y, 1040.0 - window_height - 18.0);
        assert!(tray_icon_rect_is_usable(
            monitor,
            notice_tray(1860.0, 1048.0, 24.0, 24.0),
            false
        ));
    }

    #[test]
    fn linux_fallback_follows_the_panel_edge_of_the_work_area() {
        let monitor = notice_monitor(1920.0, 1080.0);
        let top_panel = LogicalRect {
            x: 0.0,
            y: 28.0,
            width: 1920.0,
            height: 1052.0,
        };
        let bottom_panel = LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        };
        assert_eq!(
            startup_notice_fallback_edge_from_insets(monitor, top_panel),
            StartupNoticeCaret::Top
        );
        assert_eq!(
            startup_notice_fallback_edge_from_insets(monitor, bottom_panel),
            StartupNoticeCaret::Bottom
        );
        let below_panel = fallback_startup_notice(monitor, top_panel, StartupNoticeCaret::Top);
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(below_panel.y, 38.0);
        assert_eq!(below_panel.x, 1920.0 - window_width - 18.0);
    }

    #[test]
    fn tray_notice_window_reserves_padding_and_caret_space() {
        assert_eq!(
            tray_notice_window_size(400.0, 118.0, false),
            (
                400.0 + TRAY_NOTICE_FRAME_PAD * 2.0,
                118.0 + TRAY_NOTICE_FRAME_PAD * 2.0
            )
        );
        assert_eq!(
            tray_notice_window_size(400.0, 118.0, true),
            (
                400.0 + TRAY_NOTICE_FRAME_PAD * 2.0,
                118.0 + TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_SIZE
            )
        );
        assert_eq!(
            tray_notice_window_size(440.0, 290.0, true),
            (
                440.0 + TRAY_NOTICE_FRAME_PAD * 2.0,
                290.0 + TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_SIZE
            )
        );
    }

    #[test]
    fn tray_accelerators_use_menu_key_names_instead_of_code_tokens() {
        assert_eq!(
            tray_accelerator("CommandOrControl+Shift+Digit4").as_deref(),
            Some("CommandOrControl+Shift+4")
        );
        assert_eq!(
            tray_accelerator("Super+Shift+KeyW").as_deref(),
            Some("Super+Shift+W")
        );
        assert_eq!(tray_accelerator("").as_deref(), None);
        assert_eq!(
            tray_accelerator("Control+Shift+Space").as_deref(),
            Some("Control+Shift+Space")
        );
    }

    #[test]
    fn region_capture_uses_the_crosshair_cursor_icon() {
        use captures_capture::CaptureMode;
        use tauri::CursorIcon;

        assert_eq!(
            capture_cursor_icon(CaptureMode::Region),
            CursorIcon::Crosshair
        );
        assert_eq!(
            capture_cursor_icon(CaptureMode::Window),
            CursorIcon::Default
        );
        assert_eq!(
            capture_cursor_icon(CaptureMode::Display),
            CursorIcon::Default
        );
    }

    #[test]
    fn capturable_windows_stay_empty_when_listing_fails() {
        let targets = capturable_windows_for_display(
            Err(AppError::Task("window list unavailable".to_owned())),
            &test_display(),
            None,
        );
        assert!(targets.windows.is_empty());
        assert!(targets.shell_chrome.is_empty());
    }

    fn test_display() -> DisplayDescriptor {
        DisplayDescriptor {
            id: "display".to_owned(),
            name: "Display".to_owned(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            scale_factor: 2.0,
            is_primary: true,
        }
    }

    #[test]
    fn overlay_snapshot_shows_the_cursor_without_mutating_the_capture_source() {
        let source = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255]));
        let snapshot =
            encode_overlay_snapshot_with_cursor(&source, &test_display(), Some((10, 12)), true)
                .expect("snapshot encoded");
        let decoded = image::load_from_memory(&snapshot)
            .expect("snapshot decoded")
            .to_rgba8();

        assert!(decoded.pixels().any(|pixel| pixel.0 == [24, 24, 24, 255]));
        assert!(source.pixels().all(|pixel| pixel.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn overlay_opens_without_waiting_for_a_slow_window_list() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            started_tx.send(()).expect("listing started");
            release_rx.recv().expect("listing released");
            Ok(Vec::new())
        });
        started_rx.recv().expect("worker is blocked");
        let (targets, pending) = take_ready_or_defer_windows(Some(task), &test_display(), None);
        assert!(targets.windows.is_empty());
        assert!(targets.shell_chrome.is_empty());
        let pending = pending.expect("slow listing should be deferred");
        release_tx.send(()).expect("release listing");
        pending.join().expect("listing finished").expect("windows");
    }

    #[test]
    fn overlay_includes_windows_when_listing_already_finished() {
        let task = std::thread::spawn(|| Ok(Vec::new()));
        while !task.is_finished() {
            std::thread::yield_now();
        }
        let (targets, pending) = take_ready_or_defer_windows(Some(task), &test_display(), None);
        assert!(targets.windows.is_empty());
        assert!(pending.is_none());
    }
}
