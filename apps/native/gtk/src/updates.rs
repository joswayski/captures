//! Preview updates for the resident app, as the Win32 host does them. An
//! installed Preview (`CAPTURES_NATIVE_PREVIEW_VERSION`) owns one shared
//! `CheckWorker::preview` from launch: it checks at launch and every five
//! minutes, downloads and verifies the signed Preview, and Update now hands
//! the verified AppImage to the shared update helper with the open previews,
//! editors and Preferences, then quits. A `.deb` install cannot replace
//! itself, so its card offers shipping's View release instead. Development
//! builds start no worker and keep "Updates unavailable".
//!
//! [`card`] maps the worker's status to shipping's `UpdatePreferences`
//! (`apps/desktop/ui/src/App.tsx`); `preferences/updates.rs` draws it.
//!
//! `ReleaseInfo` keeps `Renderer::Wgpu`: the Preview `latest.json` has no
//! renderer field, and shutdown intents and the recovery helper match Wgpu for
//! every non-macOS host.
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use captures_app::update_notice::format_update_size;
use captures_app::updater::checks::{CheckStatus, CheckWorker};
use captures_app::updater::{HealthAcknowledgement, RestartCaptureWindow, RestartIntent};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::{Host, emit};

/// Shipping's automatic check interval.
const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Shipping's View release for a copy that cannot update itself.
pub const RELEASES_URL: &str = "https://github.com/joswayski/captures/releases";
/// The download page behind "download from captur.es".
pub use captures_app::update_notice::DOWNLOAD_PAGE_URL;
pub const CHANNEL: &str = "Preview channel";
/// Shipping's Linux Preferences fallback, split around its link.
pub const FALLBACK_BEFORE: &str = "If this copy cannot update itself,";
pub const FALLBACK_LINK: &str = "download from captur.es";
pub const FALLBACK_AFTER: &str = "Debian packages replace this app; AppImage users should replace ~/.local/bin/Captures.AppImage. Settings and captures stay.";
/// The wgpu host's Debian copy, on View release's tooltip and description.
pub const MANUAL_DETAIL: &str =
    "Debian packages use manual updates. Download the latest native Preview from captur.es.";

/// What the card's button does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Check,
    /// Download and verify, then install (Update now, Try again).
    Download,
    /// Install the already verified package.
    Install,
    /// A `.deb` install: open the releases page (shipping's View release).
    ViewRelease,
}

/// The card's live copy.
#[derive(Clone, PartialEq, Debug)]
pub struct Card {
    /// `Version 2026.11.12.1`.
    pub version: String,
    pub channel: &'static str,
    pub action: &'static str,
    /// `None` while the button is disabled.
    pub step: Option<Step>,
    /// `.update-settings-status`: the heading, then its `small` detail.
    pub heading: String,
    pub detail: String,
    /// `.update-settings-status-current`: positive text.
    pub current: bool,
    /// The progress bar while installing: `Some(None)` is indeterminate.
    pub progress: Option<Option<f32>>,
    /// `.update-settings-warning`: open captures close or reopen.
    pub warning: Option<&'static str>,
    /// `.update-settings-error` (`role="alert"`).
    pub error: Option<String>,
}

