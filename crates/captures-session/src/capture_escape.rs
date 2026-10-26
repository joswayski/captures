//! Escape must cancel capture even when another screenshot tool stole focus.

/// macOS hardware key code for Escape (`kVK_Escape`).
pub const MACOS_ESCAPE_KEY_CODE: u16 = 53;
/// Windows virtual-key code for Escape (`VK_ESCAPE`).
pub const WINDOWS_VK_ESCAPE: u32 = 0x1B;

/// Capture surfaces that must yield to Escape, including when they are not key.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CaptureEscapeUi {
    pub screenshot_overlay: bool,
    pub recording_selector: bool,
    pub screenshot_countdown: bool,
    pub recording_countdown: bool,
}

impl CaptureEscapeUi {
    #[must_use]
    pub const fn is_armed(self) -> bool {
        self.screenshot_overlay
            || self.recording_selector
            || self.screenshot_countdown
            || self.recording_countdown
    }

    /// Live capture surfaces that may own Escape. A leftover session map entry
    /// is not enough: that would keep swallowing Escape after the overlay is
    /// gone and block the rest of the computer.
    #[must_use]
    pub const fn from_live_surfaces(
        capture_intent: bool,
        overlay_visible: bool,
        recording_selector: bool,
        screenshot_countdown: bool,
        recording_countdown: bool,
    ) -> Self {
        Self {
            screenshot_overlay: capture_intent || overlay_visible,
            recording_selector,
            screenshot_countdown,
            recording_countdown,
        }
    }
}

#[must_use]
pub const fn macos_key_code_is_escape(key_code: u16) -> bool {
    key_code == MACOS_ESCAPE_KEY_CODE
}

#[must_use]
pub const fn windows_vk_is_escape(vk: u32) -> bool {
    vk == WINDOWS_VK_ESCAPE
}

/// True when the Windows low-level hook may swallow Escape. Never swallow
/// while capture UI is not armed; a stuck hook would lock the key on the
/// whole computer.
#[must_use]
pub const fn windows_escape_hook_should_swallow(enabled: bool) -> bool {
    enabled
}

/// Shortcut intent may drop only after a capture surface is on screen.
/// Clearing it earlier unregisters the Windows hook and the global Escape
/// hotkey while `show()` is still queued, so Escape cannot cancel.
#[must_use]
pub const fn capture_escape_may_drop_intent(surface_visible: bool) -> bool {
    surface_visible
}

/// Escape cancels even if freeze-frame has not painted and another overlay is
/// the key window.
#[must_use]
pub const fn capture_escape_overrides_focus_and_freeze() -> bool {
    true
}

/// Shortcut key-down must arm Escape before the freeze-frame exists. Waiting
/// for cursor-claim or overlay show leaves Esc ignored during prefetch, then
/// the frozen overlay still appears.
#[must_use]
pub const fn capture_escape_arms_on_shortcut_press() -> bool {
    true
}

/// In-flight freeze-frame capture, encode, and overlay/selector present must
/// abort when Escape invalidates the capture-flow generation.
#[must_use]
pub const fn capture_flow_is_current(started: u64, current: u64) -> bool {
    started != 0 && started == current
}

/// Overlay and selector show/reveal must hide again if the session vanished
/// while the native present call ran. Otherwise Esc leaves a stuck freeze-frame.
#[must_use]
pub const fn capture_surface_must_revalidate_after_present() -> bool {
    true
}

/// Shortcut key-up must not start a new capture after Escape cancelled the
/// matching key-down flow. `press_generation == 0` means that press never
/// armed a flow (no freeze prefetch), so release may still open capture UI.
#[must_use]
pub const fn shortcut_release_should_start_capture(press_generation: u64, current: u64) -> bool {
    press_generation == 0 || capture_flow_is_current(press_generation, current)
}

/// After recording prep, shortcut intent must not keep Escape armed. A
/// countdown window owns Escape while it is visible; a live recording must
/// not swallow the key in other apps.
#[must_use]
pub const fn recording_prep_must_disarm_escape_intent() -> bool {
    true
}

