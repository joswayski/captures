//! Native first-run permission bookkeeping. Hosts serialize these calls with
//! settings writes on a worker and retain one session for the process lifetime.
//! Checking access never displays an OS prompt. Restart and window ownership
//! belong to the host, not this service.

use std::path::Path;

use captures_settings::AppSettings;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Check,
    RequestScreen,
    RequestMicrophone,
    Complete,
}

#[derive(Debug, Serialize)]
pub struct State {
    pub platform: &'static str,
    pub onboarding_completed: bool,
    pub screen_recording_required: bool,
    pub screen_recording_granted: bool,
    pub screen_recording_can_request: bool,
    pub screen_recording_requested_this_launch: bool,
    pub microphone_granted: bool,
    pub microphone_can_request: bool,
}

#[derive(Default)]
pub struct Session {
    screen_requested: bool,
}

impl Session {
    pub const fn new() -> Self {
        Self {
            screen_requested: false,
        }
    }

    pub fn execute(&mut self, path: &Path, action: Action) -> Result<State, String> {
        self.execute_with(path, action, &mut System)
    }

    fn execute_with(
        &mut self,
        path: &Path,
        action: Action,
        permissions: &mut impl Permissions,
    ) -> Result<State, String> {
        // Fail closed on unreadable/newer settings, rather than replacing them.
        let mut settings = captures_settings::load(path).map_err(|e| e.to_string())?;
        let identity = permissions.screen_identity()?;
        match action {
            Action::Check => {}
            Action::RequestScreen if identity.is_some() && !permissions.screen_granted() => {
                let can_request = settings.last_screen_permission_request_id != identity;
                if can_request {
                    settings
                        .last_screen_permission_request_id
                        .clone_from(&identity);
                    // Persist before requesting: a restart must not repeat the prompt.
                    captures_settings::write_atomic(path, &settings).map_err(|e| e.to_string())?;
                }
                self.screen_requested = true;
                permissions.request_screen(can_request)?;
            }
            Action::RequestScreen => {}
            Action::RequestMicrophone => permissions.request_microphone()?,
            Action::Complete => {
                if identity.is_some() && !permissions.screen_granted() {
                    return Err(
                        "Screen recording access is required before setup can finish.".into(),
                    );
                }
                if !settings.onboarding_completed {
                    settings.onboarding_completed = true;
                    captures_settings::write_atomic(path, &settings).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(self.state(&settings, identity.as_deref(), permissions))
    }

    fn state(
        &self,
        settings: &AppSettings,
        identity: Option<&str>,
        permissions: &impl Permissions,
    ) -> State {
        let screen_granted = identity.is_none() || permissions.screen_granted();
        let (microphone_granted, microphone_can_request) = permissions.microphone_status();
        State {
            platform: std::env::consts::OS,
            onboarding_completed: settings.onboarding_completed,
            screen_recording_required: identity.is_some(),
            screen_recording_granted: screen_granted,
            screen_recording_can_request: !screen_granted
                && settings.last_screen_permission_request_id.as_deref() != identity,
            screen_recording_requested_this_launch: self.screen_requested,
            microphone_granted,
            microphone_can_request: !microphone_granted && microphone_can_request,
        }
    }
}

trait Permissions {
    fn screen_identity(&self) -> Result<Option<String>, String>;
    fn screen_granted(&self) -> bool;
    fn request_screen(&mut self, can_request: bool) -> Result<(), String>;
    fn microphone_status(&self) -> (bool, bool);
    fn request_microphone(&mut self) -> Result<(), String>;
}

struct System;

impl Permissions for System {
    fn screen_identity(&self) -> Result<Option<String>, String> {
        #[cfg(target_os = "macos")]
        {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let metadata = executable.metadata().map_err(|e| e.to_string())?;
            let modified = metadata
                .modified()
                .map_err(|e| e.to_string())?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            Ok(Some(format!(
                "{}:{}:{modified}",
                executable.to_string_lossy(),
                metadata.len()
            )))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(None)
        }
    }

    fn screen_granted(&self) -> bool {
        captures_capture::XcapBackend
            .ensure_permission(false)
            .is_ok()
    }

    fn request_screen(&mut self, can_request: bool) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            use captures_capture::CaptureError;
            match captures_capture::XcapBackend.ensure_permission(can_request) {
                Ok(()) | Err(CaptureError::PermissionRequestStarted) => Ok(()),
                Err(CaptureError::PermissionDenied) => open_privacy_settings("ScreenCapture"),
                Err(error) => Err(error.to_string()),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = can_request;
            Ok(())
        }
    }

    fn microphone_status(&self) -> (bool, bool) {
        #[cfg(target_os = "macos")]
        {
            (
                captures_recording_macos::microphone_authorized(),
                captures_recording_macos::microphone_can_request(),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            (true, false)
        }
    }

    fn request_microphone(&mut self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        if !captures_recording_macos::request_microphone_access()
            && !captures_recording_macos::microphone_can_request()
        {
            return open_privacy_settings("Microphone");
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn open_privacy_settings(pane: &str) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_{pane}"
        ))
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("System Settings could not be opened.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        identity: Option<String>,
        granted: bool,
        microphone: bool,
        requests: Vec<bool>,
        fail_request: bool,
    }
    impl Fake {
        fn mac() -> Self {
            Self {
                identity: Some("executable-a:5:100".into()),
                granted: false,
                microphone: false,
                requests: vec![],
                fail_request: false,
            }
        }
    }
    impl Permissions for Fake {
        fn screen_identity(&self) -> Result<Option<String>, String> {
            Ok(self.identity.clone())
        }
        fn screen_granted(&self) -> bool {
            self.granted
        }
        fn request_screen(&mut self, can_request: bool) -> Result<(), String> {
            self.requests.push(can_request);
            if self.fail_request {
                Err("System Settings unavailable".into())
            } else {
                Ok(())
            }
        }
        fn microphone_status(&self) -> (bool, bool) {
            (self.microphone, false)
        }
        fn request_microphone(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn checks_never_prompt_or_complete_setup_and_denied_completion_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut session = Session::new();
        let mut permissions = Fake::mac();
        for _ in 0..3 {
            let state = session
                .execute_with(&path, Action::Check, &mut permissions)
                .unwrap();
            assert!(state.screen_recording_can_request);
            assert!(!state.onboarding_completed);
            assert!(!state.screen_recording_requested_this_launch);
        }
        assert!(
            session
                .execute_with(&path, Action::Complete, &mut permissions)
                .is_err()
        );
        assert!(permissions.requests.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn prompt_identity_survives_restart_but_launch_flag_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut permissions = Fake::mac();
        let mut session = Session::new();
        let state = session
            .execute_with(&path, Action::RequestScreen, &mut permissions)
            .unwrap();
        assert!(state.screen_recording_requested_this_launch);
        assert!(!state.screen_recording_can_request);
        let mut restarted = Session::new();
        let state = restarted
            .execute_with(&path, Action::Check, &mut permissions)
            .unwrap();
        assert!(!state.screen_recording_can_request);
        assert!(!state.screen_recording_requested_this_launch);
        restarted
            .execute_with(&path, Action::RequestScreen, &mut permissions)
            .unwrap();
        permissions.identity = Some("executable-b:7:300".into());
        restarted
            .execute_with(&path, Action::RequestScreen, &mut permissions)
            .unwrap();
        assert_eq!(permissions.requests, [true, false, true]);
    }

    #[test]
    fn completion_requires_screen_not_microphone_and_preserves_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = AppSettings {
            theme: captures_settings::ColorTheme::Violet,
            output_directory: "/unusual/output".into(),
            ..AppSettings::default()
        };
        captures_settings::write_atomic(&path, &settings).unwrap();
        let mut permissions = Fake::mac();
        permissions.granted = true;
        let state = Session::new()
            .execute_with(&path, Action::Complete, &mut permissions)
            .unwrap();
        assert!(state.onboarding_completed);
        assert!(!state.microphone_granted);
        // An older Preferences snapshot must not undo trusted completion.
        let saved = captures_settings::save(&path, &settings).unwrap();
        assert!(saved.onboarding_completed);
        assert_eq!(saved.output_directory, "/unusual/output");
        assert_eq!(saved.theme, captures_settings::ColorTheme::Violet);
        assert!(permissions.requests.is_empty());
    }

    #[test]
    fn non_macos_has_no_upfront_prompt_and_does_not_claim_capture_backend_support() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut permissions = Fake::mac();
        permissions.identity = None;
        let state = Session::new()
            .execute_with(&path, Action::Complete, &mut permissions)
            .unwrap();
        assert!(!state.screen_recording_required);
        assert!(!state.screen_recording_can_request);
        assert!(state.onboarding_completed);
        assert!(permissions.requests.is_empty());
    }

    #[test]
    fn failed_request_retains_marker_and_failed_load_has_no_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut permissions = Fake::mac();
        permissions.fail_request = true;
        let mut session = Session::new();
        assert!(
            session
                .execute_with(&path, Action::RequestScreen, &mut permissions)
                .is_err()
        );
        assert_eq!(
            captures_settings::load(&path)
                .unwrap()
                .last_screen_permission_request_id,
            permissions.identity
        );
        std::fs::write(&path, "not settings").unwrap();
        assert!(
            session
                .execute_with(&path, Action::Complete, &mut permissions)
                .is_err()
        );
        assert!(
            session
                .execute_with(&path, Action::RequestScreen, &mut permissions)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not settings");
        assert_eq!(permissions.requests, [true]);
    }
}
