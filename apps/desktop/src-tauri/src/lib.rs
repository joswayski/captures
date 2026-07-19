#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use std::{
    fs,
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

use tauri::CursorIcon;

use captures_capture::{CaptureError, CaptureMode, LogicalRect};
use chrono::Utc;
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
mod state;
mod storage;

use models::{
    ActiveSession, AppSettings, CaptureArtifact, CaptureSession, ClipboardCopyStatus,
    ClipboardState,
};
use state::AppState;

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
    #[error("capture session is no longer available")]
    SessionUnavailable,
    #[error("the selection must be larger than zero pixels")]
    InvalidSelection,
    #[error("shortcut registration failed: {0}")]
    Shortcut(String),
    #[error("background task failed: {0}")]
    Task(String),
}

type CommandResult<T> = Result<T, String>;

#[cfg(target_os = "macos")]
struct CaptureTrayMenuItems {
    region: MenuItem<tauri::Wry>,
    window: MenuItem<tauri::Wry>,
    display: MenuItem<tauri::Wry>,
}

#[cfg(target_os = "macos")]
impl CaptureTrayMenuItems {
    fn set_shortcuts(&self, settings: &AppSettings) -> Result<(), AppError> {
        self.region
            .set_accelerator(Some(menu_accelerator(&settings.region_shortcut)?))?;
        self.window
            .set_accelerator(Some(menu_accelerator(&settings.window_shortcut)?))?;
        self.display
            .set_accelerator(Some(menu_accelerator(&settings.display_shortcut)?))?;
        Ok(())
    }
}

