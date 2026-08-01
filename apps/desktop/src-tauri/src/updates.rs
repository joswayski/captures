use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, menu::MenuItem, window::Color,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

const UPDATE_EVENT: &str = "update-status-changed";
const RELEASES_URL: &str = "https://github.com/joswayski/captures/releases";
const UPDATE_NOTICE_WIDTH: f64 = 420.0;
const UPDATE_NOTICE_HEIGHT: f64 = 220.0;
const INITIAL_CHECK_DELAY: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    Idle {
        current_version: String,
        current_display_version: String,
    },
    Checking {
        current_version: String,
        current_display_version: String,
    },
    UpToDate {
        current_version: String,
        current_display_version: String,
    },
    Available {
        current_version: String,
        current_display_version: String,
        version: String,
        display_version: String,
        notes: Option<String>,
        installable: bool,
        manual_download_url: Option<String>,
    },
    Downloading {
        current_version: String,
        current_display_version: String,
        version: String,
        display_version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Error {
        current_version: String,
        current_display_version: String,
        message: String,
    },
}

pub struct UpdateCoordinator {
    status: Mutex<UpdateStatus>,
    pending: Mutex<Option<Update>>,
    menu_item: Mutex<Option<MenuItem<tauri::Wry>>>,
    notified_version: Mutex<Option<String>>,
    checking: AtomicBool,
    installing: AtomicBool,
}

impl Default for UpdateCoordinator {
    fn default() -> Self {
        Self {
            status: Mutex::new(UpdateStatus::Idle {
                current_version: String::new(),
                current_display_version: String::new(),
            }),
            pending: Mutex::new(None),
            menu_item: Mutex::new(None),
            notified_version: Mutex::new(None),
            checking: AtomicBool::new(false),
            installing: AtomicBool::new(false),
        }
    }
}

struct AtomicFlagGuard<'a>(&'a AtomicBool);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoticeDisposition {
    Ignore,
    Show,
    Defer,
}

impl<'a> AtomicFlagGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        (!flag.swap(true, Ordering::AcqRel)).then_some(Self(flag))
    }
}

impl Drop for AtomicFlagGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub fn initialize(app: &AppHandle) {
    let (current_version, current_display_version) = current_versions(app);
    set_status(
        app,
        UpdateStatus::Idle {
            current_version,
            current_display_version,
        },
    );

