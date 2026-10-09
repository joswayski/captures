//! Explicit native development checks and opt-in temporary package verification.
//! No automatic requests, installation, profile access or channel activation.
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
}

enum Job {
    Check(CancelToken),
    Download(CancelToken),
    Shutdown,
}

enum Event {
    Finished(CheckStatus),
    Progress(u64, u64),
    Verifying,
}

pub struct CheckWorker {
    current_version: String,
    status: CheckStatus,
    generation: u64,
    staging_enabled: bool,
    install_request_enabled: bool,
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
        if let Some(path) = &staging_directory {
            validate_staging_directory(path)?;
        }
        let current_version = client.current_version.to_string();
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
                    Job::Download(token) => {
                        let Some(update) = &pending else { continue };
                        let Some(directory) = &staging_directory else {
                            continue;
                        };
                        let release = update.info().clone();
                        let result = update
                            .download(directory, &token, |received, total| {
                                if !cancelled.is_cancelled() && !token.is_cancelled() {
                                    let _ = finished.send(Event::Progress(received, total));
                                    wake();
                                }
                            })
                            .and_then(|verified| {
                                if !cancelled.is_cancelled() && !token.is_cancelled() {
                                    let _ = finished.send(Event::Verifying);
                                    wake();
                                }
                                verified.stage(directory, &token)
                            });
                        if token.is_cancelled() {
                            // Cleanup precedes the cancellation acknowledgement.
                            drop(result);
                            CheckStatus::Available { release }
                        } else {
                            match result {
                                Ok(package) => {
                                    staged = Some(package);
                                    CheckStatus::Staged {
                                        release,
                                        sha256: update.sha256(),
                                    }
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
            jobs,
            results,
            operation_cancel: None,
            cancel,
            thread: Some(thread),
        })
    }

    /// Only a GUI launched by the explicit development supervisor enables this
    /// presentation route. The worker itself cannot replace or execute a package.
    pub fn enable_install_request(&mut self) {
        self.install_request_enabled = self.staging_enabled;
    }

    /// Repeated input during any operation cannot enqueue duplicate requests.
    pub fn check(&mut self) -> bool {
        self.poll();
        if self.cancel.is_cancelled() || self.checking() || self.thread.is_none() {
            return false;
        }
        self.status = CheckStatus::Checking;
        let token = CancelToken::default();
        self.operation_cancel = Some(token.clone());
        let accepted = self.jobs.send(Job::Check(token)).is_ok();
        if accepted {
            self.generation += 1;
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
        let token = CancelToken::default();
        self.operation_cancel = Some(token.clone());
        let accepted = self.jobs.send(Job::Download(token)).is_ok();
        if accepted {
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
    /// download-page link, close-captures warning or simulated restart.
    pub fn notice(&self, show_changelog: bool) -> Option<crate::update_notice::Presentation> {
        use crate::update_notice::{self as notice, Action, Button, UpdateStatus, ViewState};
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
                display_version: release.version.clone(),
                notes: release.notes.clone(),
                changelog: vec![],
                installable: false,
                manual_download_url: None,
                download_size: Some(release.size),
                will_close_open_captures: false,
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
            if self.install_request_enabled {
                p.close_warning = Some(notice::OPEN_CAPTURES_WARNING.into());
            }
        }
        Some(p)
    }

    fn acquisition(&self) -> Option<crate::update_notice::Button> {
        use crate::update_notice::{Action, Button};
        if !self.staging_enabled || self.thread.is_none() {
            return None;
        }
        let (label, enabled, action) = match self.status {
            CheckStatus::Available { .. } => ("Download and verify", true, Action::DownloadVerify),
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
        })
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
        Presentation {
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
                "Only Restart and install requests replacement of the selected development package. Ordinary Quit does not install. Installed Previews and release channels are unchanged."
            } else if self.staging_enabled {
                "Explicit downloads validate a temporary package. No installation, execution or update channel is enabled."
            } else {
                "Verifies signed native development metadata. No download, installation or update channel is enabled."
            },
            acquisition: self.acquisition(),
        }
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
        let deadline = Instant::now() + Duration::from_secs(10);
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
            let shown = worker.notice(true).unwrap().download.unwrap();
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
                (200, bytes.clone()),
                (200, signature.clone()),
                (200, corrupt),
                (200, body.clone()),
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
        let notice = worker.notice(true).unwrap();
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
        let notice = worker.notice(true).unwrap();
        assert_eq!(notice.footer.unwrap().primary.unwrap(), install);
        assert_eq!(
            notice.close_warning.as_deref(),
            Some(crate::update_notice::OPEN_CAPTURES_WARNING)
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
                "/artifact",
                "/artifact",
                "/native.json",
                "/native.json.minisig",
                "/artifact"
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
        let server = serve(listener, vec![(200, bytes), (200, signature), (200, body)]);
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
            ["/native.json", "/native.json.minisig", "/artifact"]
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
                let mut paths = serve(
                    listener.try_clone().unwrap(),
                    vec![(200, bytes), (200, signature)],
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
                    paths.extend(serve(listener, vec![(200, body)]).join().unwrap());
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
            let deadline = Instant::now() + Duration::from_secs(10);
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
                worker.notice(true).unwrap().download.unwrap().percent,
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
            let expected = if shutdown {
                vec!["/native.json", "/native.json.minisig", "/artifact"]
            } else {
                vec![
                    "/native.json",
                    "/native.json.minisig",
                    "/artifact",
                    "/artifact",
                ]
            };
            assert_eq!(server.join().unwrap(), expected);
        }
    }

    #[test]
    fn real_notices_use_signed_notes_without_install_or_shipping_fallbacks() {
        use crate::update_notice::Action;
        let (key, _, _) = signed(&manifest("https://example.invalid/artifact"));
        let (mut worker, _) = worker("http://127.0.0.1:9/native.json", &key, "2026.9.99");
        assert!(worker.notice(true).is_none());
        worker.status = CheckStatus::Available {
            release: ReleaseInfo {
                renderer: Renderer::Wgpu,
                target: Target::LinuxX64,
                version: "2026.10.50".into(),
                notes: Some("* Fix native selection (#321)".into()),
                size: 913,
            },
        };
        let expanded = worker.notice(true).unwrap();
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
        let collapsed = worker.notice(false).unwrap();
        assert!(collapsed.notes.is_none() && collapsed.reveal_notes.is_some());
        for status in [
            CheckStatus::Checking,
            CheckStatus::UpToDate,
            CheckStatus::Error {
                message: "Native update signature verification failed.".into(),
            },
        ] {
            worker.status = status;
            let p = worker.notice(true).unwrap();
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
