use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

const UPDATE_EVENT: &str = "update-status-changed";
const RELEASES_URL: &str = "https://github.com/joswayski/captures/releases";
const DOWNLOAD_PAGE_URL: &str = "https://captur.es/#download";
const UPDATE_NOTICE_WIDTH: f64 = 440.0;
const UPDATE_NOTICE_MIN_HEIGHT: f64 = 290.0;
const UPDATE_NOTICE_MAX_HEIGHT: f64 = 480.0;
const UPDATE_NOTICE_STACK_HEIGHT: f64 = 72.0;
const UPDATE_NOTICE_WARNING_HEIGHT: f64 = 56.0;
const UPDATE_NOTICE_ERROR_FALLBACK_HEIGHT: f64 = 40.0;
const RESTART_COUNTDOWN_SECONDS: u8 = 3;
const RESTART_FADE_DURATION: Duration = Duration::from_millis(400);
const EDITOR_CLOSE_FLUSH: Duration = Duration::from_millis(500);
const DEFER_CAPTURE_START_TIMEOUT: Duration = Duration::from_millis(1_500);
const INITIAL_CHECK_DELAY: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);
const DOWNLOAD_ATTEMPTS: u32 = 3;
const DOWNLOAD_RETRY_BASE_DELAY: Duration = Duration::from_millis(400);

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
        changelog: Vec<UpdateChangelogEntry>,
        installable: bool,
        manual_download_url: Option<String>,
        download_size: Option<u64>,
        will_close_open_captures: bool,
    },
    Downloading {
        current_version: String,
        current_display_version: String,
        version: String,
        display_version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Restarting {
        current_version: String,
        current_display_version: String,
        version: String,
        display_version: String,
        seconds_remaining: u8,
    },
    Error {
        current_version: String,
        current_display_version: String,
        message: String,
        retry_install: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UpdateChangelogEntry {
    pub version: String,
    pub display_version: String,
    pub notes: Option<String>,
}

pub struct UpdateCoordinator {
    status: Mutex<UpdateStatus>,
    pending: Mutex<Option<Update>>,
    notified_version: Mutex<Option<String>>,
    checking: AtomicBool,
    installing: AtomicBool,
    restoring: AtomicBool,
    deferred: AtomicBool,
}

impl Default for UpdateCoordinator {
    fn default() -> Self {
        Self {
            status: Mutex::new(UpdateStatus::Idle {
                current_version: String::new(),
                current_display_version: String::new(),
            }),
            pending: Mutex::new(None),
            notified_version: Mutex::new(None),
            checking: AtomicBool::new(false),
            installing: AtomicBool::new(false),
            restoring: AtomicBool::new(false),
            deferred: AtomicBool::new(false),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoticeRestorePlan {
    Ignore,
    Wait,
    Show,
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

pub fn install_is_active(app: &AppHandle) -> bool {
    app.state::<UpdateCoordinator>()
        .installing
        .load(Ordering::Acquire)
}

pub fn defer_visible_notice(app: &AppHandle) {
    begin_deferred_restore(app);
}

pub fn should_hide_update_notice_for_capture(app: &AppHandle) -> bool {
    should_hide_update_notice_status(&app.state::<UpdateCoordinator>().status.lock())
}

pub fn restore_update_notice(app: &AppHandle) {
    let restorable = notice_can_restore(app);
    let visible = update_notice_is_visible(app);
    let coordinator = app.state::<UpdateCoordinator>();
    let deferred = coordinator.deferred.load(Ordering::Acquire);
    let capture_active = active_capture_or_recording(&app.state::<Arc<AppState>>());
    match notice_restore_plan(restorable, deferred, visible, capture_active) {
        NoticeRestorePlan::Ignore => {}
        NoticeRestorePlan::Wait => begin_deferred_restore(app),
        NoticeRestorePlan::Show => {
            coordinator.deferred.store(false, Ordering::Release);
            show_update_notice(app);
        }
    }
}

/// Sync dynamic status and geometry without showing, raising, or focusing the notice.
pub fn refresh_update_notice(app: &AppHandle) {
    let restorable = notice_can_restore(app);
    let visible = update_notice_is_visible(app);
    if !should_refresh_update_notice(restorable, visible) {
        return;
    }

    let status = annotate_status(app, app.state::<UpdateCoordinator>().status.lock().clone());
    let card_height = update_notice_height(&status);
    let app = app.clone();
    let dispatch = app.clone();
    if let Err(error) = dispatch.run_on_main_thread(move || {
        let Some(window) = app.get_webview_window("update") else {
            return;
        };
        if !window.is_visible().unwrap_or(false) {
            return;
        }
        let placement =
            crate::tray_anchored_notice_placement(&app, UPDATE_NOTICE_WIDTH, card_height);
        sync_update_notice_window(&window, &status, placement);
    }) {
        eprintln!("failed to schedule update notice refresh: {error}");
    }
}

fn begin_deferred_restore(app: &AppHandle) {
    let restorable = notice_can_restore(app);
    let visible = update_notice_is_visible(app);
    let coordinator = app.state::<UpdateCoordinator>();
    let already_deferred = coordinator.deferred.load(Ordering::Acquire);
    if !should_begin_deferred_restore(restorable, visible, already_deferred) {
        return;
    }
    coordinator.deferred.store(true, Ordering::Release);
    if coordinator.restoring.swap(true, Ordering::AcqRel) {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        wait_for_capture_cycle(&app).await;
        app.state::<UpdateCoordinator>()
            .restoring
            .store(false, Ordering::Release);
        restore_update_notice(&app);
    });
}

fn notice_can_restore(app: &AppHandle) -> bool {
    if app
        .state::<UpdateCoordinator>()
        .installing
        .load(Ordering::Acquire)
    {
        return false;
    }
    let restorable_status = matches!(
        *app.state::<UpdateCoordinator>().status.lock(),
        UpdateStatus::Available { .. }
            | UpdateStatus::Downloading { .. }
            | UpdateStatus::Error { .. }
    );
    restorable_status && app.get_webview_window("update").is_some()
}

fn update_notice_is_visible(app: &AppHandle) -> bool {
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        captures_macos_window::run_on_main(move || update_notice_is_visible_inner(&app))
            .unwrap_or(true)
    }
    #[cfg(not(target_os = "macos"))]
    update_notice_is_visible_inner(app)
}

fn update_notice_is_visible_inner(app: &AppHandle) -> bool {
    app.get_webview_window("update")
        .is_some_and(|window| window.is_visible().unwrap_or(true))
}

fn should_begin_deferred_restore(restorable: bool, visible: bool, already_deferred: bool) -> bool {
    restorable && (visible || already_deferred)
}

fn should_refresh_update_notice(restorable: bool, visible: bool) -> bool {
    restorable && visible
}

fn notice_restore_plan(
    restorable: bool,
    deferred: bool,
    visible: bool,
    capture_active: bool,
) -> NoticeRestorePlan {
    if !restorable || (!deferred && !visible) {
        NoticeRestorePlan::Ignore
    } else if capture_active {
        NoticeRestorePlan::Wait
    } else {
        NoticeRestorePlan::Show
    }
}

async fn wait_for_capture_cycle(app: &AppHandle) {
    let started = Instant::now();
    while !active_capture_or_recording(&app.state::<Arc<AppState>>())
        && should_wait_for_capture_start(started.elapsed(), DEFER_CAPTURE_START_TIMEOUT)
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    while active_capture_or_recording(&app.state::<Arc<AppState>>()) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn should_wait_for_capture_start(elapsed: Duration, timeout: Duration) -> bool {
    elapsed < timeout
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
pub fn get_update_status(app: AppHandle) -> UpdateStatus {
    let status = app.state::<UpdateCoordinator>().status.lock().clone();
    annotate_status(&app, status)
}

/// Hides the update notice from Rust. The webview cannot call `Window.hide`
/// because the update window is not granted `core:window:allow-hide`.
#[tauri::command]
pub fn dismiss_update_notice(app: AppHandle) {
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window("update")
        && let Err(error) = captures_macos_window::dismiss_update_notice(&window)
    {
        eprintln!("failed to dismiss update notice: {error}");
    }
    #[cfg(not(target_os = "macos"))]
    crate::hide_window(&app, "update");
}

/// Opens the public website download section. The update window has no opener
/// permission, so failed in-app updates use this command instead of an `<a>`.
#[tauri::command]
pub fn open_update_download_page(app: AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(DOWNLOAD_PAGE_URL, None::<&str>)
        .map_err(|error| error.to_string())
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
    prepare_open_captures_for_update(&app, &app_state).await;
    if let Some(message) = install_restart_blocker(&app_state) {
        return Err(message.to_owned());
    }
    let update = coordinator
        .pending
        .lock()
        .clone()
        .ok_or_else(|| "the available update needs to be checked again".to_owned())?;
    let expected_download_size =
        manifest_download_size(&update.raw_json, update.download_url.as_str());
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
            total: expected_download_size,
        },
    );

    let progress_app = app.clone();
    let progress_current_version = current_version.clone();
    let progress_current_display_version = current_display_version.clone();
    let progress_version = version.clone();
    let progress_display_version = display_version.clone();
    let mut result = Err("the update download did not run".to_owned());
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        let mut downloaded = 0_u64;
        let progress_app = progress_app.clone();
        let progress_current_version = progress_current_version.clone();
        let progress_current_display_version = progress_current_display_version.clone();
        let progress_version = progress_version.clone();
        let progress_display_version = progress_display_version.clone();
        let attempt_result = update
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
                            total: total.or(expected_download_size),
                        },
                    );
                },
                || {},
            )
            .await;
        match attempt_result {
            Ok(()) => {
                result = Ok(());
                break;
            }
            Err(error) => {
                let message = error.to_string();
                if attempt == DOWNLOAD_ATTEMPTS || !download_error_is_retryable(&message) {
                    result = Err(message);
                    break;
                }
                eprintln!(
                    "update download failed ({message}); retrying ({attempt}/{DOWNLOAD_ATTEMPTS})"
                );
                tokio::time::sleep(download_retry_delay(attempt)).await;
            }
        }
    }

    if let Err(error) = result {
        let message = install_error_message(&error);
        set_status(
            &app,
            UpdateStatus::Error {
                current_version,
                current_display_version,
                message: message.clone(),
                retry_install: true,
            },
        );
        return Err(message);
    }

    for seconds_remaining in (1..=RESTART_COUNTDOWN_SECONDS).rev() {
        set_status(
            &app,
            UpdateStatus::Restarting {
                current_version: current_version.clone(),
                current_display_version: current_display_version.clone(),
                version: version.clone(),
                display_version: display_version.clone(),
                seconds_remaining,
            },
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    tokio::time::sleep(RESTART_FADE_DURATION).await;
    if let Err(error) = mark_update_restart_pending() {
        eprintln!("failed to remember update restart: {error}");
    }
    crate::crash_report::mark_clean_exit();
    app.restart();
}

pub fn take_update_restart_pending() -> bool {
    match take_restart_marker(&restart_marker_path()) {
        Ok(pending) => pending,
        Err(error) => {
            eprintln!("failed to clear update restart marker: {error}");
            false
        }
    }
}

fn mark_update_restart_pending() -> std::io::Result<()> {
    let path = restart_marker_path();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, [])
}

