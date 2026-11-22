//! Live-primary-only local diagnostics. No network or capture-file collection.
use std::{path::Path, sync::Arc};

use captures_feedback::crash_diagnostics::{CrashSession, DiagnosticPreview};
use captures_session::ShutdownMonitor;
use serde::Serialize;

/// All text the host can copy or explicitly add to its editable Feedback draft.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Review {
    pub title: &'static str,
    pub explanation: &'static str,
    pub summary: String,
    pub unclean_exit: bool,
    pub has_exception_evidence: bool,
}

impl Review {
    pub fn from_preview(preview: DiagnosticPreview) -> Option<Self> {
        if !preview.unclean_exit && !preview.has_exception_evidence() {
            return None;
        }
        let has_exception_evidence = preview.has_exception_evidence();
        let title = if has_exception_evidence {
            "Previous session exception evidence"
        } else {
            "Previous session did not close normally"
        };
        let explanation = "Review the local summary before sharing it. A forced stop or power loss is not proof of a crash. Nothing has been sent. Captures, logs and environment variables are not attached.";
        let mut summary = format!("{title}\n");
        if let Some(panic) = preview.rust_panic {
            summary.push_str("\nRedacted Rust panic:\n");
            summary.push_str(&panic);
        }
        if let Some(report) = preview.os_report {
            summary.push_str("\n\nRedacted OS exception summary:\n");
            summary.push_str(&report);
        }
        if !has_exception_evidence {
            summary
                .push_str("\nA session marker remained; no panic or OS exception was confirmed.");
        }
        Some(Self {
            title,
            explanation,
            summary,
            unclean_exit: preview.unclean_exit,
            has_exception_evidence,
        })
    }
}

/// Construct only after winning profile election. Clean/disarm before releasing
/// that election, and resume only after reacquiring it on a failed restart.
pub struct Session {
    evidence: Arc<CrashSession>,
    shutdown: ShutdownMonitor,
}

impl Session {
    pub fn start(profile: &Path) -> Result<Self, String> {
        let evidence = Arc::new(
            CrashSession::start(profile).map_err(|_| "Local diagnostics could not start.")?,
        );
        let cancelled = evidence.clone();
        let shutdown = match ShutdownMonitor::install(evidence.clean_exit_paths(), move || {
            if cancelled.resume_after_cancelled_exit().is_err() {
                eprintln!("Captures could not restore diagnostics after cancelled OS shutdown.");
            }
        }) {
            Ok(shutdown) => shutdown,
            Err(_) => {
                let _ = evidence.mark_clean_exit();
                return Err("Local diagnostics could not register OS shutdown handling.".into());
            }
        };
        evidence.install_panic_hook();
        Ok(Self { evidence, shutdown })
    }

    pub fn preview(&self) -> Option<Review> {
        let mut preview = self.evidence.preview();
        if preview.unclean_exit && preview.os_report.is_none() {
            preview.os_report = latest_os_report(preview.previous_session_started_at);
        }
        Review::from_preview(preview)
    }

    pub fn dismiss(&self) -> Result<(), String> {
        self.evidence
            .dismiss_previous()
            .map_err(|_| "Previous-session evidence could not be dismissed.".into())
    }

    pub fn clean_exit(&self) -> Result<(), String> {
        self.shutdown.disarm();
        self.evidence
            .mark_clean_exit()
            .map_err(|_| "Local diagnostics could not close cleanly.".into())
    }

    pub fn resume(&self) -> Result<(), String> {
        self.evidence
            .resume_after_cancelled_exit()
            .map_err(|_| "Local diagnostics could not resume.".to_owned())?;
        self.shutdown.rearm();
        Ok(())
    }
}

/// Shipping `latest_macos_crash_snippet`: a Swift or AppKit crash leaves no
/// Rust panic, so the newest matching DiagnosticReports file from the prior
/// session is summarized after the report's own process name is checked.
#[cfg(target_os = "macos")]
fn latest_os_report(started: Option<std::time::SystemTime>) -> Option<String> {
    use captures_feedback::crash_diagnostics::{ReportIdentity, summarize_report_path};
    let executable = std::env::current_exe().ok()?;
    let name = executable.file_name()?.to_str()?.to_owned();
    let reports =
        std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/DiagnosticReports");
    newest_report(&reports, &name).and_then(|(modified, path)| {
        summarize_report_path(
            &path,
            modified,
            started,
            &ReportIdentity {
                executable_name: &name,
                bundle_id: None,
                executable_path: None,
            },
        )
        .ok()
        .flatten()
    })
}

#[cfg(not(target_os = "macos"))]
fn latest_os_report(_started: Option<std::time::SystemTime>) -> Option<String> {
    None
}

/// The newest `<process>-*.ips` or `.crash` report in `directory`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn newest_report(
    directory: &Path,
    process: &str,
) -> Option<(std::time::SystemTime, std::path::PathBuf)> {
    let prefix = format!("{process}-");
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            (name.starts_with(&prefix) && (name.ends_with(".ips") || name.ends_with(".crash")))
                .then_some(())?;
            Some((entry.metadata().ok()?.modified().ok()?, entry.path()))
        })
        .max_by_key(|(modified, _)| *modified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_report_picks_this_process_latest_crash_only() {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, age: u64| {
            let path = directory.path().join(name);
            std::fs::write(&path, "{}").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(age))
                .unwrap();
        };
        write("CapturesNative-2026-11-20-old.ips", 600);
        write("CapturesNative-2026-11-22-new.ips", 60);
        write("CapturesNativeHelper-2026-11-22-newer.ips", 1);
        write("CapturesNative-2026-11-22-newest.diag", 0);
        let (_, path) = newest_report(directory.path(), "CapturesNative").unwrap();
        assert!(path.ends_with("CapturesNative-2026-11-22-new.ips"));
        assert!(newest_report(directory.path(), "Other").is_none());
    }

    #[test]
    fn unclean_exit_is_not_exception_evidence_and_clean_exit_has_no_review() {
        let clean = DiagnosticPreview {
            previous_session_started_at: None,
            unclean_exit: false,
            rust_panic: None,
            os_report: None,
        };
        assert!(Review::from_preview(clean.clone()).is_none());
        let review = Review::from_preview(DiagnosticPreview {
            unclean_exit: true,
            ..clean.clone()
        })
        .unwrap();
        assert!(!review.has_exception_evidence);
        assert_eq!(review.title, "Previous session did not close normally");
        assert!(
            review
                .summary
                .contains("no panic or OS exception was confirmed")
        );
        let panic = Review::from_preview(DiagnosticPreview {
            unclean_exit: true,
            rust_panic: Some("Panic at ~/source.rs".into()),
            ..clean
        })
        .unwrap();
        assert!(panic.has_exception_evidence);
        assert!(panic.summary.contains("Panic at ~/source.rs"));
        assert!(panic.explanation.contains("Nothing has been sent"));
    }
}
