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
use std::time::{Duration, Instant};

use captures_app::update_notice::{UpdateStatus, format_update_size};
use captures_app::updater::checks::{CheckStatus, CheckWorker};
use captures_app::updater::{
    HealthAcknowledgement, ReleaseInfo, Renderer, RestartCaptureWindow, RestartIntent, Target,
};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::notices::UpdateNotice;
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

/// What the live notice needs beside the worker's status.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct NoticeFlags {
    /// Update now (or Try again) was pressed and has not handed off or failed.
    pub installing: bool,
    /// The package can replace itself (an AppImage, not a `.deb`).
    pub installable: bool,
    /// A capture or editor is open (shipping's `will_close_open_captures`).
    pub will_close: bool,
    /// The last check was asked for (Check Now, Check again, the tray).
    /// Shipping reports only a manual check's progress and failure.
    pub manual: bool,
}

/// The shipping notice status (`update_notice::UpdateStatus`) for the live
/// Preview worker's `status`, or `None` for a state shipping never
/// publishes (idle, or an automatic check in flight or failed).
///
/// A verified package waiting on Update now reads as the available update.
/// Once it hands off, the native install starts the helper and quits in the
/// same main-loop turn, with no countdown to show, so the notice keeps the
/// finished download (a full bar) rather than shipping's "Reopening in 3
/// seconds…", which would promise a relaunch time the helper does not keep.
pub fn notice_status(
    current: &str,
    status: &CheckStatus,
    flags: NoticeFlags,
) -> Option<UpdateStatus> {
    let current_version = current.to_owned();
    let current_display_version = display_version(current);
    let downloading =
        |release: &ReleaseInfo, downloaded: u64, total: Option<u64>| UpdateStatus::Downloading {
            current_version: current_version.clone(),
            current_display_version: current_display_version.clone(),
            version: release.version.clone(),
            display_version: display_version(&release.version),
            downloaded,
            total: total.filter(|total| *total > 0),
        };
    let available = |release: &ReleaseInfo, installable: bool| UpdateStatus::Available {
        current_version: current_version.clone(),
        current_display_version: current_display_version.clone(),
        version: release.version.clone(),
        display_version: display_version(&release.version),
        notes: release
            .notes
            .clone()
            .filter(|notes| !notes.trim().is_empty()),
        changelog: Vec::new(),
        installable,
        manual_download_url: (!installable).then(|| RELEASES_URL.to_owned()),
        download_size: (release.size > 0).then_some(release.size),
        will_close_open_captures: flags.will_close,
    };
    Some(match status {
        CheckStatus::Idle => return None,
        CheckStatus::Checking if flags.manual => UpdateStatus::Checking {
            current_version,
            current_display_version,
        },
        CheckStatus::Checking => return None,
        CheckStatus::UpToDate => UpdateStatus::UpToDate {
            current_version,
            current_display_version,
        },
        CheckStatus::Available { release } => available(release, flags.installable),
        CheckStatus::Downloading {
            release,
            downloaded,
            total,
        } => downloading(release, *downloaded, Some(*total)),
        CheckStatus::Verifying { release } | CheckStatus::Cancelling { release } => {
            downloading(release, release.size, None)
        }
        CheckStatus::Staged { release, .. } if flags.installing => {
            downloading(release, release.size, Some(release.size))
        }
        CheckStatus::Staged { release, .. } => available(release, true),
        CheckStatus::DownloadError { message, .. } => UpdateStatus::Error {
            current_version,
            current_display_version,
            message: format!("Could not install the update: {message}"),
            retry_install: true,
        },
        CheckStatus::Error { message } if flags.manual => UpdateStatus::Error {
            current_version,
            current_display_version,
            message: message.clone(),
            retry_install: false,
        },
        CheckStatus::Error { .. } => return None,
    })
}

/// The notice's Update now, View release or Try again for `status`.
pub fn notice_step(status: &CheckStatus, flags: NoticeFlags) -> Option<Step> {
    match status {
        CheckStatus::Available { .. } if !flags.installable => Some(Step::ViewRelease),
        CheckStatus::Available { .. } | CheckStatus::DownloadError { .. } => Some(Step::Download),
        CheckStatus::Staged { .. } if !flags.installing => Some(Step::Install),
        _ => None,
    }
}