pub fn run() {
    let state = AppState::new();
    let protocol_state = state.clone();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("preferences") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Captures")
                .build(),
        );

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(captures_macos_window::init_panel_plugin());

    builder
        .manage(state)
        .register_uri_scheme_protocol("captures-capture", move |_context, request| {
            let path = request.uri().path().trim_matches('/');
            let body = resolve_asset(&protocol_state, path);
            match body {
                Some(bytes) => tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", "image/png")
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
            get_active_session,
            get_pending_session,
            get_settings,
            update_settings,
            get_artifacts,
            get_artifact,
            get_clipboard_state,
            copy_artifact,
            save_artifact,
            reveal_artifact,
            trash_artifact,
            dismiss_artifact,
            open_artifact_viewer,
            show_capture_overlay,
            reveal_capture_overlay,
            sync_capture_cursor,
            thumbnail_ready,
            sync_thumbnail_stack,
            get_thumbnail_pointer_position,
            set_thumbnail_cursor,
            reassert_thumbnail_cursor,
            set_thumbnail_ignore_cursor_events,
            open_captures_folder,
            open_preferences,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            setup_tray(app)?;
            let handle = app.handle().clone();
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
            if pending_capture.is_none()
                && let Err(error) = show_startup_notice(&handle)
            {
                eprintln!("failed to show startup notice: {error}");
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
        .run(|_, event| {
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit();
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
    if !state.sessions.lock().is_empty() {
        return Err(AppError::CaptureInProgress);
    }

    begin_thumbnail_capture(&state)?;
    set_capture_huds_protected(&app, true);
    hide_window(&app, "thumbnail");
    hide_window(&app, "startup");
    let result = prepare_capture(app.clone(), state.clone(), mode).await;
    set_capture_huds_protected(&app, false);
    if result.is_err() {
        restore_thumbnail_stack(&app, &state);
    }
    result
}

async fn prepare_capture(
    app: AppHandle,
    state: Arc<AppState>,
    mode: CaptureMode,
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

    if mode == CaptureMode::Display {
        let _ = finish_capture(&app, &state, mode, frame.image).await?;
        return Ok(None);
    }

    let id = Uuid::new_v4();
    let snapshot_png = storage::encode_preview_png(&frame.image, frame.descriptor.scale_factor)?;
    let windows = if mode == CaptureMode::Window {
        state
            .windows()?
            .into_iter()
            .filter(|window| {
                window.display_id == display.id
                    && window
                        .app_name
                        .as_deref()
                        .is_none_or(|app_name| !app_name.eq_ignore_ascii_case("Captures"))
            })
            .collect()
    } else {
        Vec::new()
    };
    let session = CaptureSession {
        id,
        mode,
        display: frame.descriptor,
        image: frame.image,
        snapshot_png,
        windows,
    };
    let active = ActiveSession {
        id: id.to_string(),
        mode,
        window_coordinate_scale: window_coordinate_scale(&session.display),
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
    let physical = rect.to_physical(
        session.display.scale_factor,
        session.image.width(),
        session.image.height(),
    );
    let Some(image) = session.view(physical) else {
        restore_thumbnail_stack(&app, &state);
        return Err(AppError::InvalidSelection.to_string());
    };

    let result = finish_capture(&app, &state, CaptureMode::Region, image).await;
    if result.is_err() {
        restore_thumbnail_stack(&app, &state);
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
    state
        .sessions
        .lock()
        .remove(&id)
        .ok_or_else(|| AppError::SessionUnavailable.to_string())?;
    let image = match state.backend.capture_window(&window_id) {
        Ok(image) => image,
        Err(error) => {
            restore_thumbnail_stack(&app, &state);
            return Err(error.to_string());
        }
    };

    let result = finish_capture(&app, &state, CaptureMode::Window, image).await;
    if result.is_err() {
        restore_thumbnail_stack(&app, &state);
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
    state.sessions.lock().remove(&id);
    restore_thumbnail_stack(&app, state.inner());
    Ok(())
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

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThumbnailCursorAction {
    Ignore,
    Reset,
    Apply(bool),
}

#[cfg(any(target_os = "macos", test))]
fn thumbnail_cursor_action(
    suppressed: bool,
    visible: bool,
    pointing: bool,
) -> ThumbnailCursorAction {
    if suppressed {
        ThumbnailCursorAction::Ignore
    } else if visible {
        ThumbnailCursorAction::Apply(pointing)
    } else {
        ThumbnailCursorAction::Reset
    }
}

#[tauri::command]
fn set_thumbnail_cursor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    pointing: bool,
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
            let result = match thumbnail_cursor_action(suppressed, visible, pointing) {
                // NSCursor is application-wide. Do not even invalidate the
                // hidden preview's cursor rectangles while capture owns it.
                ThumbnailCursorAction::Ignore => return,
                ThumbnailCursorAction::Reset => {
                    let _ = cursor_window.set_cursor_icon(CursorIcon::Default);
                    captures_macos_window::reset_pointing_cursor_state(&cursor_window)
                }
                ThumbnailCursorAction::Apply(effective_pointing) => {
                    let icon = if effective_pointing {
                        CursorIcon::Hand
                    } else {
                        CursorIcon::Default
                    };
                    if let Err(error) = cursor_window.set_cursor_icon(icon) {
                        eprintln!("failed to update capture thumbnail window cursor: {error}");
                    }
                    captures_macos_window::set_pointing_cursor(&cursor_window, effective_pointing)
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
        let _ = (app, state, pointing);
        Ok(())
    }
}

#[tauri::command]
fn reassert_thumbnail_cursor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
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
            let result = match thumbnail_cursor_action(suppressed, visible, true) {
                ThumbnailCursorAction::Ignore => return,
                ThumbnailCursorAction::Reset => {
                    let _ = window.set_cursor_icon(CursorIcon::Default);
                    captures_macos_window::reset_pointing_cursor_state(&window)
                }
                ThumbnailCursorAction::Apply(_) => {
                    captures_macos_window::reassert_pointing_cursor(&window)
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
        let _ = (app, state);
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
    if settings.region_shortcut.trim().is_empty()
        || settings.window_shortcut.trim().is_empty()
        || settings.display_shortcut.trim().is_empty()
    {
        return Err("all shortcuts must be set".to_owned());
    }
    let region_shortcut =
        parse_shortcut(&settings.region_shortcut).map_err(|error| error.to_string())?;
    let window_shortcut =
        parse_shortcut(&settings.window_shortcut).map_err(|error| error.to_string())?;
    let display_shortcut =
        parse_shortcut(&settings.display_shortcut).map_err(|error| error.to_string())?;
    if region_shortcut == window_shortcut
        || region_shortcut == display_shortcut
        || window_shortcut == display_shortcut
    {
        return Err("shortcuts must be unique".to_owned());
    }

    // Permission bookkeeping is internal state, not a user-editable setting.
    let previous_settings = state.settings();
    settings.last_screen_permission_request_id =
        previous_settings.last_screen_permission_request_id.clone();
    settings.pending_capture_after_restart = previous_settings.pending_capture_after_restart;

    let shortcuts_changed = settings.region_shortcut != previous_settings.region_shortcut
        || settings.window_shortcut != previous_settings.window_shortcut
        || settings.display_shortcut != previous_settings.display_shortcut;
    if shortcuts_changed && let Err(error) = register_shortcuts_with(&app, &settings) {
        let _ = register_shortcuts_with(&app, &previous_settings);
        return Err(error.to_string());
    }
    #[cfg(target_os = "macos")]
    if shortcuts_changed {
        let tray_items = app.state::<CaptureTrayMenuItems>();
        if let Err(error) = tray_items.set_shortcuts(&settings) {
            let _ = register_shortcuts_with(&app, &previous_settings);
            let _ = tray_items.set_shortcuts(&previous_settings);
            return Err(error.to_string());
        }
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
    *state.settings.write() = settings.clone();
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

#[tauri::command]
fn get_clipboard_state(state: tauri::State<'_, Arc<AppState>>) -> ClipboardState {
    let revision = current_clipboard_revision();
    let artifact_id = state.clipboard_ownership.lock().current_artifact(revision);
    let artifact_id = artifact_id.filter(|artifact_id| {
        state
            .artifacts
            .lock()
            .iter()
            .any(|artifact| artifact.id == *artifact_id)
    });
    ClipboardState {
        revision,
        artifact_id,
    }
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
    let revision = copy_to_clipboard(&app, image)
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
    state
        .clipboard_ownership
        .lock()
        .record(revision, artifact_id.clone());
    app.emit("artifact-updated", &artifact)
        .map_err(|error| error.to_string())?;
    app.emit(
        "clipboard-owner-changed",
        &ClipboardState {
            revision,
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

#[tauri::command]
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

#[tauri::command]
fn open_preferences(app: AppHandle) -> CommandResult<()> {
    show_preferences(&app);
    Ok(())
}

async fn finish_capture(
    app: &AppHandle,
    state: &Arc<AppState>,
    mode: CaptureMode,
    image: RgbaImage,
) -> Result<CaptureArtifact, AppError> {
    let width = image.width();
    let height = image.height();
    let image_for_encoding = image.clone();
    let encode_task = tauri::async_runtime::spawn_blocking(move || -> Result<_, AppError> {
        let image_png = storage::encode_png(&image_for_encoding)?;
        let preview_png = storage::encode_thumbnail_png(&image_for_encoding)?;
        Ok((image_png, preview_png))
    });
    let clipboard_task = state.settings().auto_copy_to_clipboard.then(|| {
        let clipboard_app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            write_image_to_clipboard(&clipboard_app, image)
        })
    });
    let (image_png, preview_png) = encode_task
        .await
        .map_err(|error| AppError::Task(error.to_string()))??;
    let size_bytes = u64::try_from(image_png.len()).unwrap_or(u64::MAX);
    let artifact_id = Uuid::new_v4().to_string();
    let mut artifact = CaptureArtifact {
        id: artifact_id.clone(),
        preview_url: models::artifact_url(&artifact_id),
        full_url: models::artifact_full_url(&artifact_id),
        path: None,
        width,
        height,
        size_bytes,
        created_at: Utc::now().to_rfc3339(),
        mode,
        clipboard_copy_status: if clipboard_task.is_some() {
            ClipboardCopyStatus::Pending
        } else {
            ClipboardCopyStatus::Skipped
        },
        image_png,
        preview_png,
    };
    state.artifacts.lock().push(artifact.clone());
    {
        let mut visibility = state.thumbnail_visibility.lock();
        visibility.wait_for_artifact(artifact.id.clone());
    }
    app.emit("capture-completed", &artifact)?;

    if let Some(clipboard_task) = clipboard_task {
        let clipboard_result = clipboard_task
            .await
            .map_err(|error| AppError::Task(error.to_string()))?;
        artifact.clipboard_copy_status = match clipboard_result {
            Ok(revision) => {
                state
                    .clipboard_ownership
                    .lock()
                    .record(revision, artifact.id.clone());
                app.emit(
                    "clipboard-owner-changed",
                    &ClipboardState {
                        revision,
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

async fn copy_to_clipboard(app: &AppHandle, image: RgbaImage) -> Result<isize, AppError> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || write_image_to_clipboard(&app, image))
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
}

fn write_image_to_clipboard(app: &AppHandle, image: RgbaImage) -> Result<isize, AppError> {
    let width = image.width();
    let height = image.height();
    let rgba = image.into_raw();
    let clipboard_image = Image::new_owned(rgba, width, height);
    app.clipboard()
        .write_image(&clipboard_image)
        .map_err(|error| AppError::Clipboard(error.to_string()))?;
    Ok(record_clipboard_write())
}

#[cfg(target_os = "macos")]
fn current_clipboard_revision() -> isize {
    captures_macos_window::clipboard_change_count()
}

#[cfg(target_os = "macos")]
fn record_clipboard_write() -> isize {
    current_clipboard_revision()
}

#[cfg(not(target_os = "macos"))]
static APPLICATION_CLIPBOARD_REVISION: AtomicIsize = AtomicIsize::new(0);

#[cfg(not(target_os = "macos"))]
fn current_clipboard_revision() -> isize {
    APPLICATION_CLIPBOARD_REVISION.load(Ordering::Acquire)
}

#[cfg(not(target_os = "macos"))]
fn record_clipboard_write() -> isize {
    APPLICATION_CLIPBOARD_REVISION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1)
}

fn display_under_pointer(
    state: &AppState,
) -> Result<captures_capture::DisplayDescriptor, AppError> {
    let (x, y) = match Mouse::get_mouse_position() {
        Mouse::Position { x, y } => (x, y),
        Mouse::Error => (0, 0),
    };
    let displays = state.monitors()?;
    displays
        .iter()
        .find(|display| {
            x >= display.x
                && y >= display.y
                && x < display.x + display.width as i32
                && y < display.y + display.height as i32
        })
        .cloned()
        .or_else(|| displays.iter().find(|display| display.is_primary).cloned())
        .or_else(|| displays.first().cloned())
        .ok_or(CaptureError::TargetUnavailable.into())
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

fn register_shortcuts(app: &AppHandle) -> Result<(), AppError> {
    let settings = app.state::<Arc<AppState>>().settings();
    register_shortcuts_with(app, &settings)
}

fn register_shortcuts_with(app: &AppHandle, settings: &AppSettings) -> Result<(), AppError> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|error| AppError::Shortcut(error.to_string()))?;
    register_shortcut(app, &settings.region_shortcut, CaptureMode::Region)?;
    register_shortcut(app, &settings.window_shortcut, CaptureMode::Window)?;
    register_shortcut(app, &settings.display_shortcut, CaptureMode::Display)?;
    Ok(())
}

fn register_shortcut(app: &AppHandle, shortcut: &str, mode: CaptureMode) -> Result<(), AppError> {
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
            let state = app.state::<Arc<AppState>>().inner().clone();
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

#[cfg(any(target_os = "macos", test))]
fn menu_accelerator(shortcut: &str) -> Result<String, AppError> {
    parse_shortcut(shortcut).map(|shortcut| shortcut.to_string())
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
    #[cfg(target_os = "macos")]
    let (region_accelerator, window_accelerator, display_accelerator) = {
        let settings = app.state::<Arc<AppState>>().settings();
        (
            Some(menu_accelerator(&settings.region_shortcut)?),
            Some(menu_accelerator(&settings.window_shortcut)?),
            Some(menu_accelerator(&settings.display_shortcut)?),
        )
    };
    #[cfg(not(target_os = "macos"))]
    let (region_accelerator, window_accelerator, display_accelerator) =
        (None::<String>, None::<String>, None::<String>);

    let capture_region = MenuItem::with_id(
        app,
        "capture-region",
        "Capture Region",
        true,
        region_accelerator.as_deref(),
    )?;
    let capture_window = MenuItem::with_id(
        app,
        "capture-window",
        "Capture Window",
        true,
        window_accelerator.as_deref(),
    )?;
    let capture_display = MenuItem::with_id(
        app,
        "capture-display",
        "Capture Full Screen",
        true,
        display_accelerator.as_deref(),
    )?;
    let open_folder =
        MenuItem::with_id(app, "open-folder", "Open Save Location", true, None::<&str>)?;
    let preferences = MenuItem::with_id(app, "preferences", "Preferences", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Captures", true, None::<&str>)?;
    let separator_1 = MenuItem::with_id(app, "separator-1", "────────", false, None::<&str>)?;
    let separator_2 = MenuItem::with_id(app, "separator-2", "────────", false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &capture_region,
            &capture_window,
            &capture_display,
            &separator_1,
            &open_folder,
            &preferences,
            &separator_2,
            &quit,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("Captures — Screenshot utility");

    #[cfg(target_os = "macos")]
    if let Some(icon) = macos_tray_icon() {
        tray = tray.icon(icon).icon_as_template(true);
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    tray.on_menu_event(|app, event| {
        let mode = match event.id().as_ref() {
            "capture-region" => Some(CaptureMode::Region),
            "capture-window" => Some(CaptureMode::Window),
            "capture-display" => Some(CaptureMode::Display),
            "open-folder" => {
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    let path = PathBuf::from(state.settings().output_directory);
                    let _ = fs::create_dir_all(&path);
                    let _ = app.opener().open_path(path.to_string_lossy(), None::<&str>);
                }
                None
            }
            "preferences" => {
                show_preferences(app);
                None
            }
            "quit" => {
                app.exit(0);
                None
            }
            _ => None,
        };
        if let Some(mode) = mode {
            let state = app.state::<Arc<AppState>>().inner().clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = start_capture_inner(app.clone(), state, mode).await {
                    report_capture_error(&app, &error, mode);
                }
            });
        }
    })
    .build(app)?;

    #[cfg(target_os = "macos")]
    if !app.manage(CaptureTrayMenuItems {
        region: capture_region,
        window: capture_window,
        display: capture_display,
    }) {
        return Err(Box::new(AppError::Task(
            "capture tray menu shortcuts are already managed".to_owned(),
        )));
    }
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
        #[cfg(target_os = "windows")]
        let (x, y, width, height) = {
            let scale = display.scale_factor.max(1.0);
            (
                f64::from(display.x) / scale,
                f64::from(display.y) / scale,
                f64::from(display.width) / scale,
                f64::from(display.height) / scale,
            )
        };
        #[cfg(not(target_os = "windows"))]
        let (x, y, width, height) = (
            f64::from(display.x),
            f64::from(display.y),
            f64::from(display.width),
            f64::from(display.height),
        );
        if handle.get_webview_window("overlay").is_none()
            && let Err(error) = create_overlay_window(&handle)
        {
            eprintln!("failed to create capture overlay: {error}");
            return;
        }
        if let Some(window) = handle.get_webview_window("overlay") {
            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
            let _ = window.set_size(LogicalSize::new(width, height));
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
        let Some(window) = handle.get_webview_window("thumbnail") else {
            if let Err(error) = create_thumbnail_window(&handle, count > 0 && !suppressed) {
                eprintln!("failed to create capture thumbnail stack: {error}");
            }
            return;
        };
        if count == 0 || suppressed {
            let _ = window.hide();
            return;
        }
        let (x, y, desired_height) = thumbnail_window_geometry(&handle, count);
        let visible = window.is_visible().unwrap_or(false);
        // WKWebView blanks every painted card when its NSWindow shrinks. Keep
        // the taller frame after dismissals; the stack is bottom-anchored and
        // empty top space is click-through. Exact height is restored the next
        // time the window is shown from hidden.
        let height = thumbnail_visible_window_height(
            desired_height,
            visible
                .then(|| thumbnail_window_logical_height(&window))
                .flatten(),
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

fn thumbnail_window_logical_height(window: &tauri::WebviewWindow) -> Option<f64> {
    let scale = window.scale_factor().ok()?.max(1.0);
    let size = window.inner_size().ok()?;
    Some(f64::from(size.height) / scale)
}

/// While the stack is on-screen, only grow it. Shrinking after a card exits
/// forces WKWebView to recompose and flickers every surviving preview.
fn thumbnail_visible_window_height(desired: f64, current: Option<f64>) -> f64 {
    match current {
        Some(current) => desired.max(current),
        None => desired,
    }
}

fn show_thumbnail_window(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    if let Err(error) = captures_macos_window::show_without_activating(window) {
        eprintln!("failed to raise capture thumbnail stack: {error}");
    }

    #[cfg(not(target_os = "macos"))]
    let _ = window.show();
}

fn restore_thumbnail_stack(app: &AppHandle, state: &Arc<AppState>) {
    {
        state.thumbnail_visibility.lock().restore();
    }
    update_thumbnail_stack(app);
}

fn begin_thumbnail_capture(state: &Arc<AppState>) -> Result<(), AppError> {
    if !state.thumbnail_visibility.lock().begin_capture() {
        return Err(AppError::CaptureInProgress);
    }
    Ok(())
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
            let position = monitor.position();
            let size = monitor.size();
            thumbnail_geometry(
                position.x,
                position.y,
                size.width,
                size.height,
                monitor.scale_factor(),
                count,
            )
        })
        .unwrap_or((20.0, 20.0, thumbnail_stack_height(count)))
}

fn thumbnail_stack_height(count: usize) -> f64 {
    let cards = count.max(1) as f64;
    THUMBNAIL_PADDING * 2.0 + cards * THUMBNAIL_CARD_HEIGHT + (cards - 1.0) * THUMBNAIL_GAP
}

fn thumbnail_geometry(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
    count: usize,
) -> (f64, f64, f64) {
    const EDGE_MARGIN: f64 = THUMBNAIL_PADDING;

    let scale = scale_factor.max(1.0);
    let left = f64::from(x) / scale;
    let top = f64::from(y) / scale;
    let width = f64::from(width) / scale;
    let available_height = f64::from(height) / scale - EDGE_MARGIN * 2.0;
    let stack_height = thumbnail_stack_height(count).min(available_height.max(1.0));
    let left_aligned = left + EDGE_MARGIN - THUMBNAIL_PADDING;
    let bottom_aligned =
        top + f64::from(height) / scale - stack_height - EDGE_MARGIN + THUMBNAIL_PADDING;
    (
        left_aligned
            .min(left + width - THUMBNAIL_WIDTH - EDGE_MARGIN + THUMBNAIL_PADDING)
            .max(left),
        bottom_aligned.max(top + EDGE_MARGIN),
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

fn set_capture_huds_protected(app: &AppHandle, protected: bool) {
    // The window server may still composite a just-hidden HUD into an
    // immediate display capture. Exclude Captures HUDs until the frozen background
    // frame has been read so they cannot reappear as pixels during fade-in.
    for label in ["thumbnail", "startup"] {
        if let Some(window) = app.get_webview_window(label)
            && let Err(error) = window.set_content_protected(protected)
        {
            eprintln!("failed to update {label} capture protection: {error}");
        }
    }
}

fn hide_capture_overlay(app: &AppHandle) {
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use tauri_plugin_global_shortcut::ShortcutState;

    use super::{
        CaptureMode, ThumbnailCursorAction, menu_accelerator, parse_shortcut,
        should_activate_capture_cursor_before_reveal, should_trigger_shortcut,
        thumbnail_cursor_action, thumbnail_geometry, thumbnail_pointer_position,
        thumbnail_visible_window_height, viewer_window_label,
    };

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
    fn gives_each_artifact_a_stable_viewer_window() {
        assert_eq!(viewer_window_label("first"), "viewer-first");
        assert_eq!(viewer_window_label("second"), "viewer-second");
        assert_ne!(viewer_window_label("first"), viewer_window_label("second"));
    }

    #[test]
    fn ignores_preview_cursor_updates_while_capture_is_active() {
        assert_eq!(
            thumbnail_cursor_action(true, false, false),
            ThumbnailCursorAction::Ignore
        );
        assert_eq!(
            thumbnail_cursor_action(true, true, true),
            ThumbnailCursorAction::Ignore
        );
        assert_eq!(
            thumbnail_cursor_action(false, true, true),
            ThumbnailCursorAction::Apply(true)
        );
    }

    #[test]
    fn treats_legacy_and_recorded_shortcut_formats_as_the_same_combination() {
        assert_eq!(
            parse_shortcut("Ctrl+Shift+4").expect("legacy shortcut should parse"),
            parse_shortcut("Control+Shift+Digit4").expect("recorded shortcut should parse")
        );
    }

    #[test]
    fn normalizes_shortcuts_for_native_menu_accelerators() {
        assert_eq!(
            menu_accelerator("Ctrl+Shift+4").expect("legacy shortcut should normalize"),
            menu_accelerator("Control+Shift+Digit4").expect("recorded shortcut should normalize")
        );
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
    fn stacks_thumbnails_upward_in_logical_pixels_on_retina_displays() {
        assert_eq!(
            thumbnail_geometry(0, 0, 3_992, 2_048, 2.0, 1),
            (0.0, 808.0, 216.0)
        );
        assert_eq!(
            thumbnail_geometry(-3_840, 0, 3_840, 2_048, 2.0, 2),
            (-1_920.0, 624.0, 400.0)
        );
    }

    #[test]
    fn keeps_visible_thumbnail_window_from_shrinking_after_dismiss() {
        assert_eq!(thumbnail_visible_window_height(400.0, Some(584.0)), 584.0);
        assert_eq!(thumbnail_visible_window_height(584.0, Some(400.0)), 584.0);
        assert_eq!(thumbnail_visible_window_height(216.0, None), 216.0);
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
}
