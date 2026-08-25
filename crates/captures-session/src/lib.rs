//! Reports whether the current interactive desktop session is safe to capture.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
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