/// Shipping's `notice_disposition`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disposition {
    Show,
    /// Wait until no capture or recording is running.
    Defer,
    Ignore,
}

/// A failure always shows; an available or downloading update waits for a
/// running capture or recording; nothing else opens the notice.
pub fn disposition(status: Option<&UpdateStatus>, capture_active: bool) -> Disposition {
    match status {
        Some(UpdateStatus::Error { .. }) => Disposition::Show,
        Some(UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. }) => {
            if capture_active {
                Disposition::Defer
            } else {
                Disposition::Show
            }
        }
        _ => Disposition::Ignore,
    }
}

/// What [`Schedule::next`] asks of the host for a closed notice. A visible
/// notice simply follows the status in place.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Plan {
    Ignore,
    Show,
    /// Ask again once the capture or recording ends.
    Defer,
    /// The tray's check found nothing (shipping's "Captures is up to date").
    UpToDate,
    /// The tray's check failed (shipping's "Could not check for updates").
    CheckFailed(String),
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
enum Wait {
    #[default]
    None,
    /// A newly available version (`schedule_update_notice`): it shows once
    /// no capture runs, if it is still the available version.
    Version(String),
    /// Show per [`disposition`] (the tray with an update in hand).
    Show,
    /// The tray's manual check: report its result.
    Check,
}

/// Shipping's notice policy: once per newly available version
/// (`notified_version`, cleared when up to date), deferred during a capture
/// or recording; the tray shows it for an update in hand or after its check.
#[derive(Default, Debug)]
pub struct Schedule {
    notified: Option<String>,
    wait: Wait,
}

impl Schedule {
    /// The tray's Check for Updates. Returns whether to start a manual check.
    pub fn tray(&mut self, status: Option<&UpdateStatus>) -> bool {
        let in_hand = matches!(
            status,
            Some(UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. })
        );
        self.wait = if in_hand { Wait::Show } else { Wait::Check };
        !in_hand
    }

    /// The step for the latest `status`.
    pub fn next(
        &mut self,
        status: Option<&UpdateStatus>,
        visible: bool,
        capture_active: bool,
    ) -> Plan {
        match status {
            Some(UpdateStatus::Available { version, .. })
                if self.notified.as_deref() != Some(version) =>
            {
                self.notified = Some(version.clone());
                if self.wait == Wait::None {
                    self.wait = Wait::Version(version.clone());
                }
            }
            Some(UpdateStatus::UpToDate { .. }) => self.notified = None,
            _ => {}
        }
        match std::mem::take(&mut self.wait) {
            Wait::None => Plan::Ignore,
            Wait::Check => match status {
                Some(UpdateStatus::UpToDate { .. }) => Plan::UpToDate,
                Some(UpdateStatus::Error { message, .. }) => Plan::CheckFailed(message.clone()),
                Some(UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. }) => {
                    self.show(Wait::Show, status, visible, capture_active)
                }
                // Still checking.
                _ => {
                    self.wait = Wait::Check;
                    Plan::Ignore
                }
            },
            Wait::Version(version) => match status {
                Some(UpdateStatus::Available { version: now, .. }) if *now == version => {
                    self.show(Wait::Version(version), status, visible, capture_active)
                }
                _ => Plan::Ignore,
            },
            Wait::Show => self.show(Wait::Show, status, visible, capture_active),
        }
    }

    fn show(
        &mut self,
        wait: Wait,
        status: Option<&UpdateStatus>,
        visible: bool,
        capture_active: bool,
    ) -> Plan {
        if visible {
            return Plan::Ignore;
        }
        match disposition(status, capture_active) {
            Disposition::Show => Plan::Show,
            Disposition::Defer => {
                self.wait = wait;
                Plan::Defer
            }
            Disposition::Ignore => Plan::Ignore,
        }
    }
}

