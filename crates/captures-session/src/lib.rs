//! Reports whether the current interactive desktop session is safe to capture.

mod capture_escape;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "windows", test))]
mod shell_ui;
#[cfg(target_os = "windows")]
mod win_shift_s;
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
/// hidden. Called immediately before a capture snapshot so a shortcut pressed
/// while Start or Search is open does not freeze the menu into the screenshot.
pub fn dismiss_transient_shell_ui_before_capture() {
    #[cfg(target_os = "windows")]
    windows::dismiss_transient_shell_ui_before_capture();
}

/// Phase for the Win+Shift+S interceptor that keeps Snipping Tool from seeing
/// the default region shortcut.
#[cfg(target_os = "windows")]
pub use win_shift_s::WinShiftSPhase;

#[cfg(not(target_os = "windows"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WinShiftSPhase {
    Pressed,
    Released,
}

/// Install a low-level keyboard hook that swallows Win+Shift+S while enabled.
///
/// Explorer/Snipping Tool own that chord before `RegisterHotKey`, so Tauri
/// global shortcuts never receive it. The hook runs first and can hide the key
/// from ScreenClippingHost. No-op on other platforms.
pub fn ensure_win_shift_s_takeover() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return win_shift_s::ensure_hook();
    #[cfg(not(target_os = "windows"))]
    Ok(())
}

pub fn set_win_shift_s_takeover_enabled(enabled: bool) {
    #[cfg(target_os = "windows")]
    win_shift_s::set_enabled(enabled);
    #[cfg(not(target_os = "windows"))]
    let _ = enabled;
}

pub fn set_win_shift_s_handler(handler: Option<fn(WinShiftSPhase)>) {
    #[cfg(target_os = "windows")]
    win_shift_s::set_handler(handler);
    #[cfg(not(target_os = "windows"))]
    let _ = handler;
}

pub use capture_escape::{
    CaptureEscapeUi, capture_escape_arms_on_shortcut_press, capture_escape_may_drop_intent,
    capture_escape_overrides_focus_and_freeze, capture_flow_is_current,
    capture_surface_must_revalidate_after_present, ensure_capture_escape_hook,
    macos_key_code_is_escape, set_capture_escape_enabled, set_capture_escape_handler,
    windows_escape_hook_should_swallow, windows_vk_is_escape,
};

#[cfg(test)]
mod tests {
    use super::dismiss_transient_shell_ui_before_capture;

    #[test]
    fn dismiss_shell_ui_returns_immediately_when_no_flyout_is_open() {
        dismiss_transient_shell_ui_before_capture();
    }
}
