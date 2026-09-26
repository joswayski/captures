//! Native first-run permission bookkeeping. Hosts serialize these calls with
//! settings writes on a worker and retain one session for the process lifetime.
//! Checking access never displays an OS prompt. Restart and window ownership
//! belong to the host, not this service.

use std::{borrow::Cow, path::Path};

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

/// Deserializable so a host can ask for the presentation of a state snapshot
/// (`onboarding_presentation`), e.g. in its own UI tests.
#[derive(Debug, Deserialize, Serialize)]
pub struct State {
    pub platform: Cow<'static, str>,
    pub onboarding_completed: bool,
    pub screen_recording_required: bool,
    pub screen_recording_granted: bool,
    pub screen_recording_can_request: bool,
    pub screen_recording_requested_this_launch: bool,
    pub microphone_granted: bool,
    pub microphone_can_request: bool,
    /// Microphone was requested by this process. Mirrors the shipping setup
    /// window, which offers Open Settings only after the user asked once.
    #[serde(default)]
    pub microphone_requested_this_launch: bool,
}

#[derive(Default)]
pub struct Session {
    screen_requested: bool,
    microphone_requested: bool,
}

impl Session {
    pub const fn new() -> Self {
        Self {
            screen_requested: false,
            microphone_requested: false,
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
            Action::RequestMicrophone => {
                self.microphone_requested = true;
                permissions.request_microphone()?;
            }
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
            platform: Cow::Borrowed(std::env::consts::OS),
            onboarding_completed: settings.onboarding_completed,
            screen_recording_required: identity.is_some(),
            screen_recording_granted: screen_granted,
            screen_recording_can_request: !screen_granted
                && settings.last_screen_permission_request_id.as_deref() != identity,
            screen_recording_requested_this_launch: self.screen_requested,
            microphone_granted,
            microphone_can_request: !microphone_granted && microphone_can_request,
            microphone_requested_this_launch: self.microphone_requested,
        }
    }
}

// Shipping setup copy (apps/desktop/ui/src/Onboarding.tsx). Both native hosts
// render these strings; keep them identical to the Tauri window.
pub const EYEBROW: &str = "Welcome to Captures";
pub const LEDE: &str = "Captures only reads the pixels you choose to capture. Nothing is uploaded, \
and nothing leaves this computer unless you send it somewhere.";
pub const CHECKING: &str = "Checking the access available on this computer…";
pub const SCREEN_TITLE: &str = "Screen capture";
pub const MICROPHONE_TITLE: &str = "Microphone";
pub const OPTIONAL: &str = "Optional";
pub const OPENING: &str = "Opening…";
pub const RESTARTING: &str = "Restarting…";
pub const FINISHING: &str = "Finishing…";
pub const REFRESH: &str = "Refresh status";
pub const START: &str = "Start capturing";
pub const RESTART: &str = "Restart Captures";
const MACOS_TITLE: &str = "Required permissions";
const READY_TITLE: &str = "You’re ready to capture";
/// Permission recovery reuses the setup cards from a completed workspace.
pub const RECOVERY_TITLE: &str = "Capture permissions";
pub const RECOVERY_LEDE: &str = "Your captures and editors stay open. Grant access, refresh status, \
then retry your capture. If your OS requires a restart, save your work before quitting and \
reopening Captures.";
pub const RECOVERY_DONE: &str = "Done";

/// State-independent setup copy, available before the first permission check.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Copy {
    pub eyebrow: &'static str,
    pub title: &'static str,
    pub lede: &'static str,
    pub checking: &'static str,
    pub screen_title: &'static str,
    pub microphone_title: &'static str,
    pub optional: &'static str,
    pub opening: &'static str,
    pub restarting: &'static str,
    pub finishing: &'static str,
    pub refresh: &'static str,
    pub start: &'static str,
    pub recovery_title: &'static str,
    pub recovery_lede: &'static str,
    pub recovery_done: &'static str,
}

pub const fn copy() -> Copy {
    Copy {
        eyebrow: EYEBROW,
        title: setup_title_for_os(),
        lede: LEDE,
        checking: CHECKING,
        screen_title: SCREEN_TITLE,
        microphone_title: MICROPHONE_TITLE,
        optional: OPTIONAL,
        opening: OPENING,
        restarting: RESTARTING,
        finishing: FINISHING,
        refresh: REFRESH,
        start: START,
        recovery_title: RECOVERY_TITLE,
        recovery_lede: RECOVERY_LEDE,
        recovery_done: RECOVERY_DONE,
    }
}