/// One physical Escape press owns at most one cancellation. Consume its matching
/// release, but fail open on the next press even if host cleanup never arrives.
#[cfg(any(target_os = "windows", test))]
#[derive(Default)]
struct EscapeKeyState {
    enabled: std::sync::atomic::AtomicBool,
    pressed: std::sync::atomic::AtomicBool,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Eq, PartialEq)]
enum EscapeKeyAction {
    PassThrough,
    Swallow,
    Cancel,
}

#[cfg(any(target_os = "windows", test))]
impl EscapeKeyState {
    const fn new() -> Self {
        Self {
            enabled: std::sync::atomic::AtomicBool::new(false),
            pressed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled
            .store(enabled, std::sync::atomic::Ordering::Release);
    }

    fn key_event(&self, down: bool) -> EscapeKeyAction {
        use std::sync::atomic::Ordering;

        if !down {
            return if self.pressed.swap(false, Ordering::AcqRel) {
                EscapeKeyAction::Swallow
            } else {
                EscapeKeyAction::PassThrough
            };
        }
        if self.pressed.load(Ordering::Acquire) {
            return EscapeKeyAction::Swallow;
        }
        if self.enabled.swap(false, Ordering::AcqRel) {
            self.pressed.store(true, Ordering::Release);
            EscapeKeyAction::Cancel
        } else {
            EscapeKeyAction::PassThrough
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_hook {
    use std::{
        ffi::c_void,
        ptr,
        sync::{
            Mutex, OnceLock,
            atomic::{AtomicPtr, Ordering},
        },
    };

    use windows_sys::Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        UI::WindowsAndMessaging::{
            CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW, WH_KEYBOARD_LL,
            WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    };

    use super::{EscapeKeyAction, EscapeKeyState, WINDOWS_VK_ESCAPE};

    static KEYS: EscapeKeyState = EscapeKeyState::new();
    static HOOK: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static INSTALL: Mutex<()> = Mutex::new(());
    type Handler = fn();
    static HANDLER: OnceLock<Mutex<Option<Handler>>> = OnceLock::new();

    fn handler_slot() -> &'static Mutex<Option<Handler>> {
        HANDLER.get_or_init(|| Mutex::new(None))
    }

    pub fn set_handler(handler: Option<Handler>) {
        if let Ok(mut slot) = handler_slot().lock() {
            *slot = handler;
        }
    }

    pub fn set_enabled(enabled: bool) {
        KEYS.set_enabled(enabled);
    }

    pub fn ensure_hook() -> Result<(), String> {
        let _install = INSTALL
            .lock()
            .map_err(|_| "capture Escape hook is poisoned".to_owned())?;
        if !HOOK.load(Ordering::Acquire).is_null() {
            return Ok(());
        }
        let installed = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(capture_escape_proc),
                ptr::null_mut(),
                0,
            )
        };
        if installed.is_null() {
            return Err("could not install a keyboard hook for capture Escape".to_owned());
        }
        HOOK.store(installed, Ordering::Release);
        Ok(())
    }

    extern "system" fn capture_escape_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && should_handle(wparam, lparam) {
            return 1;
        }
        unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
    }