    if cfg!(debug_assertions) || !preview_release_build() {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(INITIAL_CHECK_DELAY).await;
        loop {
            if let Err(error) = check_for_updates_inner(&app, false).await {
                eprintln!("background update check failed: {error}");
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

pub fn register_menu_item(app: &AppHandle, item: MenuItem<tauri::Wry>) {
    *app.state::<UpdateCoordinator>().menu_item.lock() = Some(item);
    refresh_menu(app);
}

pub fn install_is_active(app: &AppHandle) -> bool {
    app.state::<UpdateCoordinator>()
        .installing
        .load(Ordering::Acquire)
}

pub fn defer_visible_notice(app: &AppHandle) {
    let visible = app
        .get_webview_window("update")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    if !visible {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while active_capture_or_recording(&app.state::<Arc<AppState>>()) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        show_update_notice(&app);
    });
}

pub fn handle_tray_action(app: &AppHandle) {
    let status = app.state::<UpdateCoordinator>().status.lock().clone();
    if matches!(
        status,
        UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. }
    ) {
        show_update_notice(app);
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match check_for_updates_inner(&app, true).await {
            Ok(UpdateStatus::UpToDate { .. }) => show_dialog(
                &app,
                "Captures is up to date",
                "You already have the newest available version.",
                MessageDialogKind::Info,
            ),
            Ok(UpdateStatus::Available { .. }) | Ok(UpdateStatus::Downloading { .. }) => {
                show_update_notice(&app);
            }
            Ok(_) => {}
            Err(error) => show_dialog(
                &app,
                "Could not check for updates",
                &error,
                MessageDialogKind::Error,
            ),
        }
    });
}

#[tauri::command]
pub fn get_update_status(state: tauri::State<'_, UpdateCoordinator>) -> UpdateStatus {
    state.status.lock().clone()
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<UpdateStatus, String> {
    check_for_updates_inner(&app, true).await
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let coordinator = app.state::<UpdateCoordinator>();
    let status = coordinator.status.lock().clone();
    let installable = matches!(
        status,
        UpdateStatus::Available {
            installable: true,
            ..
        } | UpdateStatus::Downloading { .. }
    );

    if !installable {
        if matches!(status, UpdateStatus::Available { .. }) {
            app.opener()
                .open_url(RELEASES_URL, None::<&str>)
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        return Err("there is no installable update available".to_owned());
    }

    let app_state = app.state::<Arc<AppState>>();
    if let Some(message) = install_restart_blocker(&app_state) {
        return Err(message.to_owned());
    }

    let Some(_install_guard) = AtomicFlagGuard::acquire(&coordinator.installing) else {
        return Err("an update is already being installed".to_owned());
    };
    let update = coordinator
        .pending
        .lock()
        .clone()
        .ok_or_else(|| "the available update needs to be checked again".to_owned())?;
    let version = update.version.clone();
    let display_version = display_version(&version);
    let (current_version, current_display_version) = current_versions(&app);
    set_status(
        &app,
        UpdateStatus::Downloading {
            current_version: current_version.clone(),
            current_display_version: current_display_version.clone(),
            version: version.clone(),
            display_version: display_version.clone(),
            downloaded: 0,
            total: None,
        },
    );

    let progress_app = app.clone();
    let progress_current_version = current_version.clone();
    let progress_current_display_version = current_display_version.clone();
    let progress_version = version.clone();
    let progress_display_version = display_version.clone();
    let mut downloaded = 0_u64;
    let result = update
        .download_and_install(
            move |chunk_length, total| {
                downloaded = downloaded.saturating_add(chunk_length as u64);
                set_status(
                    &progress_app,
                    UpdateStatus::Downloading {
                        current_version: progress_current_version.clone(),
                        current_display_version: progress_current_display_version.clone(),
                        version: progress_version.clone(),
                        display_version: progress_display_version.clone(),
                        downloaded,
                        total,
                    },
                );
            },
            || {},
        )
        .await;

    if let Err(error) = result {
        let message = format!("Could not install the update: {error}");
        set_status(
            &app,
            UpdateStatus::Error {
                current_version,
                current_display_version,
                message: message.clone(),
            },
        );
        return Err(message);
    }

    app.restart();
}

async fn check_for_updates_inner(app: &AppHandle, manual: bool) -> Result<UpdateStatus, String> {
    if !preview_release_build() {
        let message = "Update checks are available only in Captures Preview builds.".to_owned();
        let (current_version, current_display_version) = current_versions(app);
        if let Some(status) = check_error_status(
            manual,
            current_version,
            current_display_version,
            message.clone(),
        ) {
            set_status(app, status);
        }
        return Err(message);
    }

    let coordinator = app.state::<UpdateCoordinator>();
    if coordinator.installing.load(Ordering::Acquire) {
        return Ok(coordinator.status.lock().clone());
    }
    let Some(_check_guard) = AtomicFlagGuard::acquire(&coordinator.checking) else {
        return Ok(coordinator.status.lock().clone());
    };

    let (current_version, current_display_version) = current_versions(app);
    if manual {
        set_status(
            app,
            UpdateStatus::Checking {
                current_version: current_version.clone(),
                current_display_version: current_display_version.clone(),
            },
        );
    }

    let checked = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let update = match checked {
        Ok(update) => update,
        Err(error) => {
            let message = error.to_string();
            if let Some(status) = check_error_status(
                manual,
                current_version,
                current_display_version,
                message.clone(),
            ) {
                set_status(app, status);
            }
            return Err(message);
        }
    };

    let status = if let Some(update) = update {
        let version = update.version.clone();
        let display_version = display_version(&version);
        let notes = update.body.clone().filter(|notes| !notes.trim().is_empty());
        let installable = platform_update_is_installable();
        *coordinator.pending.lock() = Some(update);
        let status = UpdateStatus::Available {
            current_version,
            current_display_version,
            version: version.clone(),
            display_version,
            notes,
            installable,
            manual_download_url: (!installable).then(|| RELEASES_URL.to_owned()),
        };
        set_status(app, status.clone());
        schedule_update_notice(app, version);
        status
    } else {
        *coordinator.pending.lock() = None;
        *coordinator.notified_version.lock() = None;
        let status = UpdateStatus::UpToDate {
            current_version,
            current_display_version,
        };
        set_status(app, status.clone());
        status
    };

    Ok(status)
}

fn set_status(app: &AppHandle, status: UpdateStatus) {
    *app.state::<UpdateCoordinator>().status.lock() = status.clone();
    refresh_menu(app);
    if let Err(error) = app.emit(UPDATE_EVENT, status) {
        eprintln!("failed to emit update status: {error}");
    }
}

fn refresh_menu(app: &AppHandle) {
    let coordinator = app.state::<UpdateCoordinator>();
    let label = match &*coordinator.status.lock() {
        UpdateStatus::Available {
            display_version, ..
        } => format!("Update Available — {display_version}…"),
        UpdateStatus::Downloading { .. } => "Installing Update…".to_owned(),
        UpdateStatus::Checking { .. } => "Checking for Updates…".to_owned(),
        _ => "Check for Updates…".to_owned(),
    };
    if let Some(item) = coordinator.menu_item.lock().as_ref()
        && let Err(error) = item.set_text(label)
    {
        eprintln!("failed to update tray update item: {error}");
    }
}

fn schedule_update_notice(app: &AppHandle, version: String) {
    let coordinator = app.state::<UpdateCoordinator>();
    {
        let mut notified_version = coordinator.notified_version.lock();
        if notified_version.as_deref() == Some(&version) {
            return;
        }
        *notified_version = Some(version.clone());
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while active_capture_or_recording(&app.state::<Arc<AppState>>()) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let still_available = matches!(
            &*app.state::<UpdateCoordinator>().status.lock(),
            UpdateStatus::Available {
                version: available,
                ..
            } if available == &version
        );
        if still_available {
            show_update_notice(&app);
        }
    });
}

fn show_update_notice(app: &AppHandle) {
    let disposition = notice_disposition(
        &app.state::<UpdateCoordinator>().status.lock(),
        active_capture_or_recording(&app.state::<Arc<AppState>>()),
    );
    match disposition {
        NoticeDisposition::Ignore => return,
        NoticeDisposition::Defer => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                while active_capture_or_recording(&app.state::<Arc<AppState>>()) {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                show_update_notice(&app);
            });
            return;
        }
        NoticeDisposition::Show => {}
    }