const fn setup_title_for_os() -> &'static str {
    if cfg!(target_os = "macos") {
        MACOS_TITLE
    } else {
        READY_TITLE
    }
}

/// A right-aligned status next to a permission. `ready` pills are positive
/// and carry a check mark; the others are neutral hints beside an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub label: &'static str,
    pub ready: bool,
}

/// Everything the setup surface shows for one state, derived exactly like
/// the shipping React component so both native hosts stay in lockstep.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Presentation {
    pub title: &'static str,
    pub screen_description: &'static str,
    pub screen_status: Option<Status>,
    pub screen_action: Option<&'static str>,
    pub show_microphone: bool,
    pub microphone_description: &'static str,
    pub microphone_status: Option<Status>,
    pub microphone_action: Option<&'static str>,
    /// Screen access is usable; finishing setup is allowed.
    pub screen_ready: bool,
    /// The OS applies the grant only after a relaunch; offer Restart instead.
    pub restart_required: bool,
    pub primary_label: &'static str,
}

pub fn setup_title(platform: &str) -> &'static str {
    if platform == "macos" {
        MACOS_TITLE
    } else {
        READY_TITLE
    }
}

fn screen_description(
    platform: &str,
    required: bool,
    restart_required: bool,
    still_off: bool,
) -> &'static str {
    match platform {
        "macos" if still_off => {
            "The switch for this copy of Captures is still off. A local build is a different row \
             from a downloaded app. Turn it on, then restart."
        }
        "macos" if restart_required => {
            "Turn the switch on next to this copy of Captures, then restart. A local build is a \
             different row from a downloaded app. macOS does not apply the permission until \
             Captures relaunches."
        }
        "macos" => {
            "This allows Captures to read the pixels you choose to capture. macOS keeps everything \
             else hidden."
        }
        "windows" => {
            "Windows provides screen capture access without a separate permission prompt. Secure \
             and protected windows remain private."
        }
        "linux" => {
            "Your desktop may show its own screen-sharing picker when a capture starts. There is \
             nothing to approve ahead of time."
        }
        _ if required => {
            "Allow your operating system to share the part of the screen you choose to capture."
        }
        _ => "Screen capture is available without an additional setup step.",
    }
}

fn microphone_description(granted: bool, asked: bool) -> &'static str {
    if granted {
        "macOS will not ask again. Turn the microphone on when you start a recording."
    } else if asked {
        "Turn Captures on in Microphone settings. macOS only lists apps after they ask."
    } else {
        "Allow it now so a recording does not pause to ask, or wait until you pick a mic."
    }
}

