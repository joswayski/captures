//! Serialized update checks and acquisition for explicit development endpoints
//! or the compiled installed Preview channel. Construction never issues HTTP.
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
};

use captures_media::CancelToken;
use serde::{Deserialize, Serialize};

use super::{Error, ReleaseInfo, Renderer, Target, UpdateClient};

/// An operator-selected scratch location, never an installation/profile path.
/// Each download and stage owns only randomly named temporary entries inside it.
pub fn validate_staging_directory(path: &Path) -> Result<(), Error> {
    if !path.is_absolute() || !path.symlink_metadata().is_ok_and(|m| m.is_dir()) {
        return Err(Error::Configuration(
            "staging directory must be an existing absolute directory, not a symlink",
        ));
    }
    Ok(())
}

/// Both native hosts pin an explicitly supplied, bounded Minisign public key.
/// This reads only the key file; construction performs no network request.
pub fn client_from_key_file(
    endpoint: &str,
    key_file: &Path,
    renderer: Renderer,
    current_version: &str,
) -> Result<UpdateClient, Error> {
    const LIMIT: u64 = 8 * 1024;
    let invalid_key = || {
        Error::Configuration("public key must be a readable regular UTF-8 file of at most 8 KiB")
    };
    if !key_file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(invalid_key());
    }
    let file = std::fs::File::open(key_file).map_err(|_| invalid_key())?;
    let mut key = String::new();
    file.take(LIMIT + 1)
        .read_to_string(&mut key)
        .map_err(|_| invalid_key())?;
    if key.len() as u64 > LIMIT {
        return Err(invalid_key());
    }
    UpdateClient::new(
        endpoint,
        &key,
        renderer,
        Target::current_host().ok_or(Error::Configuration("unsupported native update host"))?,
        current_version,
    )
}

/// Temporary acquisition is separate from installation; no state can install.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckStatus {
    Idle,
    Checking,
    UpToDate,
    Available {
        release: ReleaseInfo,
    },
    Error {
        message: String,
    },
    Downloading {
        release: ReleaseInfo,
        downloaded: u64,
        total: u64,
    },
    Verifying {
        release: ReleaseInfo,
    },
    Cancelling {
        release: ReleaseInfo,
    },
    Staged {
        release: ReleaseInfo,
        sha256: String,
    },
    DownloadError {
        release: ReleaseInfo,
        message: String,
    },
}

/// UI copy shared by AppKit and wgpu. A new worker stays idle until Check Now.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Presentation {
    pub version: String,
    pub channel: &'static str,
    pub status: String,
    pub action: &'static str,
    pub enabled: bool,
    pub failed: bool,
    pub detail: &'static str,
    pub acquisition: Option<crate::update_notice::Button>,
    /// The tray or menu-bar update row.
    pub tray: TrayItem,
    /// An installable release build's Preferences card, as shipping
    /// `UpdatePreferences` draws it: one action button whose step checks,
    /// downloads and installs (Update now), with progress and errors.
    pub card: Option<PreviewCard>,
}

/// What a [`PreviewCard`]'s button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardStep {
    Check,
    /// Download and verify, then install (Update now, Try again).
    Download,
    /// Install the already verified package.
    Install,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PreviewCard {
    pub description: &'static str,
    pub action: &'static str,
    /// `None` while the button is disabled.
    pub step: Option<CardStep>,
    /// `.update-settings-status`: the heading, then its `small` detail.
    pub heading: String,
    pub detail: String,
    /// `.update-settings-status-current`: positive text.
    pub current: bool,
    /// The installing progress bar; `progress` is `None` while indeterminate.
    pub progress_bar: bool,
    pub progress: Option<f64>,
    pub error: Option<String>,
    pub fallback: &'static str,
}

/// Shipping `tray_update_item` and `tray_tooltip`: an available or installing
/// update is pinned to the top of the tray menu with its state as the label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrayItem {
    pub label: String,
    pub enabled: bool,
    pub pin_first: bool,
    pub tooltip: &'static str,
}

enum Job {
    Check(CancelToken),
    Download(CancelToken),
    /// Rechecks behind an Available or Staged update without showing a check.
    Refresh(CancelToken),
    TakePreview(mpsc::Sender<Option<super::preview::Download>>),
    Shutdown,
}

enum Event {
    Finished(CheckStatus),
    /// A newer release replaced the one an Available or Staged update offered.
    Refreshed(ReleaseInfo),
    /// The download started on a newer release than the one it was asked for.
    Retarget(ReleaseInfo),
    Progress(u64, u64),
    Verifying,
}

enum Source {
    Development(UpdateClient),
    Preview(super::preview::Client),
}

enum Pending {
    Development(super::PendingUpdate),
    Preview(super::preview::Pending),
}

enum Stage {
    Development(super::StagedUpdate),
    Preview(super::preview::Download),
}

impl Source {
    fn check(&self, token: &CancelToken) -> Result<Option<Pending>, Error> {
        match self {
            Self::Development(client) => client
                .check(token)
                .map(|update| update.map(Pending::Development)),
            Self::Preview(client) => client
                .check(token)
                .map(|update| update.map(Pending::Preview)),
        }
    }
}

impl Pending {
    fn info(&self) -> &ReleaseInfo {
        match self {
            Self::Development(update) => update.info(),
            Self::Preview(update) => &update.info,
        }
    }

    fn stage(
        &self,
        directory: &Path,
        token: &CancelToken,
        progress: impl FnMut(u64, u64),
        verifying: impl FnOnce(),
    ) -> Result<Stage, Error> {
        match self {
            Self::Development(update) => update
                .stage(directory, token, progress, verifying)
                .map(Stage::Development),
            Self::Preview(update) => update
                .download_verifying(directory, token, progress, verifying)
                .map(Stage::Preview),
        }
    }
}

impl Stage {
    fn sha256(&self) -> String {
        match self {
            Self::Development(package) => package.sha256(),
            Self::Preview(package) => package.sha256.clone(),
        }
    }
}

pub struct CheckWorker {
    current_version: String,
    status: CheckStatus,
    generation: u64,
    staging_enabled: bool,
    install_request_enabled: bool,
    /// A refresh replaced a Staged update with a newer Available one; the
    /// worker still holds the verified package for a Restart already chosen.
    superseded_stage: bool,
    preview: bool,
    automatic_check: bool,
    jobs: mpsc::Sender<Job>,
    results: mpsc::Receiver<Event>,
    operation_cancel: Option<CancelToken>,
    cancel: CancelToken,
    thread: Option<JoinHandle<()>>,
}

impl CheckWorker {
    /// No request at construction. Without an explicit scratch directory this
    /// remains check-only. Authenticated capabilities and staged owners stay on
    /// the worker thread; paths are never serialized into UI replies.
    pub fn new(
        client: UpdateClient,
        wake: Arc<dyn Fn() + Send + Sync>,
        staging_directory: Option<PathBuf>,
    ) -> Result<Self, Error> {
        Self::with_source(Source::Development(client), wake, staging_directory)
    }