    let app = app.clone();
    let dispatch = app.clone();
    if let Err(error) = dispatch.run_on_main_thread(move || {
        if let Some(window) = app.get_webview_window("update") {
            let _ = window.show();
            return;
        }
        if let Err(error) = create_update_notice(&app) {
            eprintln!("failed to show update notice: {error}");
        }
    }) {
        eprintln!("failed to schedule update notice: {error}");
    }
}

fn create_update_notice(app: &AppHandle) -> Result<(), tauri::Error> {
    let (x, y) = update_notice_position(app);
    let window = WebviewWindowBuilder::new(
        app,
        "update",
        WebviewUrl::App("index.html?view=update".into()),
    )
    .title("Captures Update")
    .inner_size(UPDATE_NOTICE_WIDTH, UPDATE_NOTICE_HEIGHT)
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
    let _ = window.set_content_protected(true);
    window.show()?;
    Ok(())
}

fn update_notice_position(app: &AppHandle) -> (f64, f64) {
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
            (right - UPDATE_NOTICE_WIDTH - 18.0, top + 30.0)
        })
        .unwrap_or((20.0, 30.0))
}

fn show_dialog(app: &AppHandle, title: &str, message: &str, kind: MessageDialogKind) {
    app.dialog()
        .message(message)
        .title(title)
        .buttons(MessageDialogButtons::Ok)
        .kind(kind)
        .show(|_| {});
}

fn current_versions(app: &AppHandle) -> (String, String) {
    let current_version = app.package_info().version.to_string();
    let current_display_version = display_version(&current_version);
    (current_version, current_display_version)
}

fn preview_release_build() -> bool {
    release_channel_enabled(option_env!("CAPTURES_RELEASE_CHANNEL"))
}

fn release_channel_enabled(value: Option<&str>) -> bool {
    value == Some("preview")
}

fn check_error_status(
    manual: bool,
    current_version: String,
    current_display_version: String,
    message: String,
) -> Option<UpdateStatus> {
    manual.then_some(UpdateStatus::Error {
        current_version,
        current_display_version,
        message,
    })
}

fn notice_disposition(status: &UpdateStatus, capture_active: bool) -> NoticeDisposition {
    if !matches!(
        status,
        UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. }
    ) {
        NoticeDisposition::Ignore
    } else if capture_active {
        NoticeDisposition::Defer
    } else {
        NoticeDisposition::Show
    }
}

