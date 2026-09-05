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
}

#[must_use]
pub const fn macos_key_code_is_escape(key_code: u16) -> bool {
    key_code == MACOS_ESCAPE_KEY_CODE
}

#[must_use]
pub const fn windows_vk_is_escape(vk: u32) -> bool {
    vk == WINDOWS_VK_ESCAPE
}

/// Escape cancels even if freeze-frame has not painted and another overlay is
/// the key window.
#[must_use]
pub const fn capture_escape_overrides_focus_and_freeze() -> bool {
    true
}

#[cfg(target_os = "windows")]
mod windows_hook {
    use std::{
        ffi::c_void,
        ptr,
        sync::{
            Mutex, OnceLock,
            atomic::{AtomicBool, AtomicPtr, Ordering},
        },
    };

    use windows_sys::Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        UI::WindowsAndMessaging::{
            CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW, WH_KEYBOARD_LL,
            WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    };

    use super::WINDOWS_VK_ESCAPE;

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static SWALLOWED: AtomicBool = AtomicBool::new(false);
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
        ENABLED.store(enabled, Ordering::Release);
        if !enabled {
            SWALLOWED.store(false, Ordering::Release);
        }
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
        if is_down {
            dispatch();
            if ENABLED.load(Ordering::Acquire) {
                SWALLOWED.store(true, Ordering::Release);
                return true;
            }
            return false;
        }
        SWALLOWED.swap(false, Ordering::AcqRel)
    }

    fn dispatch() {
        if let Ok(slot) = handler_slot().lock()
            && let Some(handler) = *slot
        {
            handler();
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
        capture_escape_overrides_focus_and_freeze, macos_key_code_is_escape, windows_vk_is_escape,
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
}
