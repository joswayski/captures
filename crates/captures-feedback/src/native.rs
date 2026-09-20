//! Native-host adapter. Context is local and cached; only `submit` can send.
//! Call both operations on a worker. Neither inspects captures or diagnostics.

use std::sync::OnceLock;

use crate::{DEFAULT_FEEDBACK_URL, FeedbackClient, FeedbackContext, FeedbackDraft};

pub fn context() -> FeedbackContext {
    static CONTEXT: OnceLock<FeedbackContext> = OnceLock::new();
    CONTEXT
        .get_or_init(|| FeedbackContext {
            app_version: format!("{}-native", env!("CARGO_PKG_VERSION")),
            os: std::env::consts::OS.into(),
            os_version: os_version(),
            arch: std::env::consts::ARCH.into(),
        })
        .clone()
}

/// One client per process preserves cooldown across closed/reopened forms.
pub fn submit(draft: FeedbackDraft) -> Result<(), String> {
    static CLIENT: OnceLock<Result<FeedbackClient, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| FeedbackClient::new(DEFAULT_FEEDBACK_URL))
        .as_ref()
        .map_err(Clone::clone)?
        .submit(draft, context())
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().chars().take(128).collect())
            .unwrap_or_default()
    }
    #[cfg(target_os = "windows")]
    {
        // Match the shipping host's stable OS label, not a device/user name.
        "Windows".into()
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("PRETTY_NAME=")
                        .map(|value| value.trim().trim_matches('"').chars().take(128).collect())
                })
            })
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_context_matches_submission_context_without_private_fields() {
        let first = serde_json::to_value(context()).unwrap();
        assert_eq!(first, serde_json::to_value(context()).unwrap());
        assert_eq!(first.as_object().unwrap().len(), 4);
        assert_eq!(first["os"], std::env::consts::OS);
        assert_eq!(first["arch"], std::env::consts::ARCH);
        assert!(first["app_version"].as_str().unwrap().ends_with("-native"));
        assert!(first["os_version"].as_str().unwrap().chars().count() <= 128);
    }
}