fn display_version(version: &str) -> String {
    let normalized = version.trim_start_matches('v');
    let core = normalized
        .split_once('-')
        .map_or(normalized, |(core, _)| core);
    let mut parts = core.split('.');
    let (Some(year), Some(month), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return normalized.to_owned();
    };
    let (Ok(year), Ok(month), Ok(patch)) = (
        year.parse::<u32>(),
        month.parse::<u32>(),
        patch.parse::<u32>(),
    ) else {
        return normalized.to_owned();
    };
    let day = patch / 100;
    let revision = patch % 100;
    if !(2000..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(1..=99).contains(&revision)
    {
        return normalized.to_owned();
    }
    format!("{year:04}.{month:02}.{day:02}.{revision}")
}

fn platform_update_is_installable() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("APPIMAGE").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn capture_is_active(state: &AppState) -> bool {
    state.thumbnail_visibility.lock().is_suppressed()
        || !state.sessions.lock().is_empty()
        || state.recording_selection.lock().is_some()
}

fn active_capture_or_recording(state: &AppState) -> bool {
    capture_is_active(state) || crate::recording::recording_session_is_active(state)
}

fn install_restart_blocker(state: &AppState) -> Option<&'static str> {
    let capture_active = capture_is_active(state);
    let recording_active = crate::recording::recording_session_is_active(state);
    let has_unsaved_capture = state
        .artifacts
        .lock()
        .iter()
        .any(|artifact| artifact.path.is_none());
    restart_blocker(capture_active, recording_active, has_unsaved_capture)
}

fn restart_blocker(
    capture_active: bool,
    recording_active: bool,
    has_unsaved_capture: bool,
) -> Option<&'static str> {
    if recording_active {
        Some("Finish or cancel the active recording before installing the update.")
    } else if capture_active {
        Some("Finish or cancel the active capture before installing the update.")
    } else if has_unsaved_capture {
        Some("Save or close every unsaved capture before installing the update.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::{
        AtomicFlagGuard, NoticeDisposition, UpdateStatus, check_error_status, display_version,
        notice_disposition, release_channel_enabled, restart_blocker,
    };

    #[test]
    fn enables_automatic_updates_only_for_preview_builds() {
        assert!(release_channel_enabled(Some("preview")));
        assert!(!release_channel_enabled(None));
        assert!(!release_channel_enabled(Some("0")));
        assert!(!release_channel_enabled(Some("stable")));
    }

    #[test]
    fn formats_encoded_calver_for_people() {
        assert_eq!(display_version("2026.7.1901"), "2026.07.19.1");
        assert_eq!(display_version("2026.12.3109"), "2026.12.31.9");
    }

    #[test]
    fn preserves_development_and_invalid_versions() {
        assert_eq!(display_version("0.1.0"), "0.1.0");
        assert_eq!(display_version("2026.13.1901"), "2026.13.1901");
        assert_eq!(display_version("2026.7.1900"), "2026.7.1900");
    }

    #[test]
    fn blocks_restart_for_active_work_or_unsaved_captures() {
        assert_eq!(
            restart_blocker(true, false, false),
            Some("Finish or cancel the active capture before installing the update.")
        );
        assert_eq!(
            restart_blocker(false, true, false),
            Some("Finish or cancel the active recording before installing the update.")
        );
        assert_eq!(
            restart_blocker(false, false, true),
            Some("Save or close every unsaved capture before installing the update.")
        );
        assert_eq!(restart_blocker(false, false, false), None);
    }

    #[test]
    fn suppresses_duplicate_checks_until_the_first_finishes() {
        let checking = AtomicBool::new(false);
        let guard = AtomicFlagGuard::acquire(&checking).expect("first check should start");
        assert!(AtomicFlagGuard::acquire(&checking).is_none());
        drop(guard);
        assert!(AtomicFlagGuard::acquire(&checking).is_some());
    }

    #[test]
    fn only_manual_check_errors_change_visible_status() {
        assert!(
            check_error_status(false, "1.0.0".into(), "1.0.0".into(), "offline".into()).is_none()
        );
        assert!(matches!(
            check_error_status(true, "1.0.0".into(), "1.0.0".into(), "offline".into()),
            Some(UpdateStatus::Error { message, .. }) if message == "offline"
        ));
    }

    #[test]
    fn defers_available_update_notices_until_capture_finishes() {
        let status = UpdateStatus::Available {
            current_version: "2026.7.1901".into(),
            current_display_version: "2026.07.19.1".into(),
            version: "2026.7.1902".into(),
            display_version: "2026.07.19.2".into(),
            notes: None,
            installable: true,
            manual_download_url: None,
        };
        assert_eq!(notice_disposition(&status, true), NoticeDisposition::Defer);
        assert_eq!(notice_disposition(&status, false), NoticeDisposition::Show);
        assert_eq!(
            notice_disposition(
                &UpdateStatus::UpToDate {
                    current_version: "2026.7.1901".into(),
                    current_display_version: "2026.07.19.1".into(),
                },
                false,
            ),
            NoticeDisposition::Ignore
        );
    }
}