impl State {
    pub fn presentation(&self) -> Presentation {
        let screen_ready = !self.screen_recording_required || self.screen_recording_granted;
        let pending = !screen_ready;
        let restart_required = pending && self.screen_recording_requested_this_launch;
        let still_off = pending
            && !self.screen_recording_can_request
            && !self.screen_recording_requested_this_launch;
        let granted = |label| Status { label, ready: true };
        let hint = |label| Status {
            label,
            ready: false,
        };
        let (screen_status, screen_action) = if screen_ready {
            let label = if self.screen_recording_required {
                "Granted"
            } else {
                "Ready"
            };
            (Some(granted(label)), None)
        } else {
            let status = if restart_required {
                Some(hint("Restart required"))
            } else if still_off {
                Some(hint("Still off"))
            } else {
                None
            };
            let action = if self.screen_recording_can_request {
                "Allow access"
            } else {
                "Open Settings"
            };
            (status, Some(action))
        };
        let asked = self.microphone_requested_this_launch;
        let (microphone_status, microphone_action) = if self.microphone_granted {
            (Some(granted("Granted")), None)
        } else if asked && !self.microphone_can_request {
            (None, Some("Open Settings"))
        } else {
            (None, Some("Allow microphone"))
        };
        Presentation {
            title: setup_title(&self.platform),
            screen_description: screen_description(
                &self.platform,
                self.screen_recording_required,
                restart_required,
                still_off,
            ),
            screen_status,
            screen_action,
            show_microphone: self.platform == "macos",
            microphone_description: microphone_description(self.microphone_granted, asked),
            microphone_status,
            microphone_action,
            screen_ready,
            restart_required,
            primary_label: if restart_required { RESTART } else { START },
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

    fn mac_state() -> State {
        State {
            platform: "macos".into(),
            onboarding_completed: false,
            screen_recording_required: true,
            screen_recording_granted: false,
            screen_recording_can_request: true,
            screen_recording_requested_this_launch: false,
            microphone_granted: false,
            microphone_can_request: true,
            microphone_requested_this_launch: false,
        }
    }

    #[test]
    fn presentation_matches_shipping_macos_setup_states() {
        let mut state = mac_state();
        let fresh = state.presentation();
        assert_eq!(fresh.title, "Required permissions");
        assert!(fresh.screen_description.starts_with("This allows Captures"));
        assert_eq!(fresh.screen_status, None);
        assert_eq!(fresh.screen_action, Some("Allow access"));
        assert!(fresh.show_microphone && !fresh.screen_ready && !fresh.restart_required);
        assert_eq!(fresh.microphone_action, Some("Allow microphone"));
        assert_eq!(fresh.primary_label, "Start capturing");

        state.screen_recording_can_request = false;
        state.screen_recording_requested_this_launch = true;
        let restart = state.presentation();
        assert!(restart.restart_required);
        assert_eq!(restart.primary_label, "Restart Captures");
        assert_eq!(
            restart.screen_status,
            Some(Status {
                label: "Restart required",
                ready: false
            })
        );
        assert_eq!(restart.screen_action, Some("Open Settings"));
        assert!(
            restart
                .screen_description
                .ends_with("macOS does not apply the permission until Captures relaunches.")
        );
        assert!(!restart.screen_description.contains("  "));

        // A relaunch after the prompt: the switch is still off.
        state.screen_recording_requested_this_launch = false;
        let off = state.presentation();
        assert_eq!(off.screen_status.map(|s| s.label), Some("Still off"));
        assert!(
            off.screen_description
                .starts_with("The switch for this copy")
        );
        assert_eq!(off.primary_label, "Start capturing");

        state.screen_recording_granted = true;
        let granted = state.presentation();
        assert!(granted.screen_ready && !granted.restart_required);
        assert_eq!(
            granted.screen_status,
            Some(Status {
                label: "Granted",
                ready: true
            })
        );
        assert_eq!(granted.screen_action, None);
    }

    #[test]
    fn presentation_microphone_offers_settings_only_after_asking() {
        let mut state = mac_state();
        state.microphone_can_request = false;
        assert_eq!(
            state.presentation().microphone_action,
            Some("Allow microphone")
        );
        state.microphone_requested_this_launch = true;
        let asked = state.presentation();
        assert_eq!(asked.microphone_action, Some("Open Settings"));
        assert!(asked.microphone_description.starts_with("Turn Captures on"));
        state.microphone_granted = true;
        let granted = state.presentation();
        assert_eq!(granted.microphone_action, None);
        assert_eq!(granted.microphone_status.map(|s| s.label), Some("Granted"));
        assert!(
            granted
                .microphone_description
                .starts_with("macOS will not ask")
        );
    }

    #[test]
    fn presentation_on_windows_and_linux_is_ready_without_microphone_card() {
        for (platform, start) in [
            ("windows", "Windows provides"),
            ("linux", "Your desktop may"),
        ] {
            let state = State {
                platform: platform.into(),
                screen_recording_required: false,
                screen_recording_granted: true,
                screen_recording_can_request: false,
                microphone_granted: true,
                ..mac_state()
            };
            let view = state.presentation();
            assert_eq!(view.title, "You’re ready to capture");
            assert!(view.screen_description.starts_with(start));
            assert_eq!(
                view.screen_status,
                Some(Status {
                    label: "Ready",
                    ready: true
                })
            );
            assert!(view.screen_ready && !view.show_microphone);
        }
        assert!(LEDE.ends_with("unless you send it somewhere."));
        assert!(!LEDE.contains("  ") && !RECOVERY_LEDE.contains("  "));
    }

    #[test]
    fn microphone_request_is_remembered_for_this_launch_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut permissions = Fake::mac();
        let mut session = Session::new();
        let state = session
            .execute_with(&path, Action::RequestMicrophone, &mut permissions)
            .unwrap();
        assert!(state.microphone_requested_this_launch);
        let state = Session::new()
            .execute_with(&path, Action::Check, &mut permissions)
            .unwrap();
        assert!(!state.microphone_requested_this_launch);
    }
}