fn take_restart_marker(path: &Path) -> std::io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn restart_marker_path() -> std::path::PathBuf {
    crate::models::settings_path().with_file_name("update-restart-pending")
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
        let changelog = stacked_changelog(&update.raw_json, &current_version, &version);
        let download_size = manifest_download_size(&update.raw_json, update.download_url.as_str());
        let installable = platform_update_is_installable();
        *coordinator.pending.lock() = Some(update);
        let status = UpdateStatus::Available {
            current_version,
            current_display_version,
            version: version.clone(),
            display_version,
            notes,
            changelog,
            installable,
            manual_download_url: (!installable).then(|| RELEASES_URL.to_owned()),
            download_size,
            will_close_open_captures: false,
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
    crate::refresh_tray_menu(app);
    apply_update_notice_capture_policy(app);
    if let Err(error) = app.emit(UPDATE_EVENT, annotate_status(app, status)) {
        eprintln!("failed to emit update status: {error}");
    }
}

fn apply_update_notice_capture_policy(app: &AppHandle) {
    let protected = should_hide_update_notice_for_capture(app);
    let app = app.clone();
    let dispatch = app.clone();
    if let Err(error) = dispatch.run_on_main_thread(move || {
        if let Some(window) = app.get_webview_window("update")
            && let Err(error) = crate::set_window_content_protected(&window, protected)
        {
            eprintln!("failed to update notice capture protection: {error}");
        }
    }) {
        eprintln!("failed to schedule update notice capture protection: {error}");
    }
}

pub(crate) fn current_status(app: &AppHandle) -> UpdateStatus {
    app.state::<UpdateCoordinator>().status.lock().clone()
}

pub(crate) struct TrayUpdateItem {
    pub label: String,
    pub enabled: bool,
    pub pin_first: bool,
}

pub(crate) fn tray_update_item(status: &UpdateStatus) -> TrayUpdateItem {
    match status {
        UpdateStatus::Available {
            changelog, notes, ..
        } => TrayUpdateItem {
            label: update_available_menu_label(changelog, notes.as_deref()),
            enabled: true,
            pin_first: true,
        },
        UpdateStatus::Downloading { .. } => TrayUpdateItem {
            label: "Installing Update…".to_owned(),
            enabled: false,
            pin_first: true,
        },
        UpdateStatus::Restarting { .. } => TrayUpdateItem {
            label: "Restarting Captures…".to_owned(),
            enabled: false,
            pin_first: true,
        },
        UpdateStatus::Checking { .. } => TrayUpdateItem {
            label: "Checking for Updates…".to_owned(),
            enabled: false,
            pin_first: false,
        },
        _ => TrayUpdateItem {
            label: "Check for Updates…".to_owned(),
            enabled: true,
            pin_first: false,
        },
    }
}

pub(crate) fn tray_tooltip(status: &UpdateStatus) -> &'static str {
    match status {
        UpdateStatus::Available { .. } => "Captures — Update available",
        UpdateStatus::Downloading { .. } | UpdateStatus::Restarting { .. } => {
            "Captures — Installing update"
        }
        _ => "Captures",
    }
}

