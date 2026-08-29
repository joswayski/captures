//! Reports whether the current interactive desktop session is safe to capture.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "windows", test))]
mod shell_ui;
#[cfg(target_os = "windows")]
mod windows;

/// Returns whether the process belongs to an active, unlocked desktop session.
pub fn capture_session_available() -> bool {
    #[cfg(target_os = "linux")]
    return linux::capture_session_available();

    #[cfg(target_os = "macos")]
    return macos::capture_session_available();

    #[cfg(target_os = "windows")]
    return windows::capture_session_available();

    #[allow(unreachable_code)]
    false
}

/// Closes Start / Search if they are on screen, then waits for the fade.
///
/// No-op on platforms that do not show those flyouts, and when they are already
/// hidden. Called immediately before a capture snapshot so launching Captures
/// from the Windows Start menu does not freeze the menu into the screenshot.
pub fn dismiss_transient_shell_ui_before_capture() {
    #[cfg(target_os = "windows")]
    windows::dismiss_transient_shell_ui_before_capture();
}

#[cfg(test)]
mod tests {
    use super::dismiss_transient_shell_ui_before_capture;

    #[test]
    fn dismiss_shell_ui_returns_immediately_when_no_flyout_is_open() {
        dismiss_transient_shell_ui_before_capture();
    }
}
