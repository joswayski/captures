//! Shipping Screen Recording recovery after a capture fails (macOS only).
//!
//! Tauri (`report_capture_error`) answers a denied capture with one of two
//! dialogs: "Restart & Retry" when access was requested during this launch,
//! otherwise "Reset, Restart & Retry", which first resets only this app's
//! stale TCC record. Either path persists `pending_capture_after_restart` and
//! relaunches; the next launch takes that mode and runs the capture again.
//! Hosts own the dialog, the relaunch and starting the capture; this module
//! owns the copy, the classification and the persisted retry.

use std::path::Path;

use captures_capture::{CaptureError, CaptureMode};
use serde::Serialize;

pub const TITLE: &str = "Captures Setup";
pub const RESTART_MESSAGE: &str = "macOS requires Captures to restart before newly granted Screen \
Recording access becomes available. Captures can restart now and automatically retry this capture.";
pub const RESET_MESSAGE: &str = "This locally built Captures copy no longer matches macOS's saved \
Screen Recording record. Captures can reset only its own record, restart, and retry this capture. \
You will still need to approve Captures in System Settings; macOS does not allow apps to toggle \
this permission themselves.";
pub const RESTART_ACTION: &str = "Restart & Retry";
pub const RESET_ACTION: &str = "Reset, Restart & Retry";
pub const CANCEL_ACTION: &str = "Not Now";

/// Which shipping dialog a denied capture shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    /// Access was requested this launch; macOS applies it only after relaunch.
    Restart,
    /// The saved TCC record no longer matches this build.
    ResetAndRestart,
}

/// Dialog contents. `critical` mirrors Tauri's `MessageDialogKind::Error`
/// (reset) versus `Info` (restart).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Prompt {
    pub recovery: Recovery,
    pub title: &'static str,
    pub message: &'static str,
    pub confirm: &'static str,
    pub cancel: &'static str,
    pub critical: bool,
}

pub const fn prompt(requested_this_launch: bool) -> Prompt {
    if requested_this_launch {
        Prompt {
            recovery: Recovery::Restart,
            title: TITLE,
            message: RESTART_MESSAGE,
            confirm: RESTART_ACTION,
            cancel: CANCEL_ACTION,
            critical: false,
        }
    } else {
        Prompt {
            recovery: Recovery::ResetAndRestart,
            title: TITLE,
            message: RESET_MESSAGE,
            confirm: RESET_ACTION,
            cancel: CANCEL_ACTION,
            critical: true,
        }
    }
}

/// Shipping `show_macos_permission_recovery_error` body.
pub fn failure_message(error: &str) -> String {
    format!("Captures could not reset or restart its Screen Recording setup: {error}")
}

/// True when a host's capture error text is the denied-permission failure.
/// Native errors cross the ABI as text, so match the shared error's message.
/// A started permission request is excluded: macOS shows its own prompt.
pub fn is_permission_denied(message: &str) -> bool {
    message.contains(&CaptureError::PermissionDenied.to_string())
}

/// Whether this platform offers the recovery dialog at all (shipping gates
/// it with `cfg(target_os = "macos")`; other platforms keep the plain error).
pub const fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Persist the capture to run after the relaunch. Call before relaunching.
pub fn schedule_retry(path: &Path, mode: CaptureMode) -> Result<(), String> {
    let mut settings = captures_settings::load(path).map_err(|e| e.to_string())?;
    settings.pending_capture_after_restart = Some(mode);
    captures_settings::write_atomic(path, &settings).map_err(|e| e.to_string())
}

/// Take (and clear) the capture scheduled by [`schedule_retry`]. Clearing
/// first means a crash during the retried capture cannot loop relaunches.
pub fn take_pending_capture(path: &Path) -> Result<Option<CaptureMode>, String> {
    let mut settings = captures_settings::load(path).map_err(|e| e.to_string())?;
    let pending = settings.pending_capture_after_restart.take();
    if pending.is_some() {
        captures_settings::write_atomic(path, &settings).map_err(|e| e.to_string())?;
    }
    Ok(pending)
}

/// Resets this app's own macOS Screen Recording record.
pub trait PermissionReset {
    fn reset_screen_capture(&mut self, bundle_id: &str) -> Result<(), String>;
}

/// `/usr/bin/tccutil reset ScreenCapture <bundle id>`, as shipping runs it.
pub struct Tccutil;

impl PermissionReset for Tccutil {
    fn reset_screen_capture(&mut self, bundle_id: &str) -> Result<(), String> {
        let status = std::process::Command::new("/usr/bin/tccutil")
            .args(["reset", "ScreenCapture", bundle_id])
            .status()
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("tccutil exited with status {status}"))
        }
    }
}