fn update_available_menu_label(changelog: &[UpdateChangelogEntry], notes: Option<&str>) -> String {
    match changelog_change_count(changelog, notes) {
        0 => "Update Available".to_owned(),
        1 => "Update Available — 1 change".to_owned(),
        count => format!("Update Available — {count} changes"),
    }
}

fn changelog_change_count(changelog: &[UpdateChangelogEntry], notes: Option<&str>) -> usize {
    let from_changelog: usize = changelog
        .iter()
        .map(|entry| {
            entry
                .notes
                .as_deref()
                .map(release_note_item_count)
                .unwrap_or(0)
        })
        .sum();
    if from_changelog > 0 {
        from_changelog
    } else {
        notes.map(release_note_item_count).unwrap_or(0)
    }
}

fn release_note_item_count(markdown: &str) -> usize {
    let mut skipping_alert = false;
    let mut count = 0usize;
    for source_line in markdown.lines() {
        let line = source_line.trim();
        if line.starts_with("> [!") || (line.starts_with('>') && line.contains("[!")) {
            skipping_alert = true;
            continue;
        }
        if skipping_alert && line.starts_with('>') {
            continue;
        }
        if line.is_empty() {
            skipping_alert = false;
            continue;
        }
        skipping_alert = false;
        if line.starts_with('#') || line.to_ascii_lowercase().contains("full changelog") {
            continue;
        }
        if line.starts_with("* ") || line.starts_with("- ") || line.starts_with("+ ") {
            if line
                .to_ascii_lowercase()
                .contains("made their first contribution")
            {
                continue;
            }
            count += 1;
        }
    }
    count
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
    let status = annotate_status(app, app.state::<UpdateCoordinator>().status.lock().clone());
    let disposition = notice_disposition(
        &status,
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

    let card_height = update_notice_height(&status);
    let app = app.clone();
    let dispatch = app.clone();
    if let Err(error) = dispatch.run_on_main_thread(move || {
        let placement =
            crate::tray_anchored_notice_placement(&app, UPDATE_NOTICE_WIDTH, card_height);
        if let Some(window) = app.get_webview_window("update") {
            if should_refresh_notice_activation_source(window.is_focused().unwrap_or(false)) {
                remember_notice_activation_source();
            }
            sync_update_notice_window(&window, &status, placement);
            let _ = window.show();
            let _ = crate::set_window_content_protected(
                &window,
                should_hide_update_notice_for_capture(&app),
            );
            let _ = crate::apply_tray_notice_position(&window, placement);
            crate::focus_single_window(&window);
            return;
        }
        if let Err(error) = create_update_notice(&app, placement) {
            eprintln!("failed to show update notice: {error}");
        }
    }) {
        eprintln!("failed to schedule update notice: {error}");
    }
}

fn sync_update_notice_window(
    window: &tauri::WebviewWindow,
    status: &UpdateStatus,
    placement: crate::StartupNoticePlacement,
) {
    if let Err(error) = window.emit(UPDATE_EVENT, status.clone()) {
        eprintln!("failed to refresh update notice status: {error}");
    }
    let _ = crate::apply_tray_notice_position(window, placement);
    if let Some(caret) = crate::notice_caret_payload(&placement)
        && let Err(error) = window.emit(crate::NOTICE_CARET_EVENT, caret)
    {
        eprintln!("failed to update the update notice caret: {error}");
    }
}

fn create_update_notice(
    app: &AppHandle,
    placement: crate::StartupNoticePlacement,
) -> Result<(), tauri::Error> {
    remember_notice_activation_source();
    let (theme, background) = crate::notice_window_chrome(app);
    let window = WebviewWindowBuilder::new(
        app,
        "update",
        WebviewUrl::App(crate::tray_notice_url("update", placement).into()),
    )
    .title("Captures Update")
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
    .accept_first_mouse(true)
    .focused(false)
    .visible(false)
    .on_page_load(crate::document_window_page_load_handler(
        "failed to reveal the update notice",
    ))
    .build()?;
    crate::apply_tray_notice_position(&window, placement)?;
    window.show()?;
    let _ =
        crate::set_window_content_protected(&window, should_hide_update_notice_for_capture(app));
    crate::apply_tray_notice_position(&window, placement)?;
    crate::focus_single_window(&window);
    Ok(())
}

fn should_refresh_notice_activation_source(is_focused: bool) -> bool {
    !is_focused
}

fn remember_notice_activation_source() {
    #[cfg(target_os = "macos")]
    captures_macos_window::remember_frontmost_app_before_update_notice_activation();
}

fn update_notice_height(status: &UpdateStatus) -> f64 {
    let extra_versions = match status {
        UpdateStatus::Available { changelog, .. } => changelog.len().saturating_sub(1),
        _ => 0,
    };
    let warning = match status {
        UpdateStatus::Available {
            will_close_open_captures: true,
            ..
        } => UPDATE_NOTICE_WARNING_HEIGHT,
        _ => 0.0,
    };
    let error_fallback = match status {
        UpdateStatus::Error { .. } => UPDATE_NOTICE_ERROR_FALLBACK_HEIGHT,
        _ => 0.0,
    };
    (UPDATE_NOTICE_MIN_HEIGHT
        + extra_versions as f64 * UPDATE_NOTICE_STACK_HEIGHT
        + warning
        + error_fallback)
        .min(UPDATE_NOTICE_MAX_HEIGHT)
}

fn show_dialog(app: &AppHandle, title: &str, message: &str, kind: MessageDialogKind) {
    let app = app.clone();
    let dispatch = app.clone();
    let title = title.to_owned();
    let message = message.to_owned();
    if let Err(error) = dispatch.run_on_main_thread(move || {
        app.dialog()
            .message(message)
            .title(title)
            .buttons(MessageDialogButtons::Ok)
            .kind(kind)
            .show(|_| {});
    }) {
        eprintln!("failed to show update dialog: {error}");
    }
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
        retry_install: false,
    })
}