/// Shipping `display_version`: `2026.11.1205` reads `2026.11.12.5`.
pub fn display_version(version: &str) -> String {
    let normalized = version.trim_start_matches('v');
    let core = normalized
        .split_once(['-', '+'])
        .map_or(normalized, |(core, _)| core);
    let parts: Vec<_> = core.split('.').map(str::parse::<u32>).collect();
    let [Ok(year), Ok(month), Ok(patch)] = parts[..] else {
        return normalized.to_owned();
    };
    let (day, revision) = (patch / 100, patch % 100);
    if !(2000..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(1..=99).contains(&revision)
    {
        return normalized.to_owned();
    }
    format!("{year:04}.{month:02}.{day:02}.{revision}")
}

/// The card for `status`. `installing` is set once Update now (or Try
/// again) was pressed and stays set until the install hands off or fails.
/// `installable` is false for a `.deb` install; `will_close` means a
/// capture or editor is open (shipping's `will_close_open_captures`).
pub fn card(
    current: &str,
    status: &CheckStatus,
    installing: bool,
    installable: bool,
    will_close: bool,
) -> Card {
    let mut card = Card {
        version: format!("Version {}", display_version(current)),
        channel: CHANNEL,
        action: "Check Now",
        step: Some(Step::Check),
        heading: String::new(),
        detail: String::new(),
        current: false,
        progress: None,
        warning: None,
        error: None,
    };
    let updating = |card: &mut Card, version: &str| {
        card.heading = format!("Updating to version {}", display_version(version));
        card.action = "Installing…";
        card.step = None;
    };
    let available = |card: &mut Card, version: &str| {
        card.heading = format!("Version {} is available", display_version(version));
        card.warning =
            will_close.then(|| captures_app::update_notice::open_captures_warning(installable));
    };
    match status {
        CheckStatus::Idle => {}
        CheckStatus::Checking => {
            card.action = "Checking now…";
            card.step = None;
        }
        CheckStatus::UpToDate => {
            card.heading = "Up to date".into();
            card.current = true;
        }
        CheckStatus::Available { release } if !installable => {
            available(&mut card, &release.version);
            card.action = "View release";
            card.step = Some(Step::ViewRelease);
        }
        CheckStatus::Available { release } => {
            available(&mut card, &release.version);
            if release.size > 0 {
                card.detail = format!("{} download", format_update_size(release.size));
            }
            card.action = "Update now";
            card.step = Some(Step::Download);
        }
        CheckStatus::Downloading {
            release,
            downloaded,
            total,
        } => {
            updating(&mut card, &release.version);
            let progress = (*total > 0).then(|| {
                ((*downloaded as f64 / *total as f64) * 100.)
                    .round()
                    .min(100.)
            });
            card.detail = match progress {
                Some(percent) => format!(
                    "{} / {} · {percent}%",
                    format_update_size(*downloaded),
                    format_update_size(*total)
                ),
                None => format!("{} downloaded", format_update_size(*downloaded)),
            };
            card.progress = Some(progress.map(|p| p as f32 / 100.));
        }
        CheckStatus::Verifying { release } | CheckStatus::Cancelling { release } => {
            updating(&mut card, &release.version);
            card.progress = Some(None);
        }
        CheckStatus::Staged { .. } if installing => {
            card.heading = "Update complete".into();
            card.detail = "Reopening Captures…".into();
            card.action = "Restarting…";
            card.step = None;
        }
        // Verified, but the install was held (a capture was running).
        CheckStatus::Staged { release, .. } => {
            available(&mut card, &release.version);
            card.action = "Update now";
            card.step = Some(Step::Install);
        }
        CheckStatus::Error { message } => {
            card.heading = "Couldn’t check for updates".into();
            card.error = Some(message.clone());
        }
        CheckStatus::DownloadError { message, .. } => {
            card.heading = "Update failed".into();
            card.action = "Try again";
            card.step = Some(Step::Download);
            card.error = Some(format!("Could not install the update: {message}"));
        }
    }
    card
}

/// `--scene preferences --update-card STATE`: a Preview card for offscreen
/// renders and the smoke (see [`FIXTURES`]).
pub fn fixture(name: &str) -> Option<Card> {
    use captures_app::updater::{ReleaseInfo, Renderer, Target};
    let release = ReleaseInfo {
        renderer: Renderer::Wgpu,
        target: Target::LinuxX64,
        version: "2026.11.1302".into(),
        notes: None,
        size: 104_000_000,
    };
    let (status, installing, installable, will_close) = match name {
        "idle" => (CheckStatus::Idle, false, true, false),
        "checking" => (CheckStatus::Checking, false, true, false),
        "current" => (CheckStatus::UpToDate, false, true, false),
        "available" => (CheckStatus::Available { release }, false, true, false),
        "closing" => (CheckStatus::Available { release }, false, true, true),
        "manual" => (CheckStatus::Available { release }, false, false, true),
        "downloading" => (
            CheckStatus::Downloading {
                release,
                downloaded: 52_000_000,
                total: 104_000_000,
            },
            true,
            true,
            false,
        ),
        "verifying" => (CheckStatus::Verifying { release }, true, true, false),
        "restarting" => (
            CheckStatus::Staged {
                release,
                sha256: String::new(),
            },
            true,
            true,
            false,
        ),
        "failed" => (
            CheckStatus::DownloadError {
                release,
                message: "Native update signature verification failed.".into(),
            },
            false,
            true,
            false,
        ),
        "check-failed" => (
            CheckStatus::Error {
                message: "Could not reach the native update service.".into(),
            },
            false,
            true,
            false,
        ),
        _ => return None,
    };
    Some(card(
        "2026.11.1201",
        &status,
        installing,
        installable,
        will_close,
    ))
}

/// Every [`fixture`] name.
pub const FIXTURES: [&str; 11] = [
    "idle",
    "checking",
    "current",
    "available",
    "closing",
    "manual",
    "downloading",
    "verifying",
    "restarting",
    "failed",
    "check-failed",
];

struct Resident {
    host: Weak<Host>,
    worker: CheckWorker,
    /// Update now was pressed: install once the package is verified.
    installing: bool,
}

thread_local! {
    static RESIDENT: RefCell<Option<Resident>> = const { RefCell::new(None) };
    /// `--update-card`: the fixture Preferences shows instead.
    static FIXTURE: RefCell<Option<Card>> = const { RefCell::new(None) };
}

/// Show `card` in Preferences instead of a live state (fixtures only).
pub fn set_fixture(card: Card) {
    FIXTURE.with(|fixture| *fixture.borrow_mut() = Some(card));
}

/// Start the installed Preview's worker for the resident app. Development
/// builds make no request.
pub fn start(host: &Rc<Host>) {
    if captures_app::preview_channel::version().is_none() {
        return;
    }
    // The worker wakes from its own thread; serve it on the main loop.
    let wake = Arc::new(|| {
        glib::MainContext::default().invoke(|| {
            glib::idle_add_local_once(service);
        });
    });
    let mut worker = match CheckWorker::preview(captures_app::updater::Renderer::Wgpu, wake) {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("Preview updates unavailable: {error}");
            return;
        }
    };
    worker.check_automatically();
    RESIDENT.with(|resident| {
        *resident.borrow_mut() = Some(Resident {
            host: Rc::downgrade(host),
            worker,
            installing: false,
        })
    });
    glib::timeout_add_local(INTERVAL, || {
        let running = RESIDENT.with(|resident| {
            let mut resident = resident.borrow_mut();
            let Some(resident) = resident.as_mut() else {
                return false;
            };
            resident.worker.poll();
            if matches!(
                resident.worker.status(),
                CheckStatus::Idle | CheckStatus::UpToDate | CheckStatus::Error { .. }
            ) {
                resident.worker.check_automatically();
            }
            true
        });
        if running {
            service();
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
}

/// Join the worker before the profile election is released.
pub fn stop() {
    if let Some(mut resident) = RESIDENT.with(|resident| resident.borrow_mut().take()) {
        resident.worker.shutdown();
    }
}

/// The Preferences card: a fixture, the live Preview state, or `None` in a
/// development build.
pub fn current() -> Option<Card> {
    if let Some(card) = FIXTURE.with(|fixture| fixture.borrow().clone()) {
        return Some(card);
    }
    RESIDENT.with(|resident| {
        let resident = resident.try_borrow().ok()?;
        let resident = resident.as_ref()?;
        let host = resident.host.upgrade()?;
        let status = resident.worker.status();
        // Only an installable package gets a staging directory, which is
        // when the shared worker offers its download.
        let installable = !matches!(status, CheckStatus::Available { .. })
            || resident.worker.presentation().acquisition.is_some();
        Some(card(
            captures_app::preview_channel::version()?,
            status,
            resident.installing,
            installable,
            capture_active(&host).is_some()
                || !host.editors.borrow().is_empty()
                || !host.recordings.borrow().is_empty(),
        ))
    })
}

/// The tray's Check for Updates: check now; Preferences shows the card.
pub fn check_now() {
    act(Step::Check);
}

/// The card's button.
pub fn act(step: Step) {
    if step == Step::ViewRelease {
        if let Err(error) =
            gio::AppInfo::launch_default_for_uri(RELEASES_URL, gio::AppLaunchContext::NONE)
        {
            eprintln!("Could not open the releases page: {error}");
        }
        return;
    }
    RESIDENT.with(|resident| {
        let mut resident = resident.borrow_mut();
        let Some(resident) = resident.as_mut() else {
            return;
        };
        match step {
            Step::Check => {
                resident.worker.check();
            }
            Step::Download => resident.installing = resident.worker.download(),
            Step::Install => resident.installing = true,
            Step::ViewRelease => {}
        }
    });
    emit(
        "update-action",
        serde_json::json!({"step": format!("{step:?}")}),
    );
    service();
}

/// Publish worker progress and hand a verified package to the helper once
/// Update now asked for it.
fn service() {
    let install = RESIDENT.with(|resident| {
        let mut resident = resident.borrow_mut();
        let resident = resident.as_mut()?;
        resident.worker.poll();
        // A failed download or install ends this attempt.
        if matches!(resident.worker.status(), CheckStatus::DownloadError { .. }) {
            resident.installing = false;
        }
        Some(resident.installing && matches!(resident.worker.status(), CheckStatus::Staged { .. }))
    });
    if install == Some(true) {
        self::install();
    }
    crate::preferences::refresh_updates();
}

/// What stops an install now, as shipping's `install_restart_blocker`.
fn capture_active(host: &Host) -> Option<&'static str> {
    if host.recorder.borrow().is_some() {
        Some("Finish or cancel the active recording before installing the update.")
    } else if host.capture.borrow().is_some() || host.selector.borrow().is_some() {
        Some("Finish the capture before installing the update.")
    } else {
        None
    }
}

/// Shipping's Update now after verification: refuse during a capture or
/// recording, remember what is open, start the copied helper and quit. The
/// helper waits for this process's package lock, replaces the AppImage and
/// relaunches it with the readiness token and this restart intent.
fn install() {
    let Some(host) =
        RESIDENT.with(|resident| resident.borrow().as_ref().and_then(|r| r.host.upgrade()))
    else {
        return;
    };
    if let Some(message) = capture_active(&host) {
        RESIDENT.with(|resident| {
            if let Some(resident) = resident.borrow_mut().as_mut() {
                resident.installing = false;
            }
        });
        host.show_error(message);
        return;
    }
    let restart = RestartIntent {
        restore_preferences: host.preferences.borrow().is_some(),
        preview_artifact_ids: host
            .previews
            .borrow()
            .as_ref()
            .map(|previews| previews.ids())
            .unwrap_or_default(),
        capture_windows: host
            .editors
            .borrow()
            .iter()
            .map(|(artifact_id, _)| RestartCaptureWindow::ScreenshotEditor {
                artifact_id: artifact_id.clone(),
                source_artifact_id: None,
            })
            .chain(host.recordings.borrow().iter().map(|(artifact_id, _)| {
                RestartCaptureWindow::RecordingEditor {
                    artifact_id: artifact_id.clone(),
                }
            }))
            .collect(),
    };
    let result = RESIDENT.with(|resident| {
        let mut resident = resident.borrow_mut();
        let resident = resident.as_mut().ok_or("Captures is closing")?;
        let result = resident
            .worker
            .launch_preview_install(restart)
            .map_err(|error| error.to_string());
        if result.is_err() {
            resident.installing = false;
        }
        result
    });
    match result {
        Ok(()) => {
            emit("update-install", serde_json::json!({}));
            // Closing an editor saves its draft; then quit, which releases
            // the package lock the helper waits for.
            let editors: Vec<_> = host
                .editors
                .borrow()
                .iter()
                .map(|(_, editor)| editor.window.clone())
                .collect();
            for window in editors {
                window.close();
            }
            host.app.quit();
        }
        // The worker now reports Update failed with the reason.
        Err(error) => eprintln!("Could not start the update: {error}"),
    }
}

/// An update restart: the previews, editors and Preferences that were open.
pub fn restore(host: &Rc<Host>, restart: &RestartIntent) {
    let entries =
        captures_history::load(&host.history_root, chrono::Utc::now()).unwrap_or_default();
    let entry = |id: &str| entries.iter().find(|entry| entry.id == id);
    for id in &restart.preview_artifact_ids {
        if let Some(entry) = entry(id) {
            host.show_preview(entry.clone());
        }
    }
    for window in &restart.capture_windows {
        match window {
            RestartCaptureWindow::ScreenshotEditor { artifact_id, .. } => {
                match entry(artifact_id) {
                    Some(entry) => host.edit(entry),
                    None => eprintln!("Could not reopen a capture after the update: {artifact_id}"),
                }
            }
            RestartCaptureWindow::RecordingEditor { artifact_id } => {
                host.show_recording(artifact_id);
            }
        }
    }
    if restart.restore_preferences {
        host.show_preferences();
    }
    emit(
        "update-restored",
        serde_json::json!({
            "previews": restart.preview_artifact_ids.len(),
            "editors": restart.capture_windows.len(),
            "preferences": restart.restore_preferences,
        }),
    );
}

/// Confirm the installer's or recovery companion's readiness file once
/// settings load, GTK has a display and the bundled FFmpeg/FFprobe run.
pub fn acknowledge(host: &Rc<Host>, acknowledgement: HealthAcknowledgement) {
    let settings_file = host.settings_file.clone();
    glib::spawn_future_local(async move {
        let tools = gio::spawn_blocking(verify_media_tools)
            .await
            .unwrap_or_else(|_| Err("the media check stopped".into()));
        let settings = captures_settings::load(&settings_file).map_err(|error| error.to_string());
        let display = gtk::gdk::Display::default().is_some();
        match (tools, settings, display) {
            (Ok(()), Ok(_), true) => match acknowledgement.acknowledge() {
                Ok(()) => emit("update-ready", serde_json::json!({})),
                Err(error) => eprintln!("Could not confirm startup: {error}"),
            },
            (tools, settings, display) => eprintln!(
                "Startup not confirmed: media tools {tools:?}, display {display}, settings {:?}",
                settings.err()
            ),
        }
    });
}

/// The exact FFmpeg/FFprobe pair an installed package carries (beside the
/// executable), as the wgpu host's `verify_bundled`; a development build
/// uses the host's usual lookup.
fn verify_media_tools() -> Result<(), String> {
    let (ffmpeg, ffprobe) = if captures_app::preview_channel::version().is_some() {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let parent = executable
            .parent()
            .ok_or("Packaged executable has no parent")?;
        (parent.join("ffmpeg"), parent.join("ffprobe"))
    } else {
        crate::recording::media_tools()
    };
    captures_media::MediaToolchain::new(ffmpeg, ffprobe)
        .verify()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::updater::{ReleaseInfo, Renderer, Target};

    fn release() -> ReleaseInfo {
        ReleaseInfo {
            renderer: Renderer::Wgpu,
            target: Target::LinuxX64,
            version: "2026.11.1302".into(),
            notes: None,
            size: 104_000_000,
        }
    }

    fn live(status: &CheckStatus, installing: bool) -> Card {
        card("2026.11.1201", status, installing, true, false)
    }

    #[test]
    fn display_versions_follow_shipping() {
        assert_eq!(display_version("2026.11.1302"), "2026.11.13.2");
        assert_eq!(display_version("v2026.1.105"), "2026.01.01.5");
        // Not a dated Preview: unchanged.
        assert_eq!(display_version("0.1.0"), "0.1.0");
        assert_eq!(display_version("2026.11.1300"), "2026.11.1300");
    }

    #[test]
    fn idle_and_checking_offer_check_now() {
        let idle = live(&CheckStatus::Idle, false);
        assert_eq!(idle.version, "Version 2026.11.12.1");
        assert_eq!(idle.channel, "Preview channel");
        assert_eq!((idle.action, idle.step), ("Check Now", Some(Step::Check)));
        assert!(idle.heading.is_empty() && idle.error.is_none() && idle.warning.is_none());
        let checking = live(&CheckStatus::Checking, false);
        assert_eq!((checking.action, checking.step), ("Checking now…", None));
        let current = live(&CheckStatus::UpToDate, false);
        assert_eq!(current.heading, "Up to date");
        assert!(current.current);
        assert_eq!(current.step, Some(Step::Check));
    }

    #[test]
    fn available_updates_now_and_reports_progress() {
        let available = live(&CheckStatus::Available { release: release() }, false);
        assert_eq!(available.heading, "Version 2026.11.13.2 is available");
        assert_eq!(available.detail, "104 MB download");
        assert_eq!(
            (available.action, available.step),
            ("Update now", Some(Step::Download))
        );
        let downloading = live(
            &CheckStatus::Downloading {
                release: release(),
                downloaded: 52_000_000,
                total: 104_000_000,
            },
            true,
        );
        assert_eq!(downloading.heading, "Updating to version 2026.11.13.2");
        assert_eq!(downloading.detail, "52.0 MB / 104 MB · 50%");
        assert_eq!(
            (downloading.action, downloading.step),
            ("Installing…", None)
        );
        assert_eq!(downloading.progress, Some(Some(0.5)));
        let unknown = live(
            &CheckStatus::Downloading {
                release: release(),
                downloaded: 1_000,
                total: 0,
            },
            true,
        );
        assert_eq!(unknown.detail, "1.0 KB downloaded");
        assert_eq!(unknown.progress, Some(None));
        let verifying = live(&CheckStatus::Verifying { release: release() }, true);
        assert_eq!(
            (verifying.action, verifying.progress),
            ("Installing…", Some(None))
        );
    }

    #[test]
    fn a_deb_install_views_the_release_without_a_size() {
        let status = CheckStatus::Available { release: release() };
        let manual = card("2026.11.1201", &status, false, false, false);
        assert_eq!(manual.heading, "Version 2026.11.13.2 is available");
        assert!(manual.detail.is_empty(), "{}", manual.detail);
        assert_eq!(
            (manual.action, manual.step),
            ("View release", Some(Step::ViewRelease))
        );
        assert!(manual.warning.is_none());
        // Open captures: shipping's warning for each kind of install.
        let closing = card("2026.11.1201", &status, false, false, true);
        assert_eq!(
            closing.warning,
            Some("Open captures will close. Unsaved edits are kept as drafts.")
        );
        let reopening = card("2026.11.1201", &status, false, true, true);
        assert_eq!(
            reopening.warning,
            Some("Open captures reopen after the update. Screenshot drafts are kept.")
        );
        // Only an available update warns.
        assert!(
            card("2026.11.1201", &CheckStatus::UpToDate, false, true, true)
                .warning
                .is_none()
        );
    }

    #[test]
    fn a_staged_package_restarts_or_waits_for_update_now() {
        let staged = CheckStatus::Staged {
            release: release(),
            sha256: "00".into(),
        };
        let restarting = live(&staged, true);
        assert_eq!(restarting.heading, "Update complete");
        assert_eq!(restarting.detail, "Reopening Captures…");
        assert_eq!((restarting.action, restarting.step), ("Restarting…", None));
        let held = live(&staged, false);
        assert_eq!(
            (held.action, held.step),
            ("Update now", Some(Step::Install))
        );
    }

    #[test]
    fn failures_use_shipping_headings_and_actions() {
        let check = live(
            &CheckStatus::Error {
                message: "Could not reach the native update service.".into(),
            },
            false,
        );
        assert_eq!(check.heading, "Couldn’t check for updates");
        assert_eq!((check.action, check.step), ("Check Now", Some(Step::Check)));
        assert_eq!(
            check.error.as_deref(),
            Some("Could not reach the native update service.")
        );
        let install = live(
            &CheckStatus::DownloadError {
                release: release(),
                message: "Native update signature verification failed.".into(),
            },
            false,
        );
        assert_eq!(install.heading, "Update failed");
        assert_eq!(
            (install.action, install.step),
            ("Try again", Some(Step::Download))
        );
        assert_eq!(
            install.error.as_deref(),
            Some("Could not install the update: Native update signature verification failed.")
        );
    }

    #[test]
    fn every_fixture_renders() {
        for name in FIXTURES {
            assert!(fixture(name).is_some(), "{name}");
        }
        assert!(fixture("nope").is_none());
    }
}
