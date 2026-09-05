//! Treat OS logoff and shutdown as a clean exit.
//!
//! Captures stays running in the tray by preventing window-close from quitting.
//! Windows then force-kills the process during shutdown, which used to look like
//! a crash because the session-running marker was left behind.

#![allow(unsafe_code)]

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

static INSTALLED: Once = Once::new();
static ENDING: AtomicBool = AtomicBool::new(false);

pub fn install() {
    INSTALLED.call_once(install_platform);
}

pub fn is_ending() -> bool {
    ENDING.load(Ordering::SeqCst) || os_reports_session_ending()
}

#[cfg(windows)]
fn note_ending() {
    ENDING.store(true, Ordering::SeqCst);
    crate::crash_report::mark_clean_exit();
}

#[cfg(not(target_os = "windows"))]
fn os_reports_session_ending() -> bool {
    false
}

#[cfg(not(windows))]
fn install_platform() {
    #[cfg(unix)]
    unix::install();
}

#[cfg(windows)]
fn os_reports_session_ending() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};
    unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

#[cfg(windows)]
fn install_platform() {
    windows::install();
}

#[cfg(unix)]
mod unix {
    use super::ENDING;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;

    static SESSION_MARKER: OnceLock<CString> = OnceLock::new();
    static LAST_PANIC: OnceLock<CString> = OnceLock::new();

    pub(super) fn install() {
        cache_path(&SESSION_MARKER, &crate::crash_report::session_marker_path());
        cache_path(&LAST_PANIC, &crate::crash_report::last_panic_path());
        unsafe {
            register(libc::SIGTERM);
            register(libc::SIGHUP);
            register(libc::SIGINT);
        }
    }

    fn cache_path(slot: &OnceLock<CString>, path: &Path) {
        if let Ok(encoded) = CString::new(path.as_os_str().as_bytes()) {
            let _ = slot.set(encoded);
        }
    }

    unsafe fn register(signum: libc::c_int) {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handle as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signum, &action, std::ptr::null_mut());
        }
    }

    extern "C" fn handle(signum: libc::c_int) {
        ENDING.store(true, Ordering::SeqCst);
        unlink(&SESSION_MARKER);
        unlink(&LAST_PANIC);
        unsafe {
            libc::signal(signum, libc::SIG_DFL);
            libc::raise(signum);
        }
    }

    fn unlink(slot: &OnceLock<CString>) {
        if let Some(path) = slot.get() {
            unsafe {
                libc::unlink(path.as_ptr());
            }
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::note_ending;
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Console::{
                CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
            },
            LibraryLoader::GetModuleHandleW,
            Threading::SetProcessShutdownParameters,
        },
        UI::WindowsAndMessaging::{
            CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, RegisterClassW, WM_ENDSESSION,
            WM_QUERYENDSESSION, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
        },
    };

    const SHUTDOWN_NORETRY: u32 = 1;

    pub(super) fn install() {
        unsafe {
            let _ = SetProcessShutdownParameters(0x280, SHUTDOWN_NORETRY);
            let _ = SetConsoleCtrlHandler(Some(ctrl_handler), 1);
        }
        create_watcher_window();
    }

    fn create_watcher_window() {
        static CLASS_NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
        let class_name = CLASS_NAME.get_or_init(|| wide("CapturesSessionEnd"));
        let window_name = wide("Captures");
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            let _ = RegisterClassW(&class);
            let _hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                window_name.as_ptr(),
                WS_OVERLAPPED,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn ctrl_handler(control: u32) -> windows_sys::core::BOOL {
        if matches!(
            control,
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
        ) {
            note_ending();
            1
        } else {
            0
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_QUERYENDSESSION || message == WM_ENDSESSION {
            note_ending();
            if message == WM_QUERYENDSESSION {
                return 1;
            }
            return 0;
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }
}