/// `--scene update --update-live NAME`: the live notice for one worker
/// state, through [`notice_status`], for renders.
pub fn live_fixture(name: &str) -> Option<(UpdateStatus, bool)> {
    let release = Fake::release();
    let flags = NoticeFlags {
        installable: true,
        ..NoticeFlags::default()
    };
    let (status, flags) = match name {
        "available" => (CheckStatus::Available { release }, flags),
        "closing" => (
            CheckStatus::Available { release },
            NoticeFlags {
                will_close: true,
                ..flags
            },
        ),
        "manual" => (
            CheckStatus::Available { release },
            NoticeFlags {
                installable: false,
                ..flags
            },
        ),
        "downloading" => (
            CheckStatus::Downloading {
                release,
                downloaded: 52_000_000,
                total: 104_000_000,
            },
            NoticeFlags {
                installing: true,
                ..flags
            },
        ),
        "verifying" => (
            CheckStatus::Verifying { release },
            NoticeFlags {
                installing: true,
                ..flags
            },
        ),
        "installing" => (
            CheckStatus::Staged {
                release,
                sha256: String::new(),
            },
            NoticeFlags {
                installing: true,
                ..flags
            },
        ),
        "failed" => (
            CheckStatus::DownloadError {
                release,
                message: "Native update signature verification failed.".into(),
            },
            flags,
        ),
        "check-failed" => (
            CheckStatus::Error {
                message: "Could not reach the native update service.".into(),
            },
            NoticeFlags {
                manual: true,
                ..flags
            },
        ),
        _ => return None,
    };
    Some((
        notice_status(Fake::CURRENT, &status, flags)?,
        flags.installing,
    ))
}

/// Every [`live_fixture`] name.
pub const LIVE_FIXTURES: [&str; 8] = [
    "available",
    "closing",
    "manual",
    "downloading",
    "verifying",
    "installing",
    "failed",
    "check-failed",
];

/// `--live --update-live NAME` scripts: `available` downloads and installs;
/// `retry` fails its first download.
pub const LIVE_SCRIPTS: [&str; 2] = ["available", "retry"];

/// A scripted stand-in for the Preview worker (`--live --update-live`, in
/// development builds only), so the smoke drives the live notice: checks
/// find [`Fake::release`], downloads step to 75 % and verify, and the
/// install quits without starting a helper.
struct Fake {
    status: CheckStatus,
    /// Downloads that fail verification before one succeeds.
    failures: u32,
    due: Option<Instant>,
}

impl Fake {
    const CURRENT: &str = "2026.11.1201";
    const STEP: Duration = Duration::from_millis(400);

    fn release() -> ReleaseInfo {
        ReleaseInfo {
            renderer: Renderer::Wgpu,
            target: Target::LinuxX64,
            version: "2026.11.1302".into(),
            notes: Some(
                "## What's Changed\n* Open the update notice from live checks by @joswayski in https://github.com/joswayski/captures/pull/612\n* Keep the notice in place while it downloads by @joswayski in https://github.com/joswayski/captures/pull/613".into(),
            ),
            size: 104_000_000,
        }
    }

    fn busy(&self) -> bool {
        matches!(
            self.status,
            CheckStatus::Checking | CheckStatus::Downloading { .. } | CheckStatus::Verifying { .. }
        )
    }

    fn check(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        self.status = CheckStatus::Checking;
        self.due = Some(Instant::now() + Self::STEP);
        true
    }

    fn download(&mut self) -> bool {
        let (CheckStatus::Available { release } | CheckStatus::DownloadError { release, .. }) =
            &self.status
        else {
            return false;
        };
        self.status = CheckStatus::Downloading {
            total: release.size,
            release: release.clone(),
            downloaded: 0,
        };
        self.due = Some(Instant::now() + Self::STEP);
        true
    }