    pub fn preview(renderer: Renderer, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, Error> {
        let client = super::preview::Client::installed(renderer)?;
        let installable = client.installable();
        let directory = crate::preview_channel::profile_path()
            .map_err(|_| Error::Configuration("Captures profile is unavailable"))?
            .join("updates");
        std::fs::create_dir_all(&directory)?;
        let mut worker = Self::with_source(
            Source::Preview(client),
            wake,
            installable.then_some(directory),
        )?;
        worker.install_request_enabled = installable;
        Ok(worker)
    }

    fn with_source(
        client: Source,
        wake: Arc<dyn Fn() + Send + Sync>,
        staging_directory: Option<PathBuf>,
    ) -> Result<Self, Error> {
        if let Some(path) = &staging_directory {
            validate_staging_directory(path)?;
        }
        let current_version = match &client {
            Source::Development(client) => client.current_version.to_string(),
            Source::Preview(client) => client.current_version(),
        };
        let preview = matches!(client, Source::Preview(_));
        let staging_enabled = staging_directory.is_some();
        let (jobs, requests) = mpsc::channel();
        let (finished, results) = mpsc::channel();
        let cancel = CancelToken::default();
        let cancelled = cancel.clone();
        let thread = thread::spawn(move || {
            let mut pending = None;
            let mut staged = None;
            while let Ok(job) = requests.recv() {
                if cancelled.is_cancelled() {
                    break;
                }
                let status = match job {
                    Job::Shutdown => break,
                    Job::TakePreview(reply) => {
                        let value = match staged.take() {
                            Some(Stage::Preview(package)) => Some(package),
                            other => {
                                staged = other;
                                None
                            }
                        };
                        let _ = reply.send(value);
                        continue;
                    }
                    Job::Check(token) => {
                        // Drop verified temporary storage before rechecking.
                        staged.take();
                        pending = None;
                        match client.check(&token) {
                            Ok(Some(update)) => {
                                let release = update.info().clone();
                                pending = Some(update);
                                CheckStatus::Available { release }
                            }
                            Ok(None) => CheckStatus::UpToDate,
                            Err(error) => CheckStatus::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    Job::Refresh(token) => {
                        // Only a newer release changes anything; failures and
                        // the same release keep the offer the user is reading.
                        let Ok(Some(update)) = client.check(&token) else {
                            continue;
                        };
                        if pending
                            .as_ref()
                            .is_some_and(|known| known.info().version == update.info().version)
                        {
                            continue;
                        }
                        // Keep a package staged for the older release until a
                        // download replaces it: a Restart chosen just before
                        // this result reached the UI still installs it rather
                        // than failing.
                        let release = update.info().clone();
                        pending = Some(update);
                        if cancelled.is_cancelled()
                            || finished.send(Event::Refreshed(release)).is_err()
                        {
                            break;
                        }
                        wake();
                        continue;
                    }
                    Job::Download(token) => {
                        if pending.is_none() || staging_directory.is_none() {
                            continue;
                        }
                        // A package a refresh superseded is never installed now.
                        staged.take();
                        // An offer can sit open for hours while releases land.
                        // Download what the feed offers now, not what it offered
                        // then, or the restart lands on an already old version.
                        match client.check(&token) {
                            Ok(Some(latest)) => {
                                if pending.as_ref().is_some_and(|known| {
                                    known.info().version != latest.info().version
                                }) {
                                    let release = latest.info().clone();
                                    pending = Some(latest);
                                    if !cancelled.is_cancelled() && !token.is_cancelled() {
                                        let _ = finished.send(Event::Retarget(release));
                                        wake();
                                    }
                                }
                            }
                            Ok(None) => {
                                pending = None;
                                if cancelled.is_cancelled()
                                    || finished
                                        .send(Event::Finished(CheckStatus::UpToDate))
                                        .is_err()
                                {
                                    break;
                                }
                                wake();
                                continue;
                            }
                            // A failed or cancelled recheck leaves the known
                            // release to the download, which reports its own
                            // failure or cancellation as before.
                            Err(_) => {}
                        }
                        let (Some(update), Some(directory)) = (&pending, &staging_directory) else {
                            continue;
                        };
                        let release = update.info().clone();
                        let result = update.stage(
                            directory,
                            &token,
                            |received, total| {
                                if !cancelled.is_cancelled() && !token.is_cancelled() {
                                    let _ = finished.send(Event::Progress(received, total));
                                    wake();
                                }
                            },
                            || {
                                if !cancelled.is_cancelled() && !token.is_cancelled() {
                                    let _ = finished.send(Event::Verifying);
                                    wake();
                                }
                            },
                        );
                        if token.is_cancelled() {
                            // Cleanup precedes the cancellation acknowledgement.
                            drop(result);
                            CheckStatus::Available { release }
                        } else {
                            match result {
                                Ok(package) => {
                                    let sha256 = package.sha256();
                                    staged = Some(package);
                                    CheckStatus::Staged { release, sha256 }
                                }
                                Err(error) => CheckStatus::DownloadError {
                                    release,
                                    message: match error {
                                        Error::Io(_) => {
                                            "Native update file operation failed.".into()
                                        }
                                        _ => error.to_string(),
                                    },
                                },
                            }
                        }
                    }
                };
                if cancelled.is_cancelled() {
                    break;
                }
                if finished.send(Event::Finished(status)).is_err() {
                    break;
                }
                wake();
            }
        });
        Ok(Self {
            current_version,
            status: CheckStatus::Idle,
            generation: 0,
            staging_enabled,
            install_request_enabled: false,
            superseded_stage: false,
            preview,
            automatic_check: false,
            jobs,
            results,
            operation_cancel: None,
            cancel,
            thread: Some(thread),
        })
    }

    pub fn is_preview(&self) -> bool {
        self.preview
    }

    /// Call after the host's editor/recording veto, before shutting this worker
    /// down. The copied helper cannot install until every package-use guard exits.
    pub fn launch_preview_install(&mut self, restart: super::RestartIntent) -> Result<(), Error> {
        if !self.preview
            || !self.install_request_enabled
            || !(matches!(self.status, CheckStatus::Staged { .. })
                || self.superseded_stage && matches!(self.status, CheckStatus::Available { .. }))
        {
            return Err(Error::Installation("a verified update is not available"));
        }
        let (sender, receiver) = mpsc::channel();
        self.jobs
            .send(Job::TakePreview(sender))
            .map_err(|_| Error::Installation("update worker stopped"))?;
        let package = receiver
            .recv()
            .map_err(|_| Error::Installation("update worker stopped"))?
            .ok_or(Error::Installation(
                "verified update package is no longer available",
            ))?;
        self.superseded_stage = false;
        if let Err(error) = package.launch(restart) {
            if let CheckStatus::Staged { release, .. } = &self.status {
                self.status = CheckStatus::DownloadError {
                    release: release.clone(),
                    message: error.to_string(),
                };
            }
            return Err(error);
        }
        Ok(())
    }

    /// Development hosts enable this only under the explicit supervisor.
    /// Installed Previews enable it when their package supports replacement.
    pub fn enable_install_request(&mut self) {
        self.install_request_enabled = self.staging_enabled;
    }

    /// Repeated input during any operation cannot enqueue duplicate requests.
    pub fn check(&mut self) -> bool {
        self.poll();
        if self.cancel.is_cancelled() || self.checking() || self.thread.is_none() {
            return false;
        }
        self.automatic_check = false;
        self.status = CheckStatus::Checking;
        let token = CancelToken::default();
        self.operation_cancel = Some(token.clone());
        let accepted = self.jobs.send(Job::Check(token)).is_ok();
        if accepted {
            // A check or download drops any superseded package.
            self.superseded_stage = false;
            self.generation += 1;
        }
        accepted
    }

    /// Background checks only raise a notice when an update is available.
    /// Behind an Available or Staged update they look for a newer release
    /// instead, so an offer left open while releases land stays current;
    /// they leave downloads and failed downloads alone.
    pub fn check_automatically(&mut self) -> bool {
        self.poll();
        match self.status {
            CheckStatus::Idle | CheckStatus::UpToDate | CheckStatus::Error { .. } => {}
            CheckStatus::Available { .. } | CheckStatus::Staged { .. } => {
                return !self.cancel.is_cancelled()
                    && self.thread.is_some()
                    && self.jobs.send(Job::Refresh(CancelToken::default())).is_ok();
            }
            _ => return false,
        }
        let accepted = self.check();
        if accepted {
            self.automatic_check = true;
        }
        accepted
    }

    pub fn download(&mut self) -> bool {
        self.poll();
        if self.cancel.is_cancelled() || !self.staging_enabled || self.thread.is_none() {
            return false;
        }
        let release = match &self.status {
            CheckStatus::Available { release } | CheckStatus::DownloadError { release, .. } => {
                release.clone()
            }
            _ => return false,
        };
        self.automatic_check = false;
        let token = CancelToken::default();
        self.operation_cancel = Some(token.clone());
        let accepted = self.jobs.send(Job::Download(token)).is_ok();
        if accepted {
            // A check or download drops any superseded package.
            self.superseded_stage = false;
            self.generation += 1;
            self.status = CheckStatus::Downloading {
                total: release.size,
                release,
                downloaded: 0,
            };
        }
        accepted
    }

    /// Do not unpin busy state until the worker has finished I/O and cleanup.
    pub fn cancel_download(&mut self) -> bool {
        self.poll();
        if self.cancel.is_cancelled() {
            return false;
        }
        let release = match &self.status {
            CheckStatus::Downloading { release, .. } | CheckStatus::Verifying { release } => {
                release.clone()
            }
            _ => return false,
        };
        self.operation_cancel.as_ref().unwrap().cancel();
        self.status = CheckStatus::Cancelling { release };
        true
    }

    /// New explicit checks/downloads reveal; progress/results/cancellation do not.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn checking(&self) -> bool {
        matches!(
            self.status,
            CheckStatus::Checking
                | CheckStatus::Downloading { .. }
                | CheckStatus::Verifying { .. }
                | CheckStatus::Cancelling { .. }
        )
    }

    /// Nonblocking publication, including while Preferences is closed.
    pub fn poll(&mut self) -> bool {
        if self.cancel.is_cancelled() || self.thread.is_none() {
            return false;
        }
        let mut changed = false;
        while let Ok(event) = self.results.try_recv() {
            match event {
                Event::Finished(status) => {
                    self.status = status;
                    self.operation_cancel = None;
                }
                // A Download or Check sent after this refresh owns the status.
                Event::Refreshed(release) => {
                    if !matches!(
                        self.status,
                        CheckStatus::Available { .. } | CheckStatus::Staged { .. }
                    ) {
                        continue;
                    }
                    self.superseded_stage |= matches!(self.status, CheckStatus::Staged { .. });
                    self.status = CheckStatus::Available { release };
                }
                Event::Retarget(next) => {
                    let CheckStatus::Downloading {
                        release,
                        downloaded,
                        total,
                    } = &mut self.status
                    else {
                        continue;
                    };
                    *total = next.size;
                    *downloaded = 0;
                    *release = next;
                }
                Event::Progress(downloaded, total) => {
                    let CheckStatus::Downloading {
                        downloaded: current,
                        total: current_total,
                        ..
                    } = &mut self.status
                    else {
                        continue;
                    };
                    *current = downloaded;
                    *current_total = total;
                }
                Event::Verifying => {
                    let CheckStatus::Downloading { release, .. } = &self.status else {
                        continue;
                    };
                    self.status = CheckStatus::Verifying {
                        release: release.clone(),
                    };
                }
            }
            changed = true;
        }
        changed
    }

    pub fn status(&self) -> &CheckStatus {
        &self.status
    }

    /// Reuse shipping notice layout/notes without presenting an install, stable
    /// download-page link or simulated restart. `open_captures` (a capture or
    /// editor is open) adds the close-captures warning to an update that
    /// restarts Captures, as shipping's `will_close_open_captures`.
    pub fn notice(
        &self,
        show_changelog: bool,
        open_captures: bool,
    ) -> Option<crate::update_notice::Presentation> {
        use crate::update_notice::{self as notice, Action, Button, UpdateStatus, ViewState};
        if self.automatic_check && !matches!(self.status, CheckStatus::Available { .. }) {
            return None;
        }
        let will_close = open_captures && self.install_request_enabled;
        let current_version = self.current_version.clone();
        let current_display_version = current_version.clone();
        let status = match &self.status {
            CheckStatus::Idle => return None,
            CheckStatus::Checking => UpdateStatus::Checking {
                current_version,
                current_display_version,
            },
            CheckStatus::UpToDate => UpdateStatus::UpToDate {
                current_version,
                current_display_version,
            },
            CheckStatus::Available { release } => UpdateStatus::Available {
                current_version,
                current_display_version,
                version: release.version.clone(),
                display_version: super::display_version(&release.version),
                notes: release.notes.clone(),
                changelog: release.changelog.clone(),
                installable: false,
                manual_download_url: None,
                download_size: Some(release.size),
                will_close_open_captures: will_close,
            },
            CheckStatus::Downloading {
                release,
                downloaded,
                total,
            } => UpdateStatus::Downloading {
                current_version,
                current_display_version,
                version: release.version.clone(),
                display_version: release.version.clone(),
                downloaded: *downloaded,
                total: Some(*total),
            },
            CheckStatus::Verifying { .. } | CheckStatus::Cancelling { .. } => {
                UpdateStatus::Checking {
                    current_version,
                    current_display_version,
                }
            }
            CheckStatus::Staged { .. } => UpdateStatus::UpToDate {
                current_version,
                current_display_version,
            },
            CheckStatus::Error { message } | CheckStatus::DownloadError { message, .. } => {
                UpdateStatus::Error {
                    current_version,
                    current_display_version,
                    message: message.clone(),
                    retry_install: false,
                }
            }
        };
        let mut p = notice::present(
            Some(&status),
            &ViewState {
                show_changelog,
                ..Default::default()
            },
        );
        p.title = match self.status {
            CheckStatus::Checking => "Checking native updates",
            CheckStatus::Available { .. } => "Native development update",
            CheckStatus::UpToDate => "Native development up to date",
            CheckStatus::Downloading { .. } => "Downloading development update",
            CheckStatus::Verifying { .. } => "Verifying development package",
            CheckStatus::Cancelling { .. } => "Cancelling development download",
            CheckStatus::Staged { .. } => "Development package verified",
            CheckStatus::DownloadError { .. } => "Development download failed",
            _ => "Native development check failed",
        }
        .into();
        p.description = match &self.status {
            CheckStatus::Staged { release, .. } if self.install_request_enabled => {
                format!("{} · Supervised development update", release.version)
            }
            CheckStatus::Available { release }
            | CheckStatus::Downloading { release, .. }
            | CheckStatus::Verifying { release }
            | CheckStatus::Cancelling { release }
            | CheckStatus::Staged { release, .. }
            | CheckStatus::DownloadError { release, .. } => {
                format!(
                    "{} · {}; no installation",
                    release.version,
                    if self.staging_enabled {
                        "Temporary verification"
                    } else {
                        "Check only"
                    }
                )
            }
            _ => format!("Current {} · Check only", self.current_version),
        };
        if let Some(error) = &mut p.error {
            error.fallback_prefix = if matches!(self.status, CheckStatus::DownloadError { .. }) {
                "Retry this authenticated download below."
            } else {
                "Retry this development check below."
            }
            .into();
            error.fallback_link.clear();
            error.fallback_suffix.clear();
        }
        p.dismiss_blocked = false;
        let footer = p.footer.get_or_insert_with(|| notice::Footer {
            dismiss: Button {
                label: "Close".into(),
                enabled: true,
                action: Action::Dismiss,
                trailing_icon: None,
            },
            primary: None,
        });
        footer.primary = self.acquisition().or_else(|| {
            (!self.checking()).then(|| Button {
                label: if p.error.is_some() {
                    "Try again"
                } else {
                    "Check again"
                }
                .into(),
                enabled: self.thread.is_some(),
                action: Action::Check,
                trailing_icon: None,
            })
        });
        p.status_message = match self.status {
            CheckStatus::Verifying { .. } => Some("Validating the signed package in temporary storage…".into()),
            CheckStatus::Cancelling { .. } => Some("Waiting for pending I/O and temporary cleanup…".into()),
            CheckStatus::Staged { .. } if self.install_request_enabled => Some("Restart installs this development update after capture work and drafts finish. The previous package and a data snapshot stay available until startup is confirmed.".into()),
            CheckStatus::Staged { .. } => Some("Package verified in temporary storage. Nothing installed or executed. Recheck or quit removes it.".into()),
            _ => p.status_message,
        };
        if matches!(self.status, CheckStatus::Staged { .. }) {
            p.icon = notice::Icon::Check;
            p.icon_tone = notice::IconTone::Positive;
        }
        // The installed update restarts Captures and reopens what was open;
        // with nothing open there is nothing to warn about.
        if will_close
            && matches!(
                self.status,
                CheckStatus::Available { .. } | CheckStatus::Staged { .. }
            )
        {
            p.close_warning = Some(notice::open_captures_warning(true).into());
        }
        // Development descriptions name raw versions without a release page.
        p.version_link = None;
        // A staged update's two-line warning, in place of the status line.
        let staged_height = notice::card_height(None, false) - notice::STATUS_HEIGHT
            + notice::WARNING_HEIGHT
            + 16.0;
        if self.preview {
            p.title = match self.status {
                CheckStatus::Checking => "Checking for updates",
                CheckStatus::UpToDate => "Captures is up to date",
                CheckStatus::Downloading { .. } => "Downloading update",
                CheckStatus::Staged { .. } => "Update ready to install",
                CheckStatus::Error { .. } | CheckStatus::DownloadError { .. } => "Update failed",
                _ => "Captures update",
            }
            .into();
            p.description = self.presentation().status;
            // Shipping's `Version … · 12.6 MB`: what Download will fetch.
            if let CheckStatus::Available { release } = &self.status
                && release.size > 0
            {
                p.description = format!(
                    "{} · {}",
                    p.description,
                    notice::format_update_size(release.size)
                );
            }
            p.version_link = match &self.status {
                CheckStatus::Available { release } | CheckStatus::Staged { release, .. } => {
                    notice::version_link(&super::display_version(&release.version))
                }
                _ => None,
            };
            // A staged update says what Restart does in its close warning;
            // a second line saying the same thing was redundant.
            p.status_message = None;
            if p.close_warning.is_some() && matches!(self.status, CheckStatus::Staged { .. }) {
                p.card_height = staged_height;
            }
            if let Some(error) = &mut p.error {
                error.fallback_prefix = "Try again, or ".into();
                error.fallback_link = "download from captur.es".into();
                error.fallback_suffix = ".".into();
            }
            if !self.staging_enabled
                && matches!(self.status, CheckStatus::Available { .. })
                && let Some(footer) = &mut p.footer
            {
                // Opens captur.es in the browser: a trailing link icon.
                footer.primary = Some(Button {
                    label: "Download update".into(),
                    enabled: true,
                    action: Action::OpenDownloadPage,
                    trailing_icon: Some("external-link".into()),
                });
            }
        }
        // Every step of one update shares the tallest of its Available,
        // progress and Restart and install cards, so the notice does not jump
        // as one replaces another (an error may still grow it).
        if let CheckStatus::Available { release }
        | CheckStatus::Downloading { release, .. }
        | CheckStatus::Verifying { release }
        | CheckStatus::Cancelling { release }
        | CheckStatus::Staged { release, .. }
        | CheckStatus::DownloadError { release, .. } = &self.status
        {
            let available = UpdateStatus::Available {
                current_version: self.current_version.clone(),
                current_display_version: self.current_version.clone(),
                version: release.version.clone(),
                display_version: super::display_version(&release.version),
                notes: release.notes.clone(),
                changelog: release.changelog.clone(),
                installable: false,
                manual_download_url: None,
                download_size: Some(release.size),
                will_close_open_captures: will_close,
            };
            let staged = if self.preview && self.install_request_enabled {
                staged_height
            } else {
                0.0
            };
            p.card_height = p
                .card_height
                .max(notice::card_height(Some(&available), show_changelog))
                .max(notice::card_height(None, false))
                .max(staged);
        }
        Some(p)
    }

    fn acquisition(&self) -> Option<crate::update_notice::Button> {
        use crate::update_notice::{Action, Button};
        if !self.staging_enabled || self.thread.is_none() {
            return None;
        }
        let (label, enabled, action) = match self.status {
            CheckStatus::Available { .. } => ("Download", true, Action::DownloadVerify),
            CheckStatus::DownloadError { .. } => ("Retry download", true, Action::DownloadVerify),
            CheckStatus::Downloading { .. } | CheckStatus::Verifying { .. } => {
                ("Cancel download", true, Action::CancelDownload)
            }
            CheckStatus::Cancelling { .. } => ("Cancelling…", false, Action::CancelDownload),
            CheckStatus::Staged { .. } if self.install_request_enabled => {
                ("Restart and install", true, Action::Install)
            }
            _ => return None,
        };
        Some(Button {
            label: label.into(),
            enabled,
            action,
            trailing_icon: None,
        })
    }

    /// The tray row from the shared `update_notice::tray_update_item`. A
    /// verified package still waits for its install, so it reads as available;
    /// verification is part of installing.
    fn tray_item(&self) -> TrayItem {
        use crate::update_notice::{UpdateStatus, tray_tooltip, tray_update_item};
        let current_version = self.current_version.clone();
        let current_display_version = super::display_version(&current_version);
        let status = match &self.status {
            CheckStatus::Available { release } | CheckStatus::Staged { release, .. } => {
                Some(UpdateStatus::Available {
                    current_version,
                    current_display_version,
                    version: release.version.clone(),
                    display_version: super::display_version(&release.version),
                    notes: release.notes.clone(),
                    changelog: release.changelog.clone(),
                    installable: self.staging_enabled,
                    manual_download_url: None,
                    download_size: Some(release.size),
                    will_close_open_captures: false,
                })
            }
            CheckStatus::Downloading { release, .. }
            | CheckStatus::Verifying { release }
            | CheckStatus::Cancelling { release } => Some(UpdateStatus::Downloading {
                current_version,
                current_display_version,
                version: release.version.clone(),
                display_version: super::display_version(&release.version),
                downloaded: 0,
                total: None,
            }),
            CheckStatus::Checking => Some(UpdateStatus::Checking {
                current_version,
                current_display_version,
            }),
            _ => None,
        };
        let item = tray_update_item(status.as_ref());
        TrayItem {
            label: item.label,
            enabled: item.enabled && (item.pin_first || self.thread.is_some()),
            pin_first: item.pin_first,
            tooltip: tray_tooltip(status.as_ref()),
        }
    }

    fn preview_card(&self) -> PreviewCard {
        use crate::update_notice::format_update_size;
        let mut card = PreviewCard {
            description: "Captures checks for a new version automatically. Update now installs it in place.",
            action: "Check Now",
            step: Some(CardStep::Check),
            heading: String::new(),
            detail: String::new(),
            current: false,
            progress_bar: false,
            progress: None,
            error: None,
            fallback: "If this copy cannot update itself, download from captur.es and install over it. Settings and captures stay.",
        };
        let version = |release: &ReleaseInfo| super::display_version(&release.version);
        match &self.status {
            CheckStatus::Idle => {}
            CheckStatus::Checking => {
                card.action = "Checking now…";
                card.step = None;
            }
            CheckStatus::UpToDate => {
                card.heading = "Up to date".into();
                card.current = true;
            }
            CheckStatus::Available { release } | CheckStatus::Staged { release, .. } => {
                card.heading = format!("Version {} is available", version(release));
                if release.size > 0 && matches!(self.status, CheckStatus::Available { .. }) {
                    card.detail = format!("{} download", format_update_size(release.size));
                }
                card.action = "Update now";
                card.step = Some(if matches!(self.status, CheckStatus::Staged { .. }) {
                    CardStep::Install
                } else {
                    CardStep::Download
                });
            }
            CheckStatus::Downloading {
                release,
                downloaded,
                total,
            } => {
                card.heading = format!("Updating to version {}", version(release));
                card.action = "Installing…";
                card.step = None;
                card.progress_bar = true;
                card.progress = (*total > 0).then(|| {
                    ((*downloaded as f64 / *total as f64) * 100.)
                        .round()
                        .min(100.)
                });
                card.detail = match card.progress {
                    Some(percent) => format!(
                        "{} / {} · {percent}%",
                        format_update_size(*downloaded),
                        format_update_size(*total)
                    ),
                    None => format!("{} downloaded", format_update_size(*downloaded)),
                };
            }
            CheckStatus::Verifying { release } | CheckStatus::Cancelling { release } => {
                card.heading = format!("Updating to version {}", version(release));
                card.action = "Installing…";
                card.step = None;
                card.progress_bar = true;
            }
            CheckStatus::Error { message } => {
                card.heading = "Couldn’t check for updates".into();
                card.error = Some(message.clone());
            }
            CheckStatus::DownloadError { message, .. } => {
                card.heading = "Update failed".into();
                card.action = "Try again";
                card.step = Some(CardStep::Download);
                card.error = Some(format!("Could not install the update: {message}"));
            }
        }
        if self.thread.is_none() {
            card.step = None;
        }
        card
    }

    pub fn presentation(&self) -> Presentation {
        let status = match &self.status {
            CheckStatus::Idle => "Not checked".into(),
            CheckStatus::Checking => "Checking signed metadata…".into(),
            CheckStatus::UpToDate => "No development updates available".into(),
            CheckStatus::Available { release } => {
                format!("Development update {} available", release.version)
            }
            CheckStatus::Downloading {
                downloaded, total, ..
            } => format!("Downloading {downloaded} of {total} bytes"),
            CheckStatus::Verifying { .. } => "Verifying package…".into(),
            CheckStatus::Cancelling { .. } => "Cancelling; waiting for I/O…".into(),
            CheckStatus::Staged { release, .. } => {
                format!("Development package {} verified", release.version)
            }
            CheckStatus::Error { message } | CheckStatus::DownloadError { message, .. } => {
                message.clone()
            }
        };
        let mut presentation = Presentation {
            version: format!("Native development {}", self.current_version),
            channel: if self.install_request_enabled {
                "Explicit development endpoint · Supervised restart"
            } else if self.staging_enabled {
                "Explicit development endpoint · Temporary verification"
            } else {
                "Explicit development endpoint · Check only"
            },
            status,
            action: if self.checking() {
                "Checking…"
            } else {
                "Check Now"
            },
            enabled: !self.checking() && self.thread.is_some(),
            failed: matches!(
                self.status,
                CheckStatus::Error { .. } | CheckStatus::DownloadError { .. }
            ),
            detail: if self.install_request_enabled {
                "Only Restart and install requests replacement of the selected development package. Ordinary Quit does not install. Installed apps and release channels are unchanged."
            } else if self.staging_enabled {
                "Explicit downloads validate a temporary package. No installation, execution or update channel is enabled."
            } else {
                "Verifies signed native development metadata. No download, installation or update channel is enabled."
            },
            acquisition: self.acquisition(),
            tray: self.tray_item(),
            card: (self.preview && self.staging_enabled).then(|| self.preview_card()),
        };
        if self.preview {
            // Shipping's Preferences identity, like the Win32/GTK cards.
            presentation.version =
                format!("Version {}", super::display_version(&self.current_version));
            presentation.channel = "Release channel";
            presentation.status = match &self.status {
                CheckStatus::UpToDate => "You have the latest version".into(),
                CheckStatus::Available { release } => {
                    format!(
                        "Version {} available",
                        super::display_version(&release.version)
                    )
                }
                CheckStatus::Staged { release, .. } => {
                    format!(
                        "Version {} verified",
                        super::display_version(&release.version)
                    )
                }
                CheckStatus::Checking => "Checking for updates…".into(),
                _ => presentation.status,
            };
            presentation.detail = if self.staging_enabled {
                "Signed updates. Restart and install closes open captures after preserving drafts. Ordinary Quit does not install."
            } else {
                "Debian packages use manual updates. Download the latest version from captur.es."
            };
        }
        presentation
    }

    fn begin_shutdown(&mut self) {
        if self.cancel.is_cancelled() {
            return;
        }
        self.cancel.cancel();
        if let Some(token) = self.operation_cancel.take() {
            token.cancel();
        }
        let _ = self.jobs.send(Job::Shutdown);
    }

    /// Cancel immediately, but join only after I/O and owned-stage cleanup finish.
    /// A UI host must retain its profile election while this returns false.
    pub fn try_shutdown(&mut self) -> bool {
        self.begin_shutdown();
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            return false;
        }
        self.shutdown();
        true
    }

    /// Blocking fallback for Drop and hosts that drain off their UI thread.
    /// A currently blocked request can take up to its 60-second timeout.
    pub fn shutdown(&mut self) {
        self.begin_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CheckWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::{
        Renderer, Target,
        tests::{manifest, serve, signed},
    };
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };

    fn worker(endpoint: &str, key: &str, current: &str) -> (CheckWorker, mpsc::Receiver<()>) {
        let client =
            UpdateClient::new(endpoint, key, Renderer::Wgpu, Target::LinuxX64, current).unwrap();
        let (wake, observed) = mpsc::channel();
        (
            CheckWorker::new(
                client,
                Arc::new(move || {
                    let _ = wake.send(());
                }),
                None,
            )
            .unwrap(),
            observed,
        )
    }

    fn wait_idle(worker: &mut CheckWorker) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            worker.poll();
            if !worker.checking() {
                return;
            }
            assert!(Instant::now() < deadline, "worker never finished");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn signed_package(base: &str, body: &[u8]) -> (String, Vec<u8>, Vec<u8>) {
        use sha2::{Digest, Sha256};
        let mut value = manifest(&format!("{base}/artifact"));
        let artifact = &mut value["artifacts"][Target::LinuxX64.as_str()];
        artifact["size"] = serde_json::json!(body.len());
        artifact["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(body)));
        signed(&value)
    }

    #[test]
    fn automatic_checks_only_raise_available_notices_but_manual_checks_always_report() {
        for (current, unavailable, expect_notice) in [
            ("2026.9.99", false, true),
            ("2026.10.50", false, false),
            ("2026.9.99", true, false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/native.json", listener.local_addr().unwrap());
            let (key, bytes, signature) = signed(&manifest("https://example.invalid/artifact"));
            let replies = if unavailable {
                vec![(503, Vec::new())]
            } else {
                vec![(200, bytes), (200, signature)]
            };
            let server = serve(listener, [replies.clone(), replies].concat());
            let (mut worker, _) = worker(&endpoint, &key, current);
            assert!(worker.check_automatically());
            assert!(worker.notice(true, false).is_none());
            wait_idle(&mut worker);
            assert_eq!(worker.notice(true, false).is_some(), expect_notice);
            assert_eq!(
                matches!(worker.status(), CheckStatus::Error { .. }),
                unavailable
            );
            assert!(worker.check());
            assert!(worker.notice(true, false).is_some());
            wait_idle(&mut worker);
            assert!(worker.notice(true, false).is_some());
            worker.shutdown();
            server.join().unwrap();
        }
    }

    #[test]
    fn tray_item_pins_available_and_installing_updates_with_shipping_labels() {
        let (key, _, _) = signed(&manifest("https://example.invalid/full"));
        let (mut worker, _) = worker("https://example.invalid/manifest", &key, "2026.9.99");
        let release = |notes: Option<&str>, changelog: Vec<(&str, &str)>| ReleaseInfo {
            renderer: Renderer::Wgpu,
            target: Target::LinuxX64,
            version: "2026.10.50".into(),
            notes: notes.map(Into::into),
            size: 10_000,
            changelog: changelog
                .into_iter()
                .map(|(version, notes)| crate::update_notice::ChangelogEntry {
                    version: version.into(),
                    display_version: version.into(),
                    notes: Some(notes.into()),
                })
                .collect(),
        };
        assert_eq!(worker.presentation().tray.label, "Check for Updates…");
        assert!(!worker.presentation().tray.pin_first);
        // Changelog entries win over the release notes; first-contribution
        // lines are not product changes.
        worker.status = CheckStatus::Available {
            release: release(
                Some("* one\n* two\n* three"),
                vec![
                    ("2026.10.50", "## What's Changed\n* Fix a\n* Fix b"),
                    (
                        "2026.10.49",
                        "* Fix c\n* @x made their first contribution in https://github.com/joswayski/captures/pull/1",
                    ),
                ],
            ),
        };
        let tray = worker.presentation().tray;
        assert_eq!(tray.label, "Update Available — 3 changes");
        assert!(tray.enabled && tray.pin_first);
        assert_eq!(tray.tooltip, "Captures — Update available");
        worker.status = CheckStatus::Available {
            release: release(Some("* only"), Vec::new()),
        };
        assert_eq!(
            worker.presentation().tray.label,
            "Update Available — 1 change"
        );
        worker.status = CheckStatus::Downloading {
            release: release(None, Vec::new()),
            downloaded: 1,
            total: 2,
        };
        let tray = worker.presentation().tray;
        assert_eq!(tray.label, "Installing Update…");
        assert!(!tray.enabled && tray.pin_first);
        assert_eq!(tray.tooltip, "Captures — Installing update");
        worker.status = CheckStatus::Checking;
        let tray = worker.presentation().tray;
        assert_eq!(tray.label, "Checking for Updates…");
        assert!(!tray.enabled && !tray.pin_first);
        worker.shutdown();
    }

    #[test]
    fn preview_card_offers_one_update_now_flow_like_shipping() {
        let (key, _, _) = signed(&manifest("https://example.invalid/full"));
        let (mut worker, _) = worker("https://example.invalid/manifest", &key, "2026.11.1201");
        assert!(
            worker.presentation().card.is_none(),
            "development builds keep their card"
        );
        worker.preview = true;
        worker.staging_enabled = true;
        let release = ReleaseInfo {
            renderer: Renderer::Wgpu,
            target: Target::LinuxX64,
            version: "2026.11.1302".into(),
            notes: None,
            size: 104_000_000,
            changelog: Vec::new(),
        };
        let card = worker.presentation().card.unwrap();
        assert_eq!(
            (card.action, card.step),
            ("Check Now", Some(CardStep::Check))
        );
        // The notice keeps one height from Download to Restart and install.
        worker.enable_install_request();
        for (show_changelog, open_captures) in [(true, false), (false, false), (true, true)] {
            let heights: Vec<f64> = [
                CheckStatus::Available {
                    release: release.clone(),
                },
                CheckStatus::Downloading {
                    release: release.clone(),
                    downloaded: 1,
                    total: 2,
                },
                CheckStatus::Verifying {
                    release: release.clone(),
                },
                CheckStatus::Staged {
                    release: release.clone(),
                    sha256: String::new(),
                },
            ]
            .into_iter()
            .map(|status| {
                worker.status = status;
                worker
                    .notice(show_changelog, open_captures)
                    .unwrap()
                    .card_height
            })
            .collect();
            assert!(heights.iter().all(|h| *h == heights[0]), "{heights:?}");
        }
        worker.status = CheckStatus::Available {
            release: release.clone(),
        };
        // The notice names what Download fetches, and warns about open
        // captures only when some are open.
        let notice = worker.notice(true, false).unwrap();
        assert_eq!(
            notice.description,
            "Version 2026.11.13.2 available · 104 MB"
        );
        assert_eq!(notice.version_link.unwrap().text, "2026.11.13.2");
        assert!(notice.close_warning.is_none());
        assert_eq!(
            worker.notice(true, true).unwrap().close_warning.as_deref(),
            Some(
                "Your open captures will reopen after the update. Unsaved screenshot edits are kept."
            )
        );
        for (open_captures, warned) in [(false, false), (true, true)] {
            worker.status = CheckStatus::Staged {
                release: release.clone(),
                sha256: String::new(),
            };
            let staged = worker.notice(true, open_captures).unwrap();
            assert_eq!(staged.close_warning.is_some(), warned);
            assert_eq!(staged.description, "Version 2026.11.13.2 verified");
        }
        worker.status = CheckStatus::Available {
            release: release.clone(),
        };
        let card = worker.presentation().card.unwrap();
        assert_eq!(card.heading, "Version 2026.11.13.2 is available");
        assert_eq!(card.detail, "104 MB download");
        assert_eq!(
            (card.action, card.step),
            ("Update now", Some(CardStep::Download))
        );
        worker.status = CheckStatus::Downloading {
            release: release.clone(),
            downloaded: 26_000_000,
            total: 104_000_000,
        };
        let card = worker.presentation().card.unwrap();
        assert_eq!(card.heading, "Updating to version 2026.11.13.2");
        assert_eq!(card.detail, "26.0 MB / 104 MB · 25%");
        assert_eq!((card.action, card.step), ("Installing…", None));
        assert!(card.progress_bar);
        assert_eq!(card.progress, Some(25.));
        worker.status = CheckStatus::Staged {
            release: release.clone(),
            sha256: String::new(),
        };
        assert_eq!(
            worker.presentation().card.unwrap().step,
            Some(CardStep::Install)
        );
        worker.status = CheckStatus::DownloadError {
            release,
            message: "bad signature".into(),
        };
        let card = worker.presentation().card.unwrap();
        assert_eq!(
            (card.heading.as_str(), card.action),
            ("Update failed", "Try again")
        );
        assert_eq!(
            card.error.as_deref(),
            Some("Could not install the update: bad signature")
        );
        worker.shutdown();
    }

    #[test]
    fn progress_publication_uses_patch_total_and_resets_to_full_total_without_a_new_generation() {
        let (key, _, _) = signed(&manifest("https://example.invalid/full"));
        let (mut worker, _) = worker("https://example.invalid/manifest", &key, "2026.9.99");
        let (events, results) = mpsc::channel();
        worker.results = results;
        worker.status = CheckStatus::Downloading {
            release: ReleaseInfo {
                renderer: Renderer::Wgpu,
                target: Target::LinuxX64,
                version: "2026.10.50".into(),
                notes: None,
                size: 10_000,
                changelog: Vec::new(),
            },
            downloaded: 0,
            total: 10_000,
        };
        for (downloaded, total, percent) in [(200, 400, 50), (0, 10_000, 0), (2500, 10_000, 25)] {
            events.send(Event::Progress(downloaded, total)).unwrap();
            assert!(worker.poll());
            assert_eq!(
                worker.presentation().status,
                format!("Downloading {downloaded} of {total} bytes")
            );
            let shown = worker.notice(true, false).unwrap().download.unwrap();
            assert_eq!(shown.percent, Some(percent));
            assert!(
                shown
                    .accessible_value
                    .ends_with(&format!("{percent}% downloaded"))
            );
            assert_eq!(worker.generation(), 0);
        }
        events.send(Event::Verifying).unwrap();
        assert!(worker.poll());
        events.send(Event::Progress(400, 400)).unwrap();
        assert!(!worker.poll());
        assert!(matches!(worker.status(), CheckStatus::Verifying { .. }));
        worker.shutdown();
    }

    #[test]
    fn explicit_download_retries_authenticated_bytes_and_owns_staging_until_recheck_or_quit() {
        use crate::update_notice::Action;
        let scratch = tempfile::tempdir().unwrap();
        let sentinel = scratch.path().join("existing-file");
        std::fs::write(&sentinel, b"operator data").unwrap();
        let body = super::super::staging::tests::package_fixture(Target::LinuxX64, "valid");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, bytes, signature) = signed_package(&base, &body);
        let mut corrupt = body.clone();
        corrupt[53] ^= 1;
        let server = serve(
            listener,
            vec![
                // Check, then each download rechecks before it fetches.
                (200, bytes.clone()),
                (200, signature.clone()),
                (200, bytes.clone()),
                (200, signature.clone()),
                (200, corrupt),
                (200, bytes.clone()),
                (200, signature.clone()),
                (200, body.clone()),
                (200, bytes.clone()),
                (200, signature.clone()),
                (200, bytes),
                (200, signature),
                (200, body),
            ],
        );
        let client = UpdateClient::new(
            &format!("{base}/native.json"),
            &key,
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.9.99",
        )
        .unwrap();
        let mut worker =
            CheckWorker::new(client, Arc::new(|| {}), Some(scratch.path().to_owned())).unwrap();
        assert!(!worker.download() && !worker.cancel_download());
        assert!(worker.check());
        wait_idle(&mut worker);
        assert_eq!(
            worker.presentation().acquisition.unwrap().action,
            Action::DownloadVerify
        );
        let available_height = worker.notice(true, false).unwrap().card_height;
        assert_eq!(
            std::fs::read_dir(scratch.path()).unwrap().count(),
            1,
            "checks never download"
        );
        assert!(worker.download());
        wait_idle(&mut worker);
        assert!(matches!(worker.status(), CheckStatus::DownloadError { .. }));
        assert_eq!(
            worker.presentation().status,
            "Native update artifact hash does not match its signed manifest."
        );
        assert_eq!(
            worker.presentation().acquisition.unwrap().label,
            "Retry download"
        );
        // The error keeps the card at least as tall as the update it failed.
        assert!(worker.notice(true, false).unwrap().card_height >= available_height);
        assert_eq!(
            std::fs::read_dir(scratch.path()).unwrap().count(),
            1,
            "failure removes all scratch"
        );
        assert!(worker.download());
        wait_idle(&mut worker);
        assert!(matches!(worker.status(), CheckStatus::Staged { .. }));
        assert_eq!(worker.generation(), 3);
        assert_eq!(
            std::fs::read_dir(scratch.path()).unwrap().count(),
            2,
            "retain verified package, not original download"
        );
        let notice = worker.notice(true, false).unwrap();
        assert_eq!(notice.card_height, available_height, "no jump when staged");
        assert_eq!(notice.title, "Development package verified");
        assert!(notice.restart.is_none() && notice.close_warning.is_none());
        assert!(
            !serde_json::to_string(&notice)
                .unwrap()
                .contains(&scratch.path().display().to_string())
        );
        assert!(
            !worker.download(),
            "staged ownership cannot be overwritten by another download"
        );
        assert!(worker.presentation().acquisition.is_none());
        worker.enable_install_request();
        let install = worker.presentation().acquisition.unwrap();
        assert!(install.enabled);
        assert_eq!(install.action, crate::update_notice::Action::Install);
        assert!(worker.notice(true, false).unwrap().close_warning.is_none());
        let notice = worker.notice(true, true).unwrap();
        assert_eq!(notice.footer.unwrap().primary.unwrap(), install);
        assert_eq!(
            notice.close_warning.as_deref(),
            Some(
                "Your open captures will reopen after the update. Unsaved screenshot edits are kept."
            )
        );
        assert!(worker.check());
        wait_idle(&mut worker);
        assert_eq!(
            worker.presentation().acquisition.unwrap().action,
            crate::update_notice::Action::DownloadVerify
        );
        assert_eq!(
            std::fs::read_dir(scratch.path()).unwrap().count(),
            1,
            "recheck removes retained stage"
        );
        assert!(worker.download());
        wait_idle(&mut worker);
        worker.shutdown();
        assert_eq!(
            std::fs::read_dir(scratch.path()).unwrap().count(),
            1,
            "quit removes retained stage"
        );
        assert_eq!(std::fs::read(sentinel).unwrap(), b"operator data");
        assert_eq!(
            server.join().unwrap(),
            [
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json.minisig",
                "/artifact",
                "/native.json",
                "/native.json.minisig",
                "/artifact",
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json.minisig",
                "/artifact"
            ]
        );
    }

    #[test]
    fn an_offer_left_open_follows_newer_releases_and_downloads_the_newest() {
        use sha2::{Digest, Sha256};
        let scratch = tempfile::tempdir().unwrap();
        let body = super::super::staging::tests::package_fixture(Target::LinuxX64, "valid");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let release = |version: &str| {
            let mut value = manifest(&format!("{base}/artifact"));
            value["version"] = serde_json::json!(version);
            value["notes"] = serde_json::json!(format!("Notes for {version}"));
            let artifact = &mut value["artifacts"][Target::LinuxX64.as_str()];
            artifact["size"] = serde_json::json!(body.len());
            artifact["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(&body)));
            let bytes = serde_json::to_vec(&value).unwrap();
            let signature =
                minisign::sign(None, &pair.sk, std::io::Cursor::new(&bytes), None, None).unwrap();
            [(200, bytes), (200, signature.into_string().into_bytes())]
        };
        let server = serve(
            listener,
            [
                release("2026.10.50"),
                release("2026.10.50"),
                release("2026.10.60"),
                release("2026.10.70"),
            ]
            .concat()
            .into_iter()
            .chain([(200, body.clone())])
            .chain(release("2026.10.80"))
            .collect(),
        );
        let client = UpdateClient::new(
            &format!("{base}/native.json"),
            &pair.pk.to_box().unwrap().to_string(),
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.9.99",
        )
        .unwrap();
        let mut worker =
            CheckWorker::new(client, Arc::new(|| {}), Some(scratch.path().to_owned())).unwrap();
        let offered = |worker: &CheckWorker| match worker.status() {
            CheckStatus::Available { release } | CheckStatus::Staged { release, .. } => {
                release.version.clone()
            }
            other => panic!("no offer: {other:?}"),
        };
        assert!(worker.check_automatically());
        wait_idle(&mut worker);
        assert_eq!(offered(&worker), "2026.10.50");
        let generation = worker.generation();

        // Background checks behind the open offer: the same release changes
        // nothing, a newer one replaces it without a visible check.
        assert!(worker.check_automatically());
        assert!(!worker.checking(), "a refresh never shows Checking");
        assert!(worker.check_automatically());
        let deadline = Instant::now() + Duration::from_secs(60);
        while offered(&worker) != "2026.10.60" {
            assert!(Instant::now() < deadline, "refresh never landed");
            thread::sleep(Duration::from_millis(1));
            worker.poll();
        }
        assert_eq!(worker.generation(), generation, "a refresh does not reveal");
        let notice = serde_json::to_string(&worker.notice(true, false).unwrap()).unwrap();
        assert!(notice.contains("Notes for 2026.10.60"), "{notice}");

        // Another release lands before Download: Download takes it, not 10.60.
        assert!(worker.download());
        wait_idle(&mut worker);
        assert!(matches!(worker.status(), CheckStatus::Staged { .. }));
        assert_eq!(offered(&worker), "2026.10.70");
        let staged_entries = std::fs::read_dir(scratch.path()).unwrap().count();
        assert!(staged_entries > 0);

        // A newer release behind a verified one: offer it, but keep the
        // verified package, which a Restart already chosen still installs.
        assert!(worker.check_automatically());
        while offered(&worker) != "2026.10.80" {
            assert!(Instant::now() < deadline, "staged refresh never landed");
            thread::sleep(Duration::from_millis(1));
            worker.poll();
        }
        assert!(matches!(worker.status(), CheckStatus::Available { .. }));
        assert!(worker.superseded_stage);
        assert_eq!(
            std::fs::read_dir(scratch.path()).unwrap().count(),
            staged_entries,
            "the superseded package stays until a download replaces it"
        );
        worker.shutdown();
        assert_eq!(
            server.join().unwrap(),
            [
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json.minisig",
                "/artifact",
                "/native.json",
                "/native.json.minisig",
            ]
        );
    }

    #[test]
    fn authenticated_but_incomplete_package_is_not_reported_as_verified() {
        let scratch = tempfile::tempdir().unwrap();
        let body =
            super::super::staging::tests::package_fixture(Target::LinuxX64, "missing-sidecar");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, bytes, signature) = signed_package(&base, &body);
        let server = serve(
            listener,
            vec![
                (200, bytes.clone()),
                (200, signature.clone()),
                (200, bytes),
                (200, signature),
                (200, body),
            ],
        );
        let client = UpdateClient::new(
            &format!("{base}/native.json"),
            &key,
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.9.99",
        )
        .unwrap();
        let mut worker =
            CheckWorker::new(client, Arc::new(|| {}), Some(scratch.path().to_owned())).unwrap();
        assert!(worker.check());
        wait_idle(&mut worker);
        assert!(worker.download());
        wait_idle(&mut worker);
        assert!(matches!(worker.status(), CheckStatus::DownloadError { .. }));
        assert_eq!(
            worker.presentation().status,
            "Native update package is incomplete or has the wrong development identity."
        );
        assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
        assert_eq!(
            server.join().unwrap(),
            [
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json.minisig",
                "/artifact"
            ]
        );
    }

    #[test]
    fn cancel_or_shutdown_a_partial_download_waits_for_cleanup_and_preserves_retry() {
        for shutdown in [false, true] {
            let scratch = tempfile::tempdir().unwrap();
            let body = super::super::staging::tests::package_fixture(Target::LinuxX64, "valid");
            let expected_percent = ((23_300 + body.len() / 2) / body.len()) as u8;
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (key, bytes, signature) = signed_package(&base, &body);
            let (release, gate) = mpsc::channel();
            let server = thread::spawn(move || {
                // The check, then the download's recheck.
                let mut paths = serve(
                    listener.try_clone().unwrap(),
                    vec![
                        (200, bytes.clone()),
                        (200, signature.clone()),
                        (200, bytes.clone()),
                        (200, signature.clone()),
                    ],
                )
                .join()
                .unwrap();
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line.split_whitespace().nth(1), Some("/artifact"));
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 200 Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body[..233]).unwrap();
                stream.flush().unwrap();
                gate.recv_timeout(Duration::from_secs(10)).unwrap();
                let _ = stream.write_all(&body[233..]);
                drop(stream);
                paths.push("/artifact".into());
                if !shutdown {
                    paths.extend(
                        serve(listener, vec![(200, bytes), (200, signature), (200, body)])
                            .join()
                            .unwrap(),
                    );
                }
                paths
            });
            let client = UpdateClient::new(
                &format!("{base}/native.json"),
                &key,
                Renderer::Wgpu,
                Target::LinuxX64,
                "2026.9.99",
            )
            .unwrap();
            let mut worker =
                CheckWorker::new(client, Arc::new(|| {}), Some(scratch.path().to_owned())).unwrap();
            assert!(worker.check());
            wait_idle(&mut worker);
            assert!(worker.download());
            let deadline = Instant::now() + Duration::from_secs(60);
            while !matches!(
                worker.status(),
                CheckStatus::Downloading {
                    downloaded: 233,
                    ..
                }
            ) {
                worker.poll();
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert!(!worker.check() && !worker.download());
            assert_eq!(worker.generation(), 2);
            assert_eq!(
                worker
                    .notice(true, false)
                    .unwrap()
                    .download
                    .unwrap()
                    .percent,
                Some(expected_percent)
            );
            if shutdown {
                assert!(
                    !worker.try_shutdown(),
                    "pending I/O cannot be joined on the UI thread"
                );
                assert!(!worker.try_shutdown(), "repeated quit cannot finish early");
                assert!(!worker.poll() && !worker.check() && !worker.download());
                assert!(!worker.cancel_download());
                assert_eq!(worker.generation(), 2);
                assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 1);
                release.send(()).unwrap();
                while !worker.try_shutdown() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
                assert!(!worker.poll() && !worker.download());
            } else {
                assert!(worker.cancel_download());
                assert!(matches!(worker.status(), CheckStatus::Cancelling { .. }));
                assert!(!worker.cancel_download() && !worker.download() && !worker.check());
                assert!(worker.checking() && !worker.presentation().acquisition.unwrap().enabled);
                assert_eq!(
                    worker.generation(),
                    2,
                    "cancel cannot reveal a dismissed result"
                );
                release.send(()).unwrap();
                wait_idle(&mut worker);
                assert!(matches!(worker.status(), CheckStatus::Available { .. }));
                assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
                assert!(worker.download());
                wait_idle(&mut worker);
                assert!(matches!(worker.status(), CheckStatus::Staged { .. }));
                worker.shutdown();
            }
            assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
            let mut expected = vec![
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json.minisig",
                "/artifact",
            ];
            if !shutdown {
                expected.extend(["/native.json", "/native.json.minisig", "/artifact"]);
            }
            assert_eq!(server.join().unwrap(), expected);
        }
    }