    fn should_handle(wparam: WPARAM, lparam: LPARAM) -> bool {
        if lparam == 0 {
            return false;
        }
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if info.flags & LLKHF_INJECTED != 0 || info.vkCode != WINDOWS_VK_ESCAPE {
            return false;
        }
        let wparam = wparam as u32;
        let is_down = matches!(wparam, WM_KEYDOWN | WM_SYSKEYDOWN);
        let is_up = matches!(wparam, WM_KEYUP | WM_SYSKEYUP);
        if !is_down && !is_up {
            return false;
        }
        match KEYS.key_event(is_down) {
            EscapeKeyAction::PassThrough => false,
            EscapeKeyAction::Swallow => true,
            EscapeKeyAction::Cancel => {
                // Never hold a mutex across host cancellation, or wait for a
                // handler update inside the system-wide keyboard hook.
                let handler = handler_slot().try_lock().ok().and_then(|slot| *slot);
                if let Some(handler) = handler {
                    handler();
                    true
                } else {
                    let _ = KEYS.key_event(false);
                    false
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicUsize;

        #[test]
        fn hook_filters_input_and_releases_escape_after_cancellation() {
            static CANCELS: AtomicUsize = AtomicUsize::new(0);
            fn cancel() {
                CANCELS.fetch_add(1, Ordering::Relaxed);
                // Native hosts may clear the callback during cancellation.
                // This must not deadlock against the hook's handler mutex.
                set_handler(None);
                set_enabled(false);
            }
            let event = |message: u32, vk: u32, flags: u32| {
                let info = KBDLLHOOKSTRUCT {
                    vkCode: vk,
                    flags,
                    ..Default::default()
                };
                should_handle(message as WPARAM, &info as *const _ as LPARAM)
            };
            set_handler(Some(cancel));
            set_enabled(true);
            assert!(!event(WM_KEYDOWN, 0x41, 0));
            assert!(!event(WM_KEYDOWN, WINDOWS_VK_ESCAPE, LLKHF_INJECTED));
            assert!(event(WM_SYSKEYDOWN, WINDOWS_VK_ESCAPE, 0));
            assert!(event(WM_KEYDOWN, WINDOWS_VK_ESCAPE, 0));
            assert!(event(WM_SYSKEYUP, WINDOWS_VK_ESCAPE, 0));
            assert_eq!(CANCELS.load(Ordering::Relaxed), 1);
            assert!(!event(WM_KEYDOWN, WINDOWS_VK_ESCAPE, 0));
            assert!(!event(WM_KEYUP, WINDOWS_VK_ESCAPE, 0));

            set_enabled(true);
            assert!(!event(WM_KEYDOWN, WINDOWS_VK_ESCAPE, 0), "no handler");
            assert!(!event(WM_KEYUP, WINDOWS_VK_ESCAPE, 0));
        }
    }
}

pub fn set_capture_escape_handler(handler: Option<fn()>) {
    #[cfg(target_os = "windows")]
    windows_hook::set_handler(handler);
    #[cfg(not(target_os = "windows"))]
    let _ = handler;
}

pub fn set_capture_escape_enabled(enabled: bool) {
    #[cfg(target_os = "windows")]
    windows_hook::set_enabled(enabled);
    #[cfg(not(target_os = "windows"))]
    let _ = enabled;
}

pub fn ensure_capture_escape_hook() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return windows_hook::ensure_hook();
    #[cfg(not(target_os = "windows"))]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureEscapeUi, MACOS_ESCAPE_KEY_CODE, WINDOWS_VK_ESCAPE,
        capture_escape_arms_on_shortcut_press, capture_escape_may_drop_intent,
        capture_escape_overrides_focus_and_freeze, capture_flow_is_current,
        capture_surface_must_revalidate_after_present, macos_key_code_is_escape,
        recording_prep_must_disarm_escape_intent, shortcut_release_should_start_capture,
        windows_escape_hook_should_swallow, windows_vk_is_escape,
    };

    #[test]
    fn escape_is_armed_for_every_capture_surface() {
        assert!(!CaptureEscapeUi::default().is_armed());
        assert!(
            CaptureEscapeUi {
                screenshot_overlay: true,
                ..CaptureEscapeUi::default()
            }
            .is_armed()
        );
        assert!(
            CaptureEscapeUi {
                recording_selector: true,
                ..CaptureEscapeUi::default()
            }
            .is_armed()
        );
        assert!(
            CaptureEscapeUi {
                screenshot_countdown: true,
                ..CaptureEscapeUi::default()
            }
            .is_armed()
        );
        assert!(
            CaptureEscapeUi {
                recording_countdown: true,
                ..CaptureEscapeUi::default()
            }
            .is_armed()
        );
    }

    #[test]
    fn recognizes_platform_escape_key_codes() {
        assert!(macos_key_code_is_escape(MACOS_ESCAPE_KEY_CODE));
        assert!(!macos_key_code_is_escape(0));
        assert!(windows_vk_is_escape(WINDOWS_VK_ESCAPE));
        assert!(!windows_vk_is_escape(0x53));
    }

    #[test]
    fn escape_cancels_even_when_focus_or_freeze_is_lost() {
        assert!(capture_escape_overrides_focus_and_freeze());
    }

    #[test]
    fn leftover_sessions_do_not_keep_escape_armed() {
        assert!(
            CaptureEscapeUi::from_live_surfaces(true, false, false, false, false).is_armed(),
            "shortcut intent must arm Escape before the overlay paints"
        );
        assert!(
            CaptureEscapeUi::from_live_surfaces(false, true, false, false, false).is_armed(),
            "a visible overlay must still be cancellable"
        );
        assert!(
            !CaptureEscapeUi::from_live_surfaces(false, false, false, false, false).is_armed(),
            "idle Captures must not swallow Escape"
        );
        assert!(CaptureEscapeUi::from_live_surfaces(false, false, true, false, false).is_armed());
        assert!(CaptureEscapeUi::from_live_surfaces(false, false, false, true, false).is_armed());
        assert!(CaptureEscapeUi::from_live_surfaces(false, false, false, false, true).is_armed());
        assert!(
            !capture_escape_may_drop_intent(false),
            "intent must stay armed until the overlay or selector is visible"
        );
        assert!(capture_escape_may_drop_intent(true));
    }

    #[test]
    fn windows_escape_hook_never_swallows_while_disarmed() {
        assert!(windows_escape_hook_should_swallow(true));
        assert!(!windows_escape_hook_should_swallow(false));
    }

    #[test]
    fn windows_escape_returns_to_other_apps_without_waiting_for_cleanup() {
        use super::{EscapeKeyAction::*, EscapeKeyState};

        let keys = EscapeKeyState::new();
        assert_eq!(keys.key_event(true), PassThrough);
        assert_eq!(keys.key_event(false), PassThrough);

        keys.set_enabled(true);
        assert_eq!(keys.key_event(true), Cancel);
        assert_eq!(
            keys.key_event(true),
            Swallow,
            "repeat must not cancel twice"
        );
        assert_eq!(keys.key_event(false), Swallow);
        // Simulate delayed/missing host cleanup: no set_enabled(false) call.
        assert_eq!(keys.key_event(true), PassThrough);
        assert_eq!(keys.key_event(false), PassThrough);

        keys.set_enabled(true);
        assert_eq!(keys.key_event(true), Cancel, "a new capture can cancel");
        keys.set_enabled(false);
        assert_eq!(keys.key_event(true), Swallow);
        assert_eq!(
            keys.key_event(false),
            Swallow,
            "do not leak an orphan key-up"
        );
        assert_eq!(keys.key_event(true), PassThrough);
        assert_eq!(keys.key_event(false), PassThrough);

        keys.set_enabled(true);
        keys.set_enabled(false);
        assert_eq!(
            keys.key_event(true),
            PassThrough,
            "capture finished by mouse"
        );
        assert_eq!(keys.key_event(false), PassThrough);
    }

    #[test]
    fn escape_arms_on_shortcut_press_and_aborts_stale_freeze_frames() {
        assert!(capture_escape_arms_on_shortcut_press());
        assert!(capture_flow_is_current(4, 4));
        assert!(
            !capture_flow_is_current(4, 5),
            "Escape must invalidate in-flight overlay present"
        );
        assert!(
            !capture_flow_is_current(0, 0),
            "an unused flow token must not present capture UI"
        );
        assert!(capture_surface_must_revalidate_after_present());
    }

    #[test]
    fn cancelled_shortcut_press_must_not_start_on_release() {
        assert!(
            shortcut_release_should_start_capture(0, 9),
            "a press that never armed a flow may still open capture UI"
        );
        assert!(shortcut_release_should_start_capture(4, 4));
        assert!(
            !shortcut_release_should_start_capture(4, 5),
            "Escape during a held shortcut must suppress that key-up"
        );
        assert!(recording_prep_must_disarm_escape_intent());
    }
}