/// Shipping `reset_macos_screen_capture_permission`: reset only this bundle's
/// record, then forget the saved prompt identity so setup can ask again.
/// The caller also clears its "requested this launch" flag.
pub fn reset_screen_permission(
    path: &Path,
    bundle_id: &str,
    reset: &mut impl PermissionReset,
) -> Result<(), String> {
    let bundle_id = bundle_id.trim();
    if bundle_id.is_empty() {
        return Err("Captures has no bundle identifier to reset.".into());
    }
    // Fail closed on unreadable settings before touching the TCC database.
    let mut settings = captures_settings::load(path).map_err(|e| e.to_string())?;
    reset.reset_screen_capture(bundle_id)?;
    if settings.last_screen_permission_request_id.take().is_some() {
        captures_settings::write_atomic(path, &settings).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_settings::AppSettings;

    struct FakeReset {
        calls: Vec<String>,
        result: Result<(), String>,
    }

    impl PermissionReset for FakeReset {
        fn reset_screen_capture(&mut self, bundle_id: &str) -> Result<(), String> {
            self.calls.push(bundle_id.to_owned());
            self.result.clone()
        }
    }

    fn settings_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        captures_settings::write_atomic(&path, &AppSettings::default()).unwrap();
        (directory, path)
    }

    #[test]
    fn prompts_match_the_shipping_dialogs() {
        let restart = prompt(true);
        assert_eq!(restart.recovery, Recovery::Restart);
        assert_eq!(restart.title, "Captures Setup");
        assert_eq!(restart.confirm, "Restart & Retry");
        assert_eq!(restart.cancel, "Not Now");
        assert!(!restart.critical);
        assert!(
            restart
                .message
                .starts_with("macOS requires Captures to restart")
        );
        assert!(
            restart
                .message
                .ends_with("automatically retry this capture.")
        );

        let reset = prompt(false);
        assert_eq!(reset.recovery, Recovery::ResetAndRestart);
        assert_eq!(reset.confirm, "Reset, Restart & Retry");
        assert!(reset.critical);
        assert!(reset.message.contains("reset only its own record"));
        assert!(
            reset
                .message
                .ends_with("toggle this permission themselves.")
        );
        for message in [restart.message, reset.message] {
            assert!(!message.contains("  "), "{message}");
        }
        assert_eq!(
            failure_message("tccutil exited with status 1"),
            "Captures could not reset or restart its Screen Recording setup: tccutil exited with status 1"
        );
    }

    #[test]
    fn only_denied_capture_errors_offer_recovery() {
        let denied = crate::Error::from(CaptureError::PermissionDenied).to_string();
        assert!(is_permission_denied(&denied));
        assert!(is_permission_denied(&format!(
            "Couldn’t start capture: {denied}"
        )));
        assert!(!is_permission_denied(
            &CaptureError::PermissionRequestStarted.to_string()
        ));
        assert!(!is_permission_denied(
            &CaptureError::SessionUnavailable.to_string()
        ));
        assert!(!is_permission_denied("Capture cancelled"));
    }

    #[test]
    fn retry_survives_relaunch_once() {
        let (_directory, path) = settings_file();
        assert_eq!(take_pending_capture(&path).unwrap(), None);
        schedule_retry(&path, CaptureMode::Window).unwrap();
        let saved = captures_settings::load(&path).unwrap();
        assert_eq!(
            saved.pending_capture_after_restart,
            Some(CaptureMode::Window)
        );
        assert_eq!(
            take_pending_capture(&path).unwrap(),
            Some(CaptureMode::Window)
        );
        assert_eq!(take_pending_capture(&path).unwrap(), None);
        assert_eq!(
            captures_settings::load(&path)
                .unwrap()
                .pending_capture_after_restart,
            None
        );
    }

    #[test]
    fn unreadable_settings_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(schedule_retry(&path, CaptureMode::Region).is_err());
        assert!(take_pending_capture(&path).is_err());
        let mut reset = FakeReset {
            calls: Vec::new(),
            result: Ok(()),
        };
        assert!(reset_screen_permission(&path, "dev.captures.native", &mut reset).is_err());
        assert!(reset.calls.is_empty(), "no TCC reset without settings");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }

    #[test]
    fn reset_clears_only_the_saved_prompt_identity() {
        let (_directory, path) = settings_file();
        let mut settings = captures_settings::load(&path).unwrap();
        settings.last_screen_permission_request_id = Some("exe:1:2".into());
        settings.onboarding_completed = true;
        captures_settings::write_atomic(&path, &settings).unwrap();

        let mut reset = FakeReset {
            calls: Vec::new(),
            result: Ok(()),
        };
        reset_screen_permission(&path, " dev.captures.native ", &mut reset).unwrap();
        assert_eq!(reset.calls, ["dev.captures.native"]);
        let saved = captures_settings::load(&path).unwrap();
        assert_eq!(saved.last_screen_permission_request_id, None);
        assert!(saved.onboarding_completed);

        assert!(reset_screen_permission(&path, "  ", &mut reset).is_err());
        assert_eq!(
            reset.calls.len(),
            1,
            "a blank identifier never runs tccutil"
        );
    }

    #[test]
    fn failed_reset_keeps_the_saved_identity() {
        let (_directory, path) = settings_file();
        let mut settings = captures_settings::load(&path).unwrap();
        settings.last_screen_permission_request_id = Some("exe:1:2".into());
        captures_settings::write_atomic(&path, &settings).unwrap();
        let mut reset = FakeReset {
            calls: Vec::new(),
            result: Err("tccutil exited with status 1".into()),
        };
        assert_eq!(
            reset_screen_permission(&path, "dev.captures.native", &mut reset),
            Err("tccutil exited with status 1".into())
        );
        assert_eq!(
            captures_settings::load(&path)
                .unwrap()
                .last_screen_permission_request_id
                .as_deref(),
            Some("exe:1:2")
        );
    }
}