fn should_hide_update_notice_status(status: &UpdateStatus) -> bool {
    !matches!(status, UpdateStatus::Error { .. })
}

fn download_error_is_retryable(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("403")
        || normalized.contains("429")
        || normalized.contains("forbidden")
        || normalized.contains("too many requests")
}

fn download_retry_delay(attempt: u32) -> Duration {
    DOWNLOAD_RETRY_BASE_DELAY.saturating_mul(2u32.pow(attempt.saturating_sub(1)))
}

fn install_error_message(error: &str) -> String {
    if download_error_is_retryable(error) {
        format!(
            "Could not install the update: GitHub temporarily refused the download ({error}). This is often a short rate limit; wait a moment and try again."
        )
    } else {
        format!("Could not install the update: {error}")
    }
}

fn notice_disposition(status: &UpdateStatus, capture_active: bool) -> NoticeDisposition {
    if matches!(status, UpdateStatus::Error { .. }) {
        NoticeDisposition::Show
    } else if !matches!(
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

fn stacked_changelog(
    raw_json: &serde_json::Value,
    current_version: &str,
    latest_version: &str,
) -> Vec<UpdateChangelogEntry> {
    let Some(entries) = raw_json.get("changelog").and_then(|value| value.as_array()) else {
        return Vec::new();
    };

    let mut changelog = entries
        .iter()
        .filter_map(parse_changelog_entry)
        .filter(|entry| version_is_newer_than(entry.version.as_str(), current_version))
        .filter(|entry| !version_is_newer_than(entry.version.as_str(), latest_version))
        .collect::<Vec<_>>();
    changelog.sort_by(|left, right| {
        parse_version_tuple(&right.version).cmp(&parse_version_tuple(&left.version))
    });
    changelog
}

fn parse_changelog_entry(value: &serde_json::Value) -> Option<UpdateChangelogEntry> {
    #[derive(Deserialize)]
    struct RawChangelogEntry {
        version: String,
        #[serde(default)]
        display_version: Option<String>,
        #[serde(default)]
        notes: Option<String>,
    }

    let raw = serde_json::from_value::<RawChangelogEntry>(value.clone()).ok()?;
    if raw.version.trim().is_empty() {
        return None;
    }
    let notes = raw.notes.filter(|notes| !notes.trim().is_empty());
    Some(UpdateChangelogEntry {
        display_version: raw
            .display_version
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| display_version(&raw.version)),
        version: raw.version,
        notes,
    })
}

fn parse_version_tuple(version: &str) -> Option<(u64, u64, u64)> {
    let normalized = version.trim_start_matches('v');
    let core = normalized
        .split_once('-')
        .map_or(normalized, |(core, _)| core);
    let mut parts = core.split('.');
    let (Some(major), Some(minor), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    Some((
        major.parse().ok()?,
        minor.parse().ok()?,
        patch.parse().ok()?,
    ))
}

fn version_is_newer_than(candidate: &str, baseline: &str) -> bool {
    match (
        parse_version_tuple(candidate),
        parse_version_tuple(baseline),
    ) {
        (Some(candidate), Some(baseline)) => candidate > baseline,
        _ => false,
    }
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

fn manifest_download_size(raw_json: &serde_json::Value, download_url: &str) -> Option<u64> {
    let size = raw_json
        .get("platforms")
        .and_then(serde_json::Value::as_object)
        .and_then(|platforms| {
            platforms.values().find(|entry| {
                entry.get("url").and_then(serde_json::Value::as_str) == Some(download_url)
            })
        })
        .and_then(|entry| entry.get("size"))
        .or_else(|| raw_json.get("size"))
        .and_then(serde_json::Value::as_u64);
    size.filter(|size| *size > 0)
}

fn capture_is_active(state: &AppState) -> bool {
    state.thumbnail_visibility.lock().is_suppressed()
        || !state.sessions.lock().is_empty()
        || state.recording_selection.lock().is_some()
}

fn active_capture_or_recording(state: &AppState) -> bool {
    capture_is_active(state) || crate::recording::recording_session_is_active(state)
}

fn annotate_status(app: &AppHandle, mut status: UpdateStatus) -> UpdateStatus {
    if let UpdateStatus::Available {
        will_close_open_captures,
        ..
    } = &mut status
    {
        *will_close_open_captures = open_captures_will_close(app, &app.state::<Arc<AppState>>());
    }
    if let UpdateStatus::Error { retry_install, .. } = &mut status {
        *retry_install = app.state::<UpdateCoordinator>().pending.lock().is_some();
    }
    status
}

fn open_captures_will_close(app: &AppHandle, state: &AppState) -> bool {
    let has_unsaved_capture = state
        .artifacts
        .lock()
        .iter()
        .any(|artifact| artifact.path.is_none());
    open_captures_will_close_from(
        capture_is_active(state) || crate::screenshot_countdown_is_active(state),
        unsaved_mini_previews_are_open(has_unsaved_capture, state.settings().show_mini_previews),
        app.webview_windows()
            .keys()
            .any(|label| capture_window_should_close_for_update(label)),
    )
}

fn unsaved_mini_previews_are_open(has_unsaved_capture: bool, show_mini_previews: bool) -> bool {
    has_unsaved_capture && show_mini_previews
}

fn open_captures_will_close_from(
    capture_ui_active: bool,
    has_unsaved_capture: bool,
    has_open_editor_or_viewer: bool,
) -> bool {
    capture_ui_active || has_unsaved_capture || has_open_editor_or_viewer
}

fn capture_window_should_close_for_update(label: &str) -> bool {
    label == "viewer"
        || label.starts_with(crate::VIEWER_WINDOW_PREFIX)
        || label.starts_with(crate::screenshot_editor::SCREENSHOT_EDITOR_WINDOW_PREFIX)
        || label.starts_with(crate::RECORDING_EDITOR_WINDOW_PREFIX)
}

fn capture_windows_are_open(app: &AppHandle) -> bool {
    app.webview_windows()
        .keys()
        .any(|label| capture_window_should_close_for_update(label))
}

fn close_open_capture_windows(app: &AppHandle) {
    let app = app.clone();
    if let Err(error) = app.clone().run_on_main_thread(move || {
        let labels: Vec<String> = app
            .webview_windows()
            .into_keys()
            .filter(|label| capture_window_should_close_for_update(label))
            .collect();
        for label in labels {
            let Some(window) = app.get_webview_window(&label) else {
                continue;
            };
            if let Err(error) = window.close() {
                eprintln!("failed to close {label} before update: {error}");
                if let Err(error) = window.destroy() {
                    eprintln!("failed to destroy {label} before update: {error}");
                }
            }
        }
    }) {
        eprintln!("failed to close capture windows before update: {error}");
    }
}

async fn prepare_open_captures_for_update(app: &AppHandle, state: &Arc<AppState>) {
    let should_wait_for_editors = capture_windows_are_open(app);
    close_open_capture_windows(app);
    crate::dismiss_capture_ui_for_update(app, state);
    if should_wait_for_editors {
        tokio::time::sleep(EDITOR_CLOSE_FLUSH).await;
    }
}

fn install_restart_blocker(state: &AppState) -> Option<&'static str> {
    restart_blocker(crate::recording::recording_in_progress(state))
}

fn restart_blocker(recording_in_progress: bool) -> Option<&'static str> {
    recording_in_progress
        .then_some("Finish or cancel the active recording before installing the update.")
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::AtomicBool, time::Duration};

    use super::{
        AtomicFlagGuard, CHECK_INTERVAL, DOWNLOAD_ATTEMPTS, DOWNLOAD_PAGE_URL, NoticeDisposition,
        NoticeRestorePlan, RELEASES_URL, UpdateChangelogEntry, UpdateStatus,
        capture_window_should_close_for_update, check_error_status, display_version,
        download_error_is_retryable, download_retry_delay, install_error_message,
        manifest_download_size, notice_disposition, notice_restore_plan,
        open_captures_will_close_from, release_channel_enabled, restart_blocker,
        should_begin_deferred_restore, should_hide_update_notice_status,
        should_refresh_notice_activation_source, should_refresh_update_notice,
        should_wait_for_capture_start, stacked_changelog, take_restart_marker, tray_update_item,
        unsaved_mini_previews_are_open, update_available_menu_label, update_notice_height,
        version_is_newer_than,
    };

    #[test]
    fn checks_for_preview_updates_every_five_minutes() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(5 * 60));
    }

    #[test]
    fn prefers_the_captur_es_manifest_and_falls_back_to_github() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(
            config["plugins"]["updater"]["endpoints"],
            serde_json::json!([
                "https://captur.es/api/updates/preview",
                "https://github.com/joswayski/captures/releases/download/preview/latest.json"
            ])
        );
    }

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
    fn reads_the_selected_platform_size_from_updater_metadata() {
        let manifest = serde_json::json!({
            "version": "2026.8.3001",
            "platforms": {
                "darwin-aarch64": {
                    "url": "https://example.com/Captures.app.tar.gz",
                    "signature": "mac",
                    "size": 42_000_000
                },
                "windows-x86_64": {
                    "url": "https://example.com/Captures-setup.exe",
                    "signature": "windows",
                    "size": 55_000_000
                }
            }
        });

        assert_eq!(
            manifest_download_size(&manifest, "https://example.com/Captures-setup.exe"),
            Some(55_000_000)
        );
        assert_eq!(
            manifest_download_size(&manifest, "https://example.com/missing"),
            None
        );
    }

    #[test]
    fn tray_update_label_uses_change_count_instead_of_a_version() {
        let notes = Some(
            "> [!WARNING]\n> Experimental.\n\n## What's Changed\n* First fix by @a in https://example.com/1\n* Second fix by @b in https://example.com/2\n\n**Full Changelog**: https://example.com",
        );
        let changelog = vec![
            UpdateChangelogEntry {
                version: "2026.8.2705".into(),
                display_version: "2026.08.27.5".into(),
                notes: notes.map(str::to_owned),
            },
            UpdateChangelogEntry {
                version: "2026.8.2704".into(),
                display_version: "2026.08.27.4".into(),
                notes: Some("* Third fix by @c in https://example.com/3".into()),
            },
        ];
        assert_eq!(
            update_available_menu_label(&changelog, notes),
            "Update Available — 3 changes"
        );
        assert_eq!(
            update_available_menu_label(&[], Some("* One change by @a in https://example.com/1")),
            "Update Available — 1 change"
        );
        assert_eq!(update_available_menu_label(&[], None), "Update Available");
        assert_eq!(
            update_available_menu_label(
                &[],
                Some(
                    "* Real fix by @a in https://example.com/1\n* @bot made their first contribution in https://example.com/1\n* Another fix by @b in https://example.com/2",
                ),
            ),
            "Update Available — 2 changes"
        );
        let item = tray_update_item(&UpdateStatus::Available {
            current_version: "2026.8.2702".into(),
            current_display_version: "2026.08.27.2".into(),
            version: "2026.8.2705".into(),
            display_version: "2026.08.27.5".into(),
            notes: notes.map(str::to_owned),
            changelog,
            installable: true,
            manual_download_url: None,
            download_size: Some(42_000_000),
            will_close_open_captures: false,
        });
        assert!(item.pin_first);
        assert!(item.enabled);
        assert_eq!(item.label, "Update Available — 3 changes");
    }

    #[test]
    fn preserves_development_and_invalid_versions() {
        assert_eq!(display_version("0.1.0"), "0.1.0");
        assert_eq!(display_version("2026.13.1901"), "2026.13.1901");
        assert_eq!(display_version("2026.7.1900"), "2026.7.1900");
    }

    #[test]
    fn blocks_restart_only_for_an_in_progress_recording() {
        assert_eq!(
            restart_blocker(true),
            Some("Finish or cancel the active recording before installing the update.")
        );
        assert_eq!(restart_blocker(false), None);
    }

    #[test]
    fn warns_when_open_captures_will_close_for_an_update() {
        assert!(open_captures_will_close_from(true, false, false));
        assert!(open_captures_will_close_from(false, true, false));
        assert!(open_captures_will_close_from(false, false, true));
        assert!(!open_captures_will_close_from(false, false, false));
    }

    #[test]
    fn warns_only_for_unsaved_captures_with_mini_previews_enabled() {
        assert!(unsaved_mini_previews_are_open(true, true));
        assert!(!unsaved_mini_previews_are_open(true, false));
        assert!(!unsaved_mini_previews_are_open(false, true));
    }

    #[test]
    fn closes_editor_and_viewer_windows_before_an_update() {
        assert!(capture_window_should_close_for_update(
            "screenshot-editor-capture-1"
        ));
        assert!(capture_window_should_close_for_update(
            "recording-editor-clip-9"
        ));
        assert!(capture_window_should_close_for_update("viewer-capture-1"));
        assert!(capture_window_should_close_for_update("viewer"));
        assert!(!capture_window_should_close_for_update("update"));
        assert!(!capture_window_should_close_for_update("thumbnail"));
        assert!(!capture_window_should_close_for_update("preferences"));
        assert!(!capture_window_should_close_for_update("history"));
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
    fn consumes_update_restart_marker_once() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let marker = directory.path().join("update-restart-pending");
        std::fs::write(&marker, []).expect("marker should be written");

        assert!(take_restart_marker(&marker).expect("marker should be consumed"));
        assert!(!take_restart_marker(&marker).expect("consumed marker should stay absent"));
    }

    #[test]
    fn only_manual_check_errors_change_visible_status() {
        assert!(
            check_error_status(false, "1.0.0".into(), "1.0.0".into(), "offline".into()).is_none()
        );
        assert!(matches!(
            check_error_status(true, "1.0.0".into(), "1.0.0".into(), "offline".into()),
            Some(UpdateStatus::Error { message, retry_install, .. })
                if message == "offline" && !retry_install
        ));
    }

    #[test]
    fn keeps_failed_update_notices_visible_during_capture() {
        let error = UpdateStatus::Error {
            current_version: "2026.7.1901".into(),
            current_display_version: "2026.07.19.1".into(),
            message: "Download request failed with status: 403 Forbidden".into(),
            retry_install: true,
        };
        assert!(!should_hide_update_notice_status(&error));
        assert_eq!(notice_disposition(&error, true), NoticeDisposition::Show);
        assert_eq!(notice_disposition(&error, false), NoticeDisposition::Show);
        assert!(should_hide_update_notice_status(&UpdateStatus::UpToDate {
            current_version: "2026.7.1901".into(),
            current_display_version: "2026.07.19.1".into(),
        }));
    }

    #[test]
    fn retries_github_rate_limit_download_failures() {
        assert!(download_error_is_retryable(
            "Download request failed with status: 403 Forbidden"
        ));
        assert!(download_error_is_retryable(
            "Download request failed with status: 429 Too Many Requests"
        ));
        assert!(!download_error_is_retryable("invalid signature"));
        assert_eq!(DOWNLOAD_ATTEMPTS, 3);
        assert_eq!(download_retry_delay(1), Duration::from_millis(400));
        assert_eq!(download_retry_delay(2), Duration::from_millis(800));
        assert!(
            install_error_message("Download request failed with status: 403 Forbidden")
                .contains("GitHub temporarily refused the download")
        );
    }

    #[test]
    fn defers_available_update_notices_until_capture_finishes() {
        let status = UpdateStatus::Available {
            current_version: "2026.7.1901".into(),
            current_display_version: "2026.07.19.1".into(),
            version: "2026.7.1902".into(),
            display_version: "2026.07.19.2".into(),
            notes: None,
            changelog: Vec::new(),
            installable: true,
            manual_download_url: None,
            download_size: None,
            will_close_open_captures: false,
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

    #[test]
    fn waits_for_capture_ui_to_start_before_restoring_an_update_notice() {
        let timeout = Duration::from_millis(1_500);
        assert!(should_wait_for_capture_start(
            Duration::from_millis(0),
            timeout
        ));
        assert!(should_wait_for_capture_start(
            Duration::from_millis(1_499),
            timeout
        ));
        assert!(!should_wait_for_capture_start(timeout, timeout));
        assert!(!should_wait_for_capture_start(
            Duration::from_millis(1_501),
            timeout
        ));
    }

    #[test]
    fn defers_a_visible_or_already_hidden_update_notice() {
        assert!(should_begin_deferred_restore(true, true, false));
        assert!(should_begin_deferred_restore(true, false, true));
        assert!(!should_begin_deferred_restore(true, false, false));
        assert!(!should_begin_deferred_restore(false, true, true));
    }

    #[test]
    fn refreshes_only_an_existing_visible_update_notice() {
        assert!(should_refresh_update_notice(true, true));
        assert!(!should_refresh_update_notice(true, false));
        assert!(!should_refresh_update_notice(false, true));
    }

    #[test]
    fn later_does_not_bring_the_update_notice_back_after_a_capture() {
        assert_eq!(
            notice_restore_plan(true, false, false, false),
            NoticeRestorePlan::Ignore
        );
        assert_eq!(
            notice_restore_plan(true, true, false, true),
            NoticeRestorePlan::Wait
        );
        assert_eq!(
            notice_restore_plan(true, true, false, false),
            NoticeRestorePlan::Show
        );
        assert_eq!(
            notice_restore_plan(true, false, true, false),
            NoticeRestorePlan::Show
        );
    }

    #[test]
    fn update_notice_preserves_its_activation_source_while_already_focused() {
        assert!(should_refresh_notice_activation_source(false));
        assert!(!should_refresh_notice_activation_source(true));
    }

    #[test]
    fn stacks_changelog_entries_between_the_installed_and_latest_versions() {
        let manifest = serde_json::json!({
            "version": "2026.8.2705",
            "notes": "Five",
            "changelog": [
                {
                    "version": "2026.8.2702",
                    "display_version": "2026.08.27.2",
                    "notes": "Two"
                },
                {
                    "version": "2026.8.2703",
                    "display_version": "2026.08.27.3",
                    "notes": "Three"
                },
                {
                    "version": "2026.8.2704",
                    "display_version": "2026.08.27.4",
                    "notes": "Four"
                },
                {
                    "version": "2026.8.2705",
                    "display_version": "2026.08.27.5",
                    "notes": "Five"
                }
            ]
        });

        assert_eq!(
            stacked_changelog(&manifest, "2026.8.2703", "2026.8.2705"),
            vec![
                UpdateChangelogEntry {
                    version: "2026.8.2705".into(),
                    display_version: "2026.08.27.5".into(),
                    notes: Some("Five".into()),
                },
                UpdateChangelogEntry {
                    version: "2026.8.2704".into(),
                    display_version: "2026.08.27.4".into(),
                    notes: Some("Four".into()),
                },
            ]
        );
        assert!(!version_is_newer_than("2026.8.2703", "2026.8.2703"));
        assert!(version_is_newer_than("2026.8.2704", "2026.8.2703"));
        assert!(!version_is_newer_than("2026.8.2704", "2026.8.2705"));
    }

    #[test]
    fn keeps_a_compact_notice_for_one_version_and_grows_for_stacked_notes() {
        let single = available_status(vec![UpdateChangelogEntry {
            version: "2026.7.1902".into(),
            display_version: "2026.07.19.2".into(),
            notes: Some("Adds automatic releases.".into()),
        }]);
        let stacked = available_status(vec![
            UpdateChangelogEntry {
                version: "2026.8.2705".into(),
                display_version: "2026.08.27.5".into(),
                notes: Some("Five".into()),
            },
            UpdateChangelogEntry {
                version: "2026.8.2704".into(),
                display_version: "2026.08.27.4".into(),
                notes: Some("Four".into()),
            },
            UpdateChangelogEntry {
                version: "2026.8.2703".into(),
                display_version: "2026.08.27.3".into(),
                notes: Some("Three".into()),
            },
        ]);
        assert_eq!(update_notice_height(&single), 290.0);
        assert_eq!(update_notice_height(&stacked), 434.0);
        let warned = UpdateStatus::Available {
            current_version: "2026.7.1901".into(),
            current_display_version: "2026.07.19.1".into(),
            version: "2026.7.1902".into(),
            display_version: "2026.07.19.2".into(),
            notes: None,
            changelog: vec![UpdateChangelogEntry {
                version: "2026.7.1902".into(),
                display_version: "2026.07.19.2".into(),
                notes: Some("Adds automatic releases.".into()),
            }],
            installable: true,
            manual_download_url: None,
            download_size: None,
            will_close_open_captures: true,
        };
        assert_eq!(update_notice_height(&warned), 346.0);
        let failed = UpdateStatus::Error {
            current_version: "2026.7.1901".into(),
            current_display_version: "2026.07.19.1".into(),
            message:
                "Could not install the update: Download request failed with status: 404 Not Found"
                    .into(),
            retry_install: true,
        };
        assert_eq!(update_notice_height(&failed), 330.0);
    }

    #[test]
    fn failed_updates_open_the_website_download_page() {
        assert_eq!(DOWNLOAD_PAGE_URL, "https://captur.es/#download");
        assert_ne!(DOWNLOAD_PAGE_URL, RELEASES_URL);
    }

    fn available_status(changelog: Vec<UpdateChangelogEntry>) -> UpdateStatus {
        UpdateStatus::Available {
            current_version: "2026.7.1901".into(),
            current_display_version: "2026.07.19.1".into(),
            version: "2026.7.1902".into(),
            display_version: "2026.07.19.2".into(),
            notes: None,
            changelog,
            installable: true,
            manual_download_url: None,
            download_size: None,
            will_close_open_captures: false,
        }
    }
}
