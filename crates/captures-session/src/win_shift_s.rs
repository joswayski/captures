use std::{
    mem::size_of,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
            SendInput, VK_LSHIFT, VK_LWIN, VK_RSHIFT, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW,
            UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

const VK_S: u32 = 0x53;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WinShiftSPhase {
    Pressed,
    Released,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static SWALLOWED_S: AtomicBool = AtomicBool::new(false);
static HOOK: Mutex<HHOOK> = Mutex::new(0);
static HANDLER: OnceLock<Mutex<Option<fn(WinShiftSPhase)>>> = OnceLock::new();

fn handler_slot() -> &'static Mutex<Option<fn(WinShiftSPhase)>> {
    HANDLER.get_or_init(|| Mutex::new(None))
}

pub fn set_handler(handler: Option<fn(WinShiftSPhase)>) {
    if let Ok(mut slot) = handler_slot().lock() {
        *slot = handler;
    }
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
    if !enabled {
        SWALLOWED_S.store(false, Ordering::Release);
    }
}

pub fn ensure_hook() -> Result<(), String> {
    let mut hook = HOOK
        .lock()
        .map_err(|_| "Windows screenshot takeover hook is poisoned".to_owned())?;
    if *hook != 0 {
        return Ok(());
    }
    // SAFETY: WH_KEYBOARD_LL callbacks run in this process. The procedure lives
    // for the process lifetime; we unhook on a best-effort Drop via disable.
    let installed = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(win_shift_s_proc), 0, 0) };
    if installed == 0 {
        return Err("could not install a keyboard hook for Win+Shift+S".to_owned());
    }
    *hook = installed;
    Ok(())
}

#[allow(dead_code)]
pub fn uninstall_hook() {
    if let Ok(mut hook) = HOOK.lock()
        && *hook != 0
    {
        unsafe { UnhookWindowsHookEx(*hook) };
        *hook = 0;
    }
}

extern "system" fn win_shift_s_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ENABLED.load(Ordering::Acquire) && should_swallow(wparam, lparam) {
        return 1;
    }
    unsafe { CallNextHookEx(0, code, wparam, lparam) }
}

fn should_swallow(wparam: WPARAM, lparam: LPARAM) -> bool {
    if lparam == 0 {
        return false;
    }
    let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
    if info.flags & LLKHF_INJECTED != 0 || info.vkCode != VK_S {
        return false;
    }
    let wparam = wparam as u32;
    let is_down = matches!(wparam, WM_KEYDOWN | WM_SYSKEYDOWN);
    let is_up = matches!(wparam, WM_KEYUP | WM_SYSKEYUP);
    if !is_down && !is_up {
        return false;
    }
    if is_down {
        if !win_and_shift_are_down() {
            return false;
        }
        SWALLOWED_S.store(true, Ordering::Release);
        suppress_win_key_start_menu();
        dispatch(WinShiftSPhase::Pressed);
        return true;
    }
    if SWALLOWED_S.swap(false, Ordering::AcqRel) {
        dispatch(WinShiftSPhase::Released);
        return true;
    }
    false
}

fn win_and_shift_are_down() -> bool {
    let win = key_is_down(VK_LWIN as i32) || key_is_down(VK_RWIN as i32);
    let shift = key_is_down(VK_SHIFT as i32)
        || key_is_down(VK_LSHIFT as i32)
        || key_is_down(VK_RSHIFT as i32);
    win && shift
}

fn key_is_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
}

fn dispatch(phase: WinShiftSPhase) {
    let handler = handler_slot().lock().ok().and_then(|slot| *slot);
    if let Some(handler) = handler {
        handler(phase);
    }
}

/// Swallowing Win+Shift+S leaves Explorer thinking Win was unused, so releasing
/// it would open Start. A discarded injected key marks the Win press as a chord.
fn suppress_win_key_start_menu() {
    const VK_UNUSED: u16 = 0xE8;
    let key_down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_UNUSED,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let key_up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_UNUSED,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key_down, key_up];
    // SAFETY: `inputs` is a live two-element keyboard down/up pair. Injected
    // events are ignored by this hook via LLKHF_INJECTED.
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{WinShiftSPhase, win_and_shift_are_down};

    #[test]
    fn phases_are_distinct() {
        assert_ne!(WinShiftSPhase::Pressed, WinShiftSPhase::Released);
        let _ = win_and_shift_are_down();
    }
}