    #[test]
    fn real_notices_use_signed_notes_without_install_or_shipping_fallbacks() {
        use crate::update_notice::Action;
        let (key, _, _) = signed(&manifest("https://example.invalid/artifact"));
        let (mut worker, _) = worker("http://127.0.0.1:9/native.json", &key, "2026.9.99");
        assert!(worker.notice(true, false).is_none());
        worker.status = CheckStatus::Available {
            release: ReleaseInfo {
                renderer: Renderer::Wgpu,
                target: Target::LinuxX64,
                version: "2026.10.50".into(),
                notes: Some("* Fix native selection (#321)".into()),
                size: 913,
                changelog: Vec::new(),
            },
        };
        let expanded = worker.notice(true, false).unwrap();
        assert_eq!(expanded.title, "Native development update");
        assert_eq!(
            expanded.description,
            "2026.10.50 · Check only; no installation"
        );
        assert_eq!(
            expanded.notes.as_ref().unwrap().groups[0].items[0].text,
            "Fix native selection"
        );
        assert_eq!(
            expanded
                .footer
                .as_ref()
                .unwrap()
                .primary
                .as_ref()
                .unwrap()
                .action,
            Action::Check
        );
        assert!(
            expanded.close_warning.is_none()
                && expanded.download.is_none()
                && expanded.restart.is_none()
        );
        let collapsed = worker.notice(false, false).unwrap();
        assert!(collapsed.notes.is_none());
        for status in [
            CheckStatus::Checking,
            CheckStatus::UpToDate,
            CheckStatus::Error {
                message: "Native update signature verification failed.".into(),
            },
        ] {
            worker.status = status;
            let p = worker.notice(true, false).unwrap();
            assert!(p.notes.is_none() && p.download.is_none() && p.restart.is_none());
            assert!(!serde_json::to_string(&p).unwrap().contains("captur.es"));
            if worker.checking() {
                assert!(p.footer.unwrap().primary.is_none());
            } else {
                assert_eq!(p.footer.unwrap().primary.unwrap().action, Action::Check);
            }
        }
    }

