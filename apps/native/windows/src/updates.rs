//! Preferences' Updates card on the native Preview channel, as shipping's
//! `UpdateSettings` (`apps/desktop/ui/src/App.tsx`) draws it. The resident
//! app (`live.rs`) owns the shared `CheckWorker::preview`, which checks,
//! downloads and verifies the signed Preview; this module only maps its
//! status to shipping's copy and single action button. Shipping's Update now
//! downloads and installs in one step, so here it downloads and verifies, and
//! the resident app hands the staged package to the update helper once it is
//! verified. A failed download or install reads "Update failed" with
//! "Try again"; a failed check reads "Couldn’t check for updates".
use captures_app::update_notice::format_update_size;
use captures_app::updater::checks::CheckStatus;

/// What the card's button does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Check,
    /// Download and verify, then install (Update now, Try again).
    Download,
    /// Install the already verified package.
    Install,
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
    /// `.update-settings-error` (`role="alert"`).
    pub error: Option<String>,
}

/// Shipping's Preferences fallback under the row.
pub const FALLBACK: &str = "If this copy cannot update itself, download from captur.es and install over it. Settings and captures stay.";
pub const CHANNEL: &str = "Preview channel";

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
pub fn card(current: &str, status: &CheckStatus, installing: bool) -> Card {
    let mut card = Card {
        version: format!("Version {}", display_version(current)),
        channel: CHANNEL,
        action: "Check Now",
        step: Some(Step::Check),
        heading: String::new(),
        detail: String::new(),
        current: false,
        progress: None,
        error: None,
    };
    let updating = |card: &mut Card, version: &str| {
        card.heading = format!("Updating to version {}", display_version(version));
        card.action = "Installing…";
        card.step = None;
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
        CheckStatus::Available { release } => {
            card.heading = format!("Version {} is available", display_version(&release.version));
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
            card.heading = format!("Version {} is available", display_version(&release.version));
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

/// `--scene preferences --select update-STATE`: a Preview card for
/// offscreen renders (`idle`, `current`, `available`, `downloading`,
/// `failed`, `check-failed`).
pub fn fixture(select: &str) -> Option<Card> {
    use captures_app::updater::{ReleaseInfo, Renderer, Target};
    let release = ReleaseInfo {
        renderer: Renderer::Wgpu,
        target: Target::WindowsX64,
        version: "2026.11.1302".into(),
        notes: None,
        size: 104_000_000,
    };
    let (status, installing) = match select.strip_prefix("update-")? {
        "idle" => (CheckStatus::Idle, false),
        "current" => (CheckStatus::UpToDate, false),
        "available" => (CheckStatus::Available { release }, false),
        "downloading" => (
            CheckStatus::Downloading {
                release,
                downloaded: 52_000_000,
                total: 104_000_000,
            },
            true,
        ),
        "failed" => (
            CheckStatus::DownloadError {
                release,
                message: "Native update signature verification failed.".into(),
            },
            false,
        ),
        "check-failed" => (
            CheckStatus::Error {
                message: "Could not reach the native update service.".into(),
            },
            false,
        ),
        _ => return None,
    };
    Some(card("2026.11.1201", &status, installing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::updater::{ReleaseInfo, Renderer, Target};

    fn release() -> ReleaseInfo {
        ReleaseInfo {
            renderer: Renderer::Wgpu,
            target: Target::WindowsX64,
            version: "2026.11.1302".into(),
            notes: None,
            size: 104_000_000,
        }
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
        let idle = card("2026.11.1201", &CheckStatus::Idle, false);
        assert_eq!(idle.version, "Version 2026.11.12.1");
        assert_eq!(idle.channel, "Preview channel");
        assert_eq!((idle.action, idle.step), ("Check Now", Some(Step::Check)));
        assert!(idle.heading.is_empty() && idle.error.is_none());
        let checking = card("2026.11.1201", &CheckStatus::Checking, false);
        assert_eq!((checking.action, checking.step), ("Checking now…", None));
        let current = card("2026.11.1201", &CheckStatus::UpToDate, false);
        assert_eq!(current.heading, "Up to date");
        assert!(current.current);
        assert_eq!(current.step, Some(Step::Check));
    }

    #[test]
    fn available_updates_now_and_reports_progress() {
        let available = card(
            "2026.11.1201",
            &CheckStatus::Available { release: release() },
            false,
        );
        assert_eq!(available.heading, "Version 2026.11.13.2 is available");
        assert_eq!(available.detail, "104 MB download");
        assert_eq!(
            (available.action, available.step),
            ("Update now", Some(Step::Download))
        );
        let downloading = card(
            "2026.11.1201",
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
        let unknown = card(
            "2026.11.1201",
            &CheckStatus::Downloading {
                release: release(),
                downloaded: 1_000,
                total: 0,
            },
            true,
        );
        assert_eq!(unknown.detail, "1.0 KB downloaded");
        assert_eq!(unknown.progress, Some(None));
        let verifying = card(
            "2026.11.1201",
            &CheckStatus::Verifying { release: release() },
            true,
        );
        assert_eq!(
            (verifying.action, verifying.progress),
            ("Installing…", Some(None))
        );
    }

    #[test]
    fn a_staged_package_restarts_or_waits_for_update_now() {
        let staged = CheckStatus::Staged {
            release: release(),
            sha256: "00".into(),
        };
        let restarting = card("2026.11.1201", &staged, true);
        assert_eq!(restarting.heading, "Update complete");
        assert_eq!(restarting.detail, "Reopening Captures…");
        assert_eq!((restarting.action, restarting.step), ("Restarting…", None));
        let held = card("2026.11.1201", &staged, false);
        assert_eq!(
            (held.action, held.step),
            ("Update now", Some(Step::Install))
        );
    }

    #[test]
    fn failures_use_shipping_headings_and_actions() {
        let check = card(
            "2026.11.1201",
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
        let install = card(
            "2026.11.1201",
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
}