    fn poll(&mut self) -> bool {
        if self.due.is_none_or(|due| Instant::now() < due) {
            return false;
        }
        self.due = Some(Instant::now() + Self::STEP);
        self.status = match std::mem::replace(&mut self.status, CheckStatus::Idle) {
            CheckStatus::Checking => CheckStatus::Available {
                release: Self::release(),
            },
            CheckStatus::Downloading {
                release,
                downloaded,
                total,
            } if downloaded + total / 4 < total => CheckStatus::Downloading {
                release,
                downloaded: downloaded + total / 4,
                total,
            },
            CheckStatus::Downloading { release, .. } => CheckStatus::Verifying { release },
            CheckStatus::Verifying { release } if self.failures > 0 => {
                self.failures -= 1;
                CheckStatus::DownloadError {
                    release,
                    message: "Native update signature verification failed.".into(),
                }
            }
            CheckStatus::Verifying { release } => CheckStatus::Staged {
                release,
                sha256: String::new(),
            },
            other => other,
        };
        if !self.busy() {
            self.due = None;
        }
        true
    }
}

/// The installed Preview's worker, or the development script.
enum Source {
    Preview(CheckWorker),
    Fake(Fake),
}

impl Source {
    fn status(&self) -> &CheckStatus {
        match self {
            Self::Preview(worker) => worker.status(),
            Self::Fake(fake) => &fake.status,
        }
    }

    fn poll(&mut self) -> bool {
        match self {
            Self::Preview(worker) => worker.poll(),
            Self::Fake(fake) => fake.poll(),
        }
    }

    fn check(&mut self) -> bool {
        match self {
            Self::Preview(worker) => worker.check(),
            Self::Fake(fake) => fake.check(),
        }
    }

    fn check_automatically(&mut self) -> bool {
        match self {
            Self::Preview(worker) => worker.check_automatically(),
            Self::Fake(fake) => fake.check(),
        }
    }

    fn download(&mut self) -> bool {
        match self {
            Self::Preview(worker) => worker.download(),
            Self::Fake(fake) => fake.download(),
        }
    }

    /// Only an installable package gets a staging directory, which is when
    /// the shared worker offers its download.
    fn installable(&self) -> bool {
        match self {
            Self::Preview(worker) => worker.presentation().acquisition.is_some(),
            Self::Fake(_) => true,
        }
    }

    fn current_version(&self) -> &'static str {
        match self {
            Self::Preview(_) => captures_app::preview_channel::version().unwrap_or_default(),
            Self::Fake(_) => Fake::CURRENT,
        }
    }

    fn launch_install(&mut self, restart: RestartIntent) -> Result<(), String> {
        match self {
            Self::Preview(worker) => worker
                .launch_preview_install(restart)
                .map_err(|error| error.to_string()),
            // The script installs nothing; it only quits as the install would.
            Self::Fake(_) => Ok(()),
        }
    }

    fn shutdown(&mut self) {
        if let Self::Preview(worker) = self {
            worker.shutdown();
        }
    }
}

struct Resident {
    host: Weak<Host>,
    source: Source,
    /// Update now was pressed: install once the package is verified.
    installing: bool,
    /// The last check was asked for, not automatic.
    manual: bool,
    schedule: Schedule,
    /// A deferred notice is waiting for a capture to end.
    deferred: bool,
}

thread_local! {
    static RESIDENT: RefCell<Option<Resident>> = const { RefCell::new(None) };
    /// `--update-card`: the fixture Preferences shows instead.
    static FIXTURE: RefCell<Option<Card>> = const { RefCell::new(None) };
    /// `--live --update-live NAME`: the script [`start`] runs instead.
    static SCRIPT: RefCell<Option<Fake>> = const { RefCell::new(None) };
    /// The floating update notice, while open.
    static NOTICE: RefCell<Option<Rc<UpdateNotice>>> = const { RefCell::new(None) };
}

/// Show `card` in Preferences instead of a live state (fixtures only).
pub fn set_fixture(card: Card) {
    FIXTURE.with(|fixture| *fixture.borrow_mut() = Some(card));
}

/// Run the `--update-live` script instead of the Preview worker (a
/// development build's live app; see [`LIVE_SCRIPTS`]).
pub fn use_script(name: &str) {
    let fake = Fake {
        status: CheckStatus::Idle,
        failures: u32::from(name == "retry"),
        due: None,
    };
    SCRIPT.with(|script| *script.borrow_mut() = Some(fake));
}