    #[test]
    fn explicit_key_configuration_is_bounded_and_does_not_reveal_the_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("public.key");
        let (key, _, _) = signed(&manifest("https://example.invalid/artifact"));
        std::fs::write(&path, &key).unwrap();
        let client = client_from_key_file(
            "http://127.0.0.1:9/private-endpoint",
            &path,
            Renderer::Wgpu,
            "2026.9.99",
        )
        .unwrap();
        let debug = format!("{client:?}");
        assert!(!debug.contains("private-endpoint") && !debug.contains(&key));
        for bytes in [vec![b'x'; 8193], vec![0xff], b"not a public key".to_vec()] {
            std::fs::write(&path, bytes).unwrap();
            assert!(
                client_from_key_file(
                    "http://127.0.0.1:9/native.json",
                    &path,
                    Renderer::Wgpu,
                    "2026.9.99"
                )
                .is_err()
            );
        }
        assert!(
            client_from_key_file(
                "http://127.0.0.1:9/native.json",
                root.path(),
                Renderer::Wgpu,
                "2026.9.99"
            )
            .is_err()
        );
    }

    #[test]
    fn only_an_explicit_check_publishes_signed_metadata_and_never_downloads() {
        for current in ["2026.9.99", "2026.10.50", "2026.10.51"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (key, bytes, signature) = signed(&manifest(&format!("{base}/artifact")));
            let server = serve(
                listener,
                vec![
                    (200, bytes.clone()),
                    (200, signature.clone()),
                    (200, bytes),
                    (200, signature),
                ],
            );
            let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, current);
            assert!(matches!(worker.status(), CheckStatus::Idle));
            assert_eq!(worker.presentation().status, "Not checked");
            assert!(observed.try_recv().is_err());
            for _ in 0..2 {
                assert!(worker.check());
                assert!(worker.checking());
                assert!(!worker.presentation().enabled);
                observed.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(worker.poll());
                assert!(!worker.checking() && worker.presentation().enabled);
                if current == "2026.9.99" {
                    assert!(
                        !worker.download(),
                        "check-only launches cannot acquire even authenticated bytes"
                    );
                    let CheckStatus::Available { release } = worker.status() else {
                        panic!("newer release not published")
                    };
                    assert_eq!(release.version, "2026.10.50");
                    assert_eq!(
                        release.notes.as_deref(),
                        Some("Native development fixture only.")
                    );
                    assert_eq!(release.size, 23);
                    assert_eq!(
                        worker.presentation().status,
                        "Development update 2026.10.50 available"
                    );
                } else {
                    assert!(matches!(worker.status(), CheckStatus::UpToDate));
                    assert_eq!(
                        worker.presentation().status,
                        "No development updates available"
                    );
                }
                assert_eq!(
                    worker.presentation().version,
                    format!("Native development {current}")
                );
            }
            worker.shutdown();
            assert!(!worker.check());
            assert_eq!(
                server.join().unwrap(),
                [
                    "/native.json",
                    "/native.json.minisig",
                    "/native.json",
                    "/native.json.minisig"
                ]
            );
        }
    }

    #[test]
    fn failed_checks_are_errors_not_up_to_date_and_can_be_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, mut tampered, signature) = signed(&manifest(&format!("{base}/artifact")));
        let original = tampered.clone();
        tampered.push(b' '); // Still valid JSON; no longer the signed bytes.
        let server = serve(
            listener,
            vec![
                (200, tampered),
                (200, signature.clone()),
                (503, vec![]),
                (200, original),
                (200, signature),
            ],
        );
        let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, "2026.9.99");
        for message in [
            "Native update signature verification failed.",
            "Native update service returned HTTP 503.",
        ] {
            assert!(worker.check());
            observed.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(worker.poll());
            assert!(matches!(worker.status(), CheckStatus::Error { .. }));
            assert_eq!(worker.presentation().status, message);
            assert_eq!(worker.presentation().action, "Check Now");
            assert!(worker.presentation().failed);
        }
        assert!(worker.check());
        assert!(!worker.presentation().failed);
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(worker.poll());
        assert!(matches!(worker.status(), CheckStatus::Available { .. }));
        assert_eq!(
            server.join().unwrap(),
            [
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json",
                "/native.json.minisig"
            ]
        );
    }

    // Hold the first response until the caller has exercised input or shutdown.
    // This tests the boundary deterministically instead of racing a fast server.
    fn delayed_manifest(
        listener: TcpListener,
        bytes: Vec<u8>,
        signature: Option<Vec<u8>>,
    ) -> (
        mpsc::Receiver<()>,
        mpsc::Sender<()>,
        JoinHandle<Vec<String>>,
    ) {
        let (started, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.split_whitespace().nth(1), Some("/native.json"));
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
            drop(stream);
            let mut paths = vec!["/native.json".into()];
            if let Some(signature) = signature {
                paths.extend(serve(listener, vec![(200, signature)]).join().unwrap());
            }
            paths
        });
        (observed, release, server)
    }

    #[test]
    fn a_second_check_during_the_response_cannot_enqueue_work() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, bytes, signature) = signed(&manifest(&format!("{base}/artifact")));
        let (started, release, server) = delayed_manifest(listener, bytes, Some(signature));
        let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, "2026.9.99");
        assert!(worker.check());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(worker.generation(), 1);
        assert!(!worker.check());
        assert_eq!(worker.generation(), 1);
        release.send(()).unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(worker.poll());
        assert!(matches!(worker.status(), CheckStatus::Available { .. }));
        worker.shutdown();
        assert!(observed.try_recv().is_err());
        assert_eq!(
            server.join().unwrap(),
            ["/native.json", "/native.json.minisig"]
        );
    }

    #[test]
    fn shutdown_joins_pending_http_without_publishing_or_waking_a_closed_host() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, bytes, _) = signed(&manifest(&format!("{base}/artifact")));
        let (started, release, server) = delayed_manifest(listener, bytes, None);
        let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, "2026.9.99");
        assert!(worker.check());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let cancel = worker.cancel.clone();
        let joined = thread::spawn(move || {
            worker.shutdown();
            worker
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cancel.is_cancelled() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        release.send(()).unwrap();
        let mut worker = joined.join().unwrap();
        assert!(!worker.poll() && !worker.check());
        assert!(observed.try_recv().is_err());
        assert_eq!(server.join().unwrap(), ["/native.json"]);
    }
}
