#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use std::{fs, path::PathBuf, sync::Arc};

use ces_capture::{CaptureError, CaptureMode, LogicalRect};
use chrono::Utc;
use image::RgbaImage;
use mouse_position::mouse_position::Mouse;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    window::Color,
};
use tauri_plugin_autostart::ManagerExt as AutoStartExt;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use thiserror::Error;
use uuid::Uuid;

mod models;
mod state;
mod storage;

use models::{ActiveSession, AppSettings, CaptureArtifact, CaptureSession};
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

pub fn run() {
    let state = AppState::new();
    let protocol_state = state.clone();

    tauri::Builder::default()
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
                .app_name("CES")
                .build(),
        )
        .manage(state)
        .register_uri_scheme_protocol("ces-capture", move |_context, request| {
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
            copy_artifact,
            save_artifact,
            reveal_artifact,
            trash_artifact,
            dismiss_artifact,
            open_artifact_viewer,
            show_capture_overlay,
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
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building CES")
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

    hide_window(&app, "thumbnail");
    let result = prepare_capture(app.clone(), state.clone(), mode).await;
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
    state.backend.ensure_permission()?;
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
                        .is_none_or(|app_name| !app_name.eq_ignore_ascii_case("CES"))
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
    hide_window(&app, "overlay");
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
    hide_window(&app, "overlay");
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
    hide_window(&app, "overlay");
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
    if !state.sessions.lock().contains_key(&id) {
        return Err(AppError::SessionUnavailable.to_string());
    }
    if let Some(window) = app.get_webview_window("overlay") {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        Ok(())
    } else {
        Err("capture overlay is unavailable".to_owned())
    }
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, Arc<AppState>>) -> AppSettings {
    state.settings()
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
    if settings.region_shortcut == settings.window_shortcut
        || settings.region_shortcut == settings.display_shortcut
        || settings.window_shortcut == settings.display_shortcut
    {
        return Err("shortcuts must be unique".to_owned());
    }

    register_shortcuts_with(&app, &settings).map_err(|error| error.to_string())?;
    if settings.launch_at_login {
        app.autolaunch()
            .enable()
            .map_err(|error| error.to_string())?;
    } else {
        app.autolaunch()
            .disable()
            .map_err(|error| error.to_string())?;
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
    copy_to_clipboard(&app, image)
        .await
        .map_err(|error| error.to_string())
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
    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
        .ok_or_else(|| "artifact is no longer available".to_owned())?;

    if let Some(window) = app.get_webview_window("viewer") {
        app.emit("viewer-artifact-changed", &artifact)
            .map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    WebviewWindowBuilder::new(
        &app,
        "viewer",
        WebviewUrl::App(format!("index.html?view=viewer&artifact_id={artifact_id}").into()),
    )
    .title("CES Preview")
    .inner_size(1_000.0, 700.0)
    .min_inner_size(560.0, 400.0)
    .center()
    .resizable(true)
    .focused(true)
    .build()
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn remove_artifact(app: &AppHandle, state: &Arc<AppState>, artifact_id: &str) -> CommandResult<()> {
    let count = {
        let mut artifacts = state.artifacts.lock();
        let original_len = artifacts.len();
        artifacts.retain(|artifact| artifact.id != artifact_id);
        if artifacts.len() == original_len {
            return Err("artifact is no longer available".to_owned());
        }
        artifacts.len()
    };
    app.emit("artifact-removed", artifact_id)
        .map_err(|error| error.to_string())?;
    update_thumbnail_stack(app, count);
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
    let clipboard_app = app.clone();
    let clipboard_task = tauri::async_runtime::spawn_blocking(move || {
        write_image_to_clipboard(&clipboard_app, image)
    });
    let (image_png, preview_png) = encode_task
        .await
        .map_err(|error| AppError::Task(error.to_string()))??;
    let artifact_id = Uuid::new_v4().to_string();
    let mut artifact = CaptureArtifact {
        id: artifact_id.clone(),
        preview_url: models::artifact_url(&artifact_id),
        full_url: models::artifact_full_url(&artifact_id),
        path: None,
        width,
        height,
        created_at: Utc::now().to_rfc3339(),
        mode,
        clipboard_copied: true,
        image_png,
        preview_png,
    };
    state.artifacts.lock().push(artifact.clone());
    app.emit("capture-completed", &artifact)?;
    show_thumbnail(app);

    let copied = clipboard_task
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
        .is_ok();
    if !copied {
        artifact.clipboard_copied = false;
        if let Some(stored) = state
            .artifacts
            .lock()
            .iter_mut()
            .find(|stored| stored.id == artifact.id)
        {
            stored.clipboard_copied = false;
        }
        app.emit("artifact-updated", &artifact)?;
    }
    Ok(artifact)
}

async fn copy_to_clipboard(app: &AppHandle, image: RgbaImage) -> Result<(), AppError> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || write_image_to_clipboard(&app, image))
        .await
        .map_err(|error| AppError::Task(error.to_string()))?
}

fn write_image_to_clipboard(app: &AppHandle, image: RgbaImage) -> Result<(), AppError> {
    let width = image.width();
    let height = image.height();
    let rgba = image.into_raw();
    let clipboard_image = Image::new_owned(rgba, width, height);
    app.clipboard()
        .write_image(&clipboard_image)
        .map_err(|error| AppError::Clipboard(error.to_string()))
}

fn display_under_pointer(state: &AppState) -> Result<ces_capture::DisplayDescriptor, AppError> {
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

fn window_coordinate_scale(display: &ces_capture::DisplayDescriptor) -> f64 {
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
    let shortcut = shortcut.to_owned();
    let parsed = shortcut
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .map_err(|error| AppError::Shortcut(error.to_string()))?;
    app.global_shortcut()
        .on_shortcut(parsed, move |app, _shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let state = app.state::<Arc<AppState>>().inner().clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = start_capture_inner(app.clone(), state, mode).await {
                    report_capture_error(&app, &error);
                }
            });
        })
        .map_err(|error| AppError::Shortcut(error.to_string()))
}

fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let capture_region =
        MenuItem::with_id(app, "capture-region", "Capture Region", true, None::<&str>)?;
    let capture_window =
        MenuItem::with_id(app, "capture-window", "Capture Window", true, None::<&str>)?;
    let capture_display = MenuItem::with_id(
        app,
        "capture-display",
        "Capture Display",
        true,
        None::<&str>,
    )?;
    let open_folder = MenuItem::with_id(
        app,
        "open-folder",
        "Open Captures Folder",
        true,
        None::<&str>,
    )?;
    let preferences = MenuItem::with_id(app, "preferences", "Preferences", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit CES", true, None::<&str>)?;
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
    TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("CES")
        .on_menu_event(|app, event| {
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
                        report_capture_error(&app, &error);
                    }
                });
            }
        })
        .build(app)?;
    Ok(())
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
    .title("CES Capture")
    .inner_size(1.0, 1.0)
    .position(-10_000.0, -10_000.0);
    #[cfg(target_os = "linux")]
    let builder = if wayland_session() {
        builder.fullscreen(true)
    } else {
        builder
    };
    builder
        .decorations(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build()
        .map(|_| ())
}

fn show_thumbnail(app: &AppHandle) {
    let count = app.state::<Arc<AppState>>().artifacts.lock().len();
    update_thumbnail_stack(app, count);
}

fn create_thumbnail_window(app: &AppHandle, visible: bool) -> Result<(), tauri::Error> {
    let (x, y, height) = thumbnail_window_geometry(app, 1);
    WebviewWindowBuilder::new(
        app,
        "thumbnail",
        WebviewUrl::App("index.html?view=thumbnail".into()),
    )
    .title("CES Capture")
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
    .visible(visible)
    .build()
    .map(|_| ())
}

const THUMBNAIL_WIDTH: f64 = 300.0;
const THUMBNAIL_CARD_HEIGHT: f64 = 160.0;
const THUMBNAIL_GAP: f64 = 8.0;
const THUMBNAIL_PADDING: f64 = 8.0;

fn update_thumbnail_stack(app: &AppHandle, count: usize) {
    let app = app.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window("thumbnail") else {
            if let Err(error) = create_thumbnail_window(&handle, count > 0) {
                eprintln!("failed to create capture thumbnail stack: {error}");
            }
            return;
        };
        if count == 0 {
            let _ = window.hide();
            return;
        }
        let (x, y, height) = thumbnail_window_geometry(&handle, count);
        let _ = window.set_size(LogicalSize::new(THUMBNAIL_WIDTH, height));
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
        let _ = window.show();
    });
}

