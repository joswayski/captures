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
        Review::from_preview(self.evidence.preview())
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

#[cfg(test)]
mod tests {
    use super::*;

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