/// Whether this app checks for updates: an installed Preview, or the script.
pub fn enabled() -> bool {
    captures_app::preview_channel::version().is_some()
        || SCRIPT.with(|script| script.borrow().is_some())
        || RESIDENT.with(|resident| resident.borrow().is_some())
}

/// Start the installed Preview's worker (or the development script) for
/// the resident app. Development builds otherwise make no request.
pub fn start(host: &Rc<Host>) {
    let source = if let Some(fake) = SCRIPT.with(|script| script.borrow_mut().take()) {
        // The script has no thread to wake the main loop.
        glib::timeout_add_local(Duration::from_millis(100), || {
            if RESIDENT.with(|resident| resident.borrow().is_none()) {
                return glib::ControlFlow::Break;
            }
            service();
            glib::ControlFlow::Continue
        });
        Source::Fake(fake)
    } else {
        if captures_app::preview_channel::version().is_none() {
            return;
        }
        // The worker wakes from its own thread; serve it on the main loop.
        let wake = Arc::new(|| {
            glib::MainContext::default().invoke(|| {
                glib::idle_add_local_once(service);
            });
        });
        match CheckWorker::preview(Renderer::Wgpu, wake) {
            Ok(worker) => Source::Preview(worker),
            Err(error) => {
                eprintln!("Preview updates unavailable: {error}");
                return;
            }
        }
    };
    let mut resident = Resident {
        host: Rc::downgrade(host),
        source,
        installing: false,
        manual: false,
        schedule: Schedule::default(),
        deferred: false,
    };
    resident.source.check_automatically();
    RESIDENT.with(|slot| *slot.borrow_mut() = Some(resident));
    glib::timeout_add_local(INTERVAL, || {
        let running = RESIDENT.with(|resident| {
            let mut resident = resident.borrow_mut();
            let Some(resident) = resident.as_mut() else {
                return false;
            };
            resident.source.poll();
            if matches!(
                resident.source.status(),
                CheckStatus::Idle | CheckStatus::UpToDate | CheckStatus::Error { .. }
            ) && resident.source.check_automatically()
            {
                resident.manual = false;
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
    if let Some(notice) = NOTICE.with(|notice| notice.borrow_mut().take()) {
        notice.window.close();
    }
    if let Some(mut resident) = RESIDENT.with(|resident| resident.borrow_mut().take()) {
        resident.source.shutdown();
    }
}

/// A capture or editor is open: Update now closes or reopens it.
fn will_close(host: &Host) -> bool {
    capture_active(host).is_some()
        || !host.editors.borrow().is_empty()
        || !host.recordings.borrow().is_empty()
}

impl Resident {
    fn flags(&self, host: &Host) -> NoticeFlags {
        NoticeFlags {
            installing: self.installing,
            // Only an available update depends on the package kind.
            installable: !matches!(self.source.status(), CheckStatus::Available { .. })
                || self.source.installable(),
            will_close: will_close(host),
            manual: self.manual,
        }
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
        let flags = resident.flags(&host);
        Some(card(
            resident.source.current_version(),
            resident.source.status(),
            flags.installing,
            flags.installable,
            flags.will_close,
        ))
    })
}

/// The tray's Check for Updates, as shipping's `handle_tray_action`: an
/// update in hand shows the notice; otherwise check now and report the
/// result (the notice for an update, else a dialog).
pub fn check_from_tray() {
    let check = RESIDENT.with(|resident| {
        let mut resident = resident.borrow_mut();
        let resident = resident.as_mut()?;
        let host = resident.host.upgrade()?;
        let flags = resident.flags(&host);
        let status = notice_status(
            resident.source.current_version(),
            resident.source.status(),
            flags,
        );
        Some(resident.schedule.tray(status.as_ref()))
    });
    match check {
        Some(true) => act(Step::Check),
        Some(false) => service(),
        None => {}
    }
}

/// The notice's Update now, View release or Try again.
pub fn act_from_notice() {
    let step = RESIDENT.with(|resident| {
        let resident = resident.borrow();
        let resident = resident.as_ref()?;
        let host = resident.host.upgrade()?;
        notice_step(resident.source.status(), resident.flags(&host))
    });
    if let Some(step) = step {
        act(step);
    }
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
                // A check already running now reports as asked for.
                resident.manual = true;
                resident.source.check();
            }
            Step::Download => resident.installing = resident.source.download(),
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

/// Publish worker progress, hand a verified package to the helper once
/// Update now asked for it, and open or refresh the notice.
fn service() {
    let install = RESIDENT.with(|resident| {
        let mut resident = resident.borrow_mut();
        let resident = resident.as_mut()?;
        resident.source.poll();
        // A failed download or install ends this attempt.
        if matches!(resident.source.status(), CheckStatus::DownloadError { .. }) {
            resident.installing = false;
        }
        Some(resident.installing && matches!(resident.source.status(), CheckStatus::Staged { .. }))
    });
    if install == Some(true) && self::install() {
        return;
    }
    crate::preferences::refresh_updates();
    sync_notice();
}

/// Shipping's notice policy for the latest status: refresh a visible
/// notice in place (never raising or focusing it), or open, defer or report
/// per [`Schedule`].
fn sync_notice() {
    let notice = NOTICE.with(|notice| notice.borrow().clone());
    let Some((host, status, installing, plan, wait)) = RESIDENT.with(|resident| {
        let mut resident = resident.try_borrow_mut().ok()?;
        let resident = resident.as_mut()?;
        let host = resident.host.upgrade()?;
        let flags = resident.flags(&host);
        let status = notice_status(
            resident.source.current_version(),
            resident.source.status(),
            flags,
        );
        let plan = resident.schedule.next(
            status.as_ref(),
            notice.is_some(),
            capture_active(&host).is_some(),
        );
        // One pending recheck at a time while a capture runs.
        let wait = plan == Plan::Defer && !std::mem::replace(&mut resident.deferred, true);
        Some((host, status, flags.installing, plan, wait))
    }) else {
        return;
    };
    if let (Some(notice), Some(status)) = (&notice, &status) {
        notice.set_live(status.clone(), installing);
    }
    match plan {
        Plan::Ignore => {}
        Plan::Show => {
            if let Some(status) = status {
                open_notice(&host, status, installing);
            }
        }
        Plan::Defer => {
            if wait {
                emit("update-notice-deferred", serde_json::json!({}));
                glib::timeout_add_local_once(Duration::from_secs(1), || {
                    RESIDENT.with(|resident| {
                        if let Some(resident) = resident.borrow_mut().as_mut() {
                            resident.deferred = false;
                        }
                    });
                    service();
                });
            }
        }
        Plan::UpToDate => dialog(
            &host,
            "Captures is up to date",
            "You already have the newest available version.",
            gtk::MessageType::Info,
        ),
        Plan::CheckFailed(message) => dialog(
            &host,
            "Could not check for updates",
            &message,
            gtk::MessageType::Error,
        ),
    }
}

/// Open the floating notice without taking focus, as shipping's
/// non-focusable notice window.
fn open_notice(host: &Host, status: UpdateStatus, installing: bool) {
    let notice =
        crate::notices::update_live(&host.app, host.settings_file.clone(), status, installing);
    notice.window.connect_close_request(|window| {
        NOTICE.with(|notice| {
            let mut slot = notice.borrow_mut();
            if slot.as_ref().is_some_and(|open| open.window == *window) {
                slot.take();
            }
        });
        glib::Propagation::Proceed
    });
    notice.window.set_visible(true);
    NOTICE.with(|slot| *slot.borrow_mut() = Some(notice));
}

/// Shipping's tray result dialogs (`show_dialog`).
#[allow(deprecated)] // GtkAlertDialog needs GTK 4.10.
fn dialog(host: &Host, title: &str, message: &str, kind: gtk::MessageType) {
    emit(
        "update-dialog",
        serde_json::json!({"title": title, "message": message}),
    );
    let dialog = gtk::MessageDialog::builder()
        .title("Captures")
        .text(title)
        .secondary_text(message)
        .message_type(kind)
        .buttons(gtk::ButtonsType::Ok)
        .build();
    dialog.set_application(Some(&host.app));
    dialog.connect_response(|dialog, _| dialog.destroy());
    dialog.present();
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
/// relaunches it with the readiness token and this restart intent. Returns
/// whether the app is quitting.
fn install() -> bool {
    let Some(host) =
        RESIDENT.with(|resident| resident.borrow().as_ref().and_then(|r| r.host.upgrade()))
    else {
        return false;
    };
    if let Some(message) = capture_active(&host) {
        RESIDENT.with(|resident| {
            if let Some(resident) = resident.borrow_mut().as_mut() {
                resident.installing = false;
            }
        });
        host.show_error(message);
        return false;
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
        let result = resident.source.launch_install(restart);
        if result.is_err() {
            resident.installing = false;
        }
        result
    });
    match result {
        Ok(()) => {
            emit("update-install", serde_json::json!({}));
            // The notice leaves with the app.
            if let Some(notice) = NOTICE.with(|notice| notice.borrow_mut().take()) {
                notice.close_for_install();
            }
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
            true
        }
        // The worker now reports Update failed with the reason.
        Err(error) => {
            eprintln!("Could not start the update: {error}");
            false
        }
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

    #[test]
    fn live_worker_states_map_to_shipping_notice_statuses() {
        let flags = NoticeFlags {
            installable: true,
            ..NoticeFlags::default()
        };
        let map = |status: &CheckStatus, flags| notice_status("2026.11.1201", status, flags);
        let mut notes = release();
        notes.notes = Some("* Fix it in https://github.com/joswayski/captures/pull/7".into());
        let Some(UpdateStatus::Available {
            display_version,
            current_display_version,
            notes: Some(_),
            installable: true,
            manual_download_url: None,
            download_size: Some(104_000_000),
            will_close_open_captures: true,
            ..
        }) = map(
            &CheckStatus::Available { release: notes },
            NoticeFlags {
                will_close: true,
                ..flags
            },
        )
        else {
            panic!("available");
        };
        assert_eq!(
            (display_version.as_str(), current_display_version.as_str()),
            ("2026.11.13.2", "2026.11.12.1")
        );
        // A `.deb`: View release on the releases page.
        assert!(matches!(
            map(&CheckStatus::Available { release: release() }, NoticeFlags::default()),
            Some(UpdateStatus::Available { installable: false, manual_download_url: Some(url), .. })
                if url == RELEASES_URL
        ));
        assert!(matches!(
            map(
                &CheckStatus::Downloading {
                    release: release(),
                    downloaded: 5,
                    total: 10
                },
                flags
            ),
            Some(UpdateStatus::Downloading {
                downloaded: 5,
                total: Some(10),
                ..
            })
        ));
        for busy in [
            CheckStatus::Verifying { release: release() },
            CheckStatus::Cancelling { release: release() },
        ] {
            assert!(matches!(
                map(&busy, flags),
                Some(UpdateStatus::Downloading { total: None, .. })
            ));
        }
        let staged = CheckStatus::Staged {
            release: release(),
            sha256: String::new(),
        };
        // Handing off: the finished download, not a countdown.
        assert!(matches!(
            map(
                &staged,
                NoticeFlags {
                    installing: true,
                    ..flags
                }
            ),
            Some(UpdateStatus::Downloading {
                downloaded: 104_000_000,
                total: Some(104_000_000),
                ..
            })
        ));
        assert!(matches!(
            map(&staged, flags),
            Some(UpdateStatus::Available {
                installable: true,
                ..
            })
        ));
        assert_eq!(
            notice_step(&staged, flags),
            Some(Step::Install),
            "a held package installs"
        );
        let failed = CheckStatus::DownloadError {
            release: release(),
            message: "bad signature".into(),
        };
        assert!(matches!(
            map(&failed, flags),
            Some(UpdateStatus::Error { retry_install: true, message, .. })
                if message == "Could not install the update: bad signature"
        ));
        assert_eq!(notice_step(&failed, flags), Some(Step::Download));
        // Only a manual check reports progress or failure.
        let error = CheckStatus::Error {
            message: "offline".into(),
        };
        assert!(map(&error, flags).is_none());
        assert!(map(&CheckStatus::Checking, flags).is_none());
        let manual = NoticeFlags {
            manual: true,
            ..flags
        };
        assert!(matches!(
            map(&error, manual),
            Some(UpdateStatus::Error {
                retry_install: false,
                ..
            })
        ));
        assert!(matches!(
            map(&CheckStatus::Checking, manual),
            Some(UpdateStatus::Checking { .. })
        ));
        assert!(map(&CheckStatus::Idle, manual).is_none());
        assert_eq!(
            notice_step(
                &CheckStatus::Available { release: release() },
                NoticeFlags::default()
            ),
            Some(Step::ViewRelease)
        );
        for name in LIVE_FIXTURES {
            assert!(live_fixture(name).is_some(), "{name}");
        }
    }

    #[test]
    fn the_notice_shows_once_per_version_after_captures_and_from_the_tray() {
        let flags = NoticeFlags {
            installable: true,
            ..NoticeFlags::default()
        };
        let status = |check: &CheckStatus| notice_status("2026.11.1201", check, flags);
        let available = status(&CheckStatus::Available { release: release() });
        let up_to_date = status(&CheckStatus::UpToDate);
        let mut schedule = Schedule::default();
        // Nothing until an update is found.
        assert_eq!(
            schedule.next(up_to_date.as_ref(), false, false),
            Plan::Ignore
        );
        // A new version waits for the capture, then shows once.
        assert_eq!(schedule.next(available.as_ref(), false, true), Plan::Defer);
        assert_eq!(schedule.next(available.as_ref(), false, true), Plan::Defer);
        assert_eq!(schedule.next(available.as_ref(), false, false), Plan::Show);
        assert_eq!(schedule.next(available.as_ref(), true, false), Plan::Ignore);
        // Later: the same version does not come back.
        assert_eq!(
            schedule.next(available.as_ref(), false, false),
            Plan::Ignore
        );
        // Up to date resets it; the version shows again if it returns.
        assert_eq!(
            schedule.next(up_to_date.as_ref(), false, false),
            Plan::Ignore
        );
        assert_eq!(schedule.next(available.as_ref(), false, false), Plan::Show);
        // A version no longer available by the time the capture ends stays
        // hidden.
        let mut newer = release();
        newer.version = "2026.11.1401".into();
        let newer = status(&CheckStatus::Available { release: newer });
        assert_eq!(schedule.next(newer.as_ref(), false, true), Plan::Defer);
        assert_eq!(
            schedule.next(up_to_date.as_ref(), false, false),
            Plan::Ignore
        );
        // The tray: an update in hand shows (after a capture) without a check.
        assert!(!schedule.tray(available.as_ref()));
        assert_eq!(schedule.next(available.as_ref(), false, true), Plan::Defer);
        assert_eq!(schedule.next(available.as_ref(), false, false), Plan::Show);
        // Otherwise it checks and reports the result.
        assert!(schedule.tray(up_to_date.as_ref()));
        let checking = notice_status(
            "2026.11.1201",
            &CheckStatus::Checking,
            NoticeFlags {
                manual: true,
                ..flags
            },
        );
        assert_eq!(schedule.next(checking.as_ref(), false, false), Plan::Ignore);
        assert_eq!(
            schedule.next(up_to_date.as_ref(), false, false),
            Plan::UpToDate
        );
        assert!(schedule.tray(None));
        let failed = notice_status(
            "2026.11.1201",
            &CheckStatus::Error {
                message: "offline".into(),
            },
            NoticeFlags {
                manual: true,
                ..flags
            },
        );
        assert_eq!(
            schedule.next(failed.as_ref(), false, false),
            Plan::CheckFailed("offline".into())
        );
        // shipping's `notice_disposition`.
        assert_eq!(disposition(failed.as_ref(), true), Disposition::Show);
        assert_eq!(disposition(available.as_ref(), true), Disposition::Defer);
        assert_eq!(disposition(up_to_date.as_ref(), false), Disposition::Ignore);
        assert_eq!(disposition(None, false), Disposition::Ignore);
    }
}