fn restore_thumbnail_stack(app: &AppHandle, state: &Arc<AppState>) {
    let count = state.artifacts.lock().len();
    update_thumbnail_stack(app, count);
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
    const EDGE_MARGIN: f64 = 24.0;

    let scale = scale_factor.max(1.0);
    let left = f64::from(x) / scale;
    let top = f64::from(y) / scale;
    let width = f64::from(width) / scale;
    let available_height = f64::from(height) / scale - EDGE_MARGIN * 2.0;
    let stack_height = thumbnail_stack_height(count).min(available_height.max(1.0));
    let left_aligned = left + EDGE_MARGIN;
    let bottom_aligned = top + f64::from(height) / scale - stack_height - EDGE_MARGIN;
    (
        left_aligned
            .min(left + width - THUMBNAIL_WIDTH - EDGE_MARGIN)
            .max(left),
        bottom_aligned.max(top + EDGE_MARGIN),
        stack_height,
    )
}

fn report_capture_error(app: &AppHandle, error: &AppError) {
    eprintln!("capture failed: {error}");

    #[cfg(target_os = "macos")]
    if matches!(error, AppError::Capture(CaptureError::PermissionDenied)) {
        let app = app.clone();
        app.dialog()
            .message(
                "CES needs Screen Recording permission to capture your open windows. Click Open System Settings, turn on CES under Screen & System Audio Recording, then relaunch CES.",
            )
            .title("CES Setup")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Open System Settings".to_owned(),
                "Not Now".to_owned(),
            ))
            .kind(MessageDialogKind::Error)
            .show(move |open_settings| {
                if open_settings {
                    const SCREEN_RECORDING_SETTINGS_URL: &str =
                        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
                    if let Err(error) = app
                        .opener()
                        .open_url(SCREEN_RECORDING_SETTINGS_URL, None::<&str>)
                    {
                        eprintln!("failed to open Screen Recording settings: {error}");
                    }
                }
            });
        return;
    }

    let message = capture_error_message(error);
    app.dialog()
        .message(message)
        .title("CES Capture")
        .buttons(MessageDialogButtons::Ok)
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

fn capture_error_message(error: &AppError) -> String {
    if matches!(error, AppError::Capture(CaptureError::Unsupported)) {
        #[cfg(target_os = "linux")]
        if wayland_session() {
            return "Window capture is not available on a pure Wayland session yet. Use Region or Display capture, or log in to an X11 session for Window capture. Region and Display capture use your desktop screenshot portal.".to_owned();
        }

        return "This capture mode is not supported on the current desktop session. Try Region capture instead.".to_owned();
    }

    #[cfg(target_os = "linux")]
    if wayland_session() && matches!(error, AppError::Capture(CaptureError::Backend(_))) {
        return "CES could not capture this Wayland desktop. Make sure an xdg-desktop-portal screenshot backend is installed and running, then try Region or Display capture again.".to_owned();
    }

    if matches!(error, AppError::Capture(CaptureError::PermissionDenied)) {
        #[cfg(target_os = "windows")]
        {
            return "CES could not access the screen. Windows desktop capture does not use a separate Screen Recording permission; secure/UAC windows and protected content cannot be captured.".to_owned();
        }

        return "CES needs Screen Recording permission to capture your open windows. Enable it in your operating system's privacy settings, then restart CES.".to_owned();
    }

    format!("CES could not start the capture: {error}")
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
        .title("CES Preferences")
        .inner_size(520.0, 480.0)
        .center()
        .resizable(false)
        .build();
        if let Err(error) = result {
            eprintln!("failed to create preferences window: {error}");
        }
    });
}

fn hide_window(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
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
    fn view(&self, rect: ces_capture::PhysicalRect) -> Option<RgbaImage> {
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
    use super::thumbnail_geometry;

    #[test]
    fn stacks_thumbnails_upward_in_logical_pixels_on_retina_displays() {
        assert_eq!(
            thumbnail_geometry(0, 0, 3_992, 2_048, 2.0, 1),
            (24.0, 824.0, 176.0)
        );
        assert_eq!(
            thumbnail_geometry(-3_840, 0, 3_840, 2_048, 2.0, 2),
            (-1_896.0, 656.0, 344.0)
        );
    }
}
