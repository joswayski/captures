//! Shared rules for dismissing Windows Start / Search before a capture.
//!
//! Opening Captures from the Start menu starts a capture immediately. The
//! menu's close animation is slower than that snapshot, so the flyout would
//! otherwise appear in the freeze-frame.

use std::time::Duration;

pub(crate) const SHELL_UI_POLL_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const SHELL_UI_DISMISS_TIMEOUT: Duration = Duration::from_millis(750);
pub(crate) const SHELL_UI_SECOND_ESCAPE_AFTER: Duration = Duration::from_millis(120);
pub(crate) const SHELL_UI_FADE_SETTLE: Duration = Duration::from_millis(200);
pub(crate) const MIN_SHELL_FLYOUT_EDGE: i32 = 200;

const TRANSIENT_SHELL_HOSTS: &[&str] = &[
    "StartMenuExperienceHost.exe",
    "SearchHost.exe",
    "ShellExperienceHost.exe",
];

pub(crate) fn process_is_transient_shell_host(image_path: &str) -> bool {
    image_path.rsplit(['\\', '/']).next().is_some_and(|name| {
        TRANSIENT_SHELL_HOSTS
            .iter()
            .any(|host| name.eq_ignore_ascii_case(host))
    })
}

pub(crate) fn window_qualifies_as_shell_flyout(
    visible: bool,
    cloaked: bool,
    width: i32,
    height: i32,
) -> bool {
    visible && !cloaked && width >= MIN_SHELL_FLYOUT_EDGE && height >= MIN_SHELL_FLYOUT_EDGE
}

pub(crate) fn should_send_second_escape(elapsed: Duration, already_sent: bool) -> bool {
    !already_sent && elapsed >= SHELL_UI_SECOND_ESCAPE_AFTER
}

pub(crate) fn shell_ui_dismiss_timed_out(elapsed: Duration) -> bool {
    elapsed >= SHELL_UI_DISMISS_TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::{
        SHELL_UI_DISMISS_TIMEOUT, SHELL_UI_FADE_SETTLE, SHELL_UI_POLL_INTERVAL,
        SHELL_UI_SECOND_ESCAPE_AFTER, process_is_transient_shell_host, shell_ui_dismiss_timed_out,
        should_send_second_escape, window_qualifies_as_shell_flyout,
    };
    use std::time::Duration;

    #[test]
    fn recognizes_start_and_search_host_processes() {
        assert!(process_is_transient_shell_host(
            r"C:\Windows\SystemApps\Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy\StartMenuExperienceHost.exe"
        ));
        assert!(process_is_transient_shell_host(
            r"C:\Windows\SystemApps\MicrosoftWindows.Client.CBS_cw5n1h2txyewy\SearchHost.exe"
        ));
        assert!(process_is_transient_shell_host(
            r"C:\Windows\SystemApps\ShellExperienceHost_cw5n1h2txyewy\ShellExperienceHost.exe"
        ));
        assert!(process_is_transient_shell_host(
            "/Windows/SystemApps/SearchHost.exe"
        ));
        assert!(!process_is_transient_shell_host(r"C:\Windows\explorer.exe"));
        assert!(!process_is_transient_shell_host(
            r"C:\Program Files\Captures\captures.exe"
        ));
    }

    #[test]
    fn ignores_cloaked_or_tiny_shell_host_windows() {
        assert!(window_qualifies_as_shell_flyout(true, false, 640, 720));
        assert!(!window_qualifies_as_shell_flyout(true, true, 640, 720));
        assert!(!window_qualifies_as_shell_flyout(false, false, 640, 720));
        assert!(!window_qualifies_as_shell_flyout(true, false, 32, 32));
        assert!(!window_qualifies_as_shell_flyout(true, false, 640, 40));
    }

    #[test]
    fn retries_escape_once_then_gives_up() {
        assert!(!should_send_second_escape(Duration::from_millis(50), false));
        assert!(should_send_second_escape(
            SHELL_UI_SECOND_ESCAPE_AFTER,
            false
        ));
        assert!(!should_send_second_escape(
            SHELL_UI_SECOND_ESCAPE_AFTER,
            true
        ));
        assert!(!shell_ui_dismiss_timed_out(Duration::from_millis(100)));
        assert!(shell_ui_dismiss_timed_out(SHELL_UI_DISMISS_TIMEOUT));
        assert!(SHELL_UI_POLL_INTERVAL < SHELL_UI_SECOND_ESCAPE_AFTER);
        assert!(SHELL_UI_SECOND_ESCAPE_AFTER < SHELL_UI_DISMISS_TIMEOUT);
        assert!(SHELL_UI_FADE_SETTLE > Duration::from_millis(40));
    }
}
