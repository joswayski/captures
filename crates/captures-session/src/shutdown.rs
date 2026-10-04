//! Marks an elected process's crash evidence clean during normal OS shutdown.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

static STATE: OnceLock<State> = OnceLock::new();
const DISARMED: u8 = 0;
const ARMED: u8 = 1;
const CALLBACK: u8 = 2;

struct State {
    clean_paths: [PathBuf; 2],
    ownership: AtomicU8,
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    resume: Box<dyn Fn() + Send + Sync>,
}

impl State {
    #[cfg(any(target_os = "windows", test))]
    fn clean_if_armed(&self) {
        if self.begin_callback() {
            for path in &self.clean_paths {
                // Missing files are already clean. Other failures retain evidence.
                let _ = std::fs::remove_file(path);
            }
            self.end_callback();
        }
    }

    #[cfg(any(target_os = "windows", test))]
    fn cancel_if_armed(&self) {
        if self.begin_callback() {
            (self.resume)();
            self.end_callback();
        }
    }

    fn begin_callback(&self) -> bool {
        self.ownership
            .compare_exchange(ARMED, CALLBACK, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn end_callback(&self) {
        let _ =
            self.ownership
                .compare_exchange(CALLBACK, ARMED, Ordering::Release, Ordering::Relaxed);
    }

    fn disarm(&self) {
        loop {
            match self.ownership.compare_exchange(
                ARMED,
                DISARMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) | Err(DISARMED) => return,
                Err(CALLBACK) => std::hint::spin_loop(),
                Err(_) => unreachable!("invalid shutdown ownership state"),
            }
        }
    }
}

/// Process-local OS shutdown registration for the elected primary instance.
///
/// Installation may be attempted only once. `disarm` must be called while the
/// instance lock is still held; `rearm` is only valid after winning election
/// again with the same marker paths.
pub struct ShutdownMonitor {
    _private: (),
}

impl ShutdownMonitor {
    pub fn install(
        clean_paths: [PathBuf; 2],
        resume: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        STATE
            .set(State {
                clean_paths,
                ownership: AtomicU8::new(ARMED),
                resume: Box::new(resume),
            })
            .map_err(|_| "OS shutdown monitor is already installed".to_owned())?;

        if let Err(error) = platform::install() {
            STATE.get().expect("state was just installed").disarm();
            return Err(error);
        }
        Ok(Self { _private: () })
    }

    pub fn disarm(&self) {
        state().disarm();
    }

    pub fn rearm(&self) {
        state().ownership.store(ARMED, Ordering::Release);
    }
}

fn state() -> &'static State {
    STATE
        .get()
        .expect("ShutdownMonitor used before installation")
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionMessage {
    Query,
    Confirmed,
    Cancelled,
}

#[cfg(any(target_os = "windows", test))]
const WM_QUERYENDSESSION: u32 = 0x0011;
#[cfg(any(target_os = "windows", test))]
const WM_ENDSESSION: u32 = 0x0016;

#[cfg(any(target_os = "windows", test))]
fn classify_session_message(message: u32, wparam: usize) -> Option<SessionMessage> {
    match message {
        WM_QUERYENDSESSION => Some(SessionMessage::Query),
        WM_ENDSESSION if wparam == 0 => Some(SessionMessage::Cancelled),
        WM_ENDSESSION => Some(SessionMessage::Confirmed),
        _ => None,
    }
}

#[cfg(unix)]
mod platform {
    use super::{Ordering, state};
    use std::cell::UnsafeCell;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::AtomicPtr;

    static PATHS: [AtomicPtr<libc::c_char>; 2] = [
        AtomicPtr::new(std::ptr::null_mut()),
        AtomicPtr::new(std::ptr::null_mut()),
    ];

    struct Previous(UnsafeCell<[libc::sigaction; 3]>);
    // Written before handlers are registered, then read only by a handler.
    unsafe impl Sync for Previous {}
    static PREVIOUS: Previous = Previous(UnsafeCell::new(unsafe { std::mem::zeroed() }));
    const SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGHUP, libc::SIGINT];

    pub(super) fn install() -> Result<(), String> {
        for (slot, path) in PATHS.iter().zip(&state().clean_paths) {
            let path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| format!("shutdown marker path contains NUL: {}", path.display()))?;
            slot.store(path.into_raw(), Ordering::Release);
        }

        for (index, signal) in SIGNALS.into_iter().enumerate() {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = handler as *const () as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            // Publish the immutable old disposition BEFORE enabling our handler;
            // never let a handler read a snapshot the kernel is still writing.
            let mut result = unsafe {
                libc::sigaction(
                    signal,
                    std::ptr::null(),
                    &raw mut (*PREVIOUS.0.get())[index],
                )
            };
            if result == 0 {
                result = unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
            }
            if result != 0 {
                let error = std::io::Error::last_os_error();
                for (restore_index, signal) in SIGNALS.iter().enumerate().take(index) {
                    unsafe {
                        libc::sigaction(
                            *signal,
                            &(*PREVIOUS.0.get())[restore_index],
                            std::ptr::null_mut(),
                        );
                    }
                }
                return Err(format!(
                    "failed to register shutdown signal {signal}: {error}"
                ));
            }
        }
        Ok(())
    }

    extern "C" fn handler(signal: libc::c_int) {
        if state().begin_callback() {
            for path in &PATHS {
                let path = path.load(Ordering::Acquire);
                if !path.is_null() {
                    unsafe { libc::unlink(path) };
                }
            }
            state().end_callback();
        }

        let index = match signal {
            libc::SIGTERM => Some(0),
            libc::SIGHUP => Some(1),
            libc::SIGINT => Some(2),
            _ => None,
        };
        if let Some(index) = index {
            unsafe {
                libc::sigaction(signal, &(*PREVIOUS.0.get())[index], std::ptr::null_mut());
                libc::raise(signal);
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{SessionMessage, classify_session_message, state};
    use std::ptr;
    use std::sync::OnceLock;
    use windows_sys::Win32::{
        Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Console::{
                CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
            },
            LibraryLoader::GetModuleHandleW,
            Threading::SetProcessShutdownParameters,
        },
        UI::WindowsAndMessaging::{
            CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, RegisterClassW, WNDCLASSW,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
        },
    };

    const SHUTDOWN_NORETRY: u32 = 1;

    pub(super) fn install() -> Result<(), String> {
        unsafe {
            if SetProcessShutdownParameters(0x280, SHUTDOWN_NORETRY) == 0 {
                return Err(last_error("SetProcessShutdownParameters"));
            }
            if SetConsoleCtrlHandler(Some(console_handler), 1) == 0 {
                return Err(last_error("SetConsoleCtrlHandler"));
            }
        }
        if let Err(error) = create_watcher_window() {
            unsafe { SetConsoleCtrlHandler(Some(console_handler), 0) };
            return Err(error);
        }
        Ok(())
    }

    fn create_watcher_window() -> Result<(), String> {
        static CLASS_NAME: OnceLock<Vec<u16>> = OnceLock::new();
        let class_name = CLASS_NAME.get_or_init(|| wide("CapturesShutdownMonitor"));
        let window_name = wide("Captures shutdown monitor");
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(last_error("RegisterClassW"));
            }
            let window = CreateWindowExW(
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
            if window.is_null() {
                return Err(last_error("CreateWindowExW"));
            }
        }
        Ok(())
    }

    unsafe extern "system" fn console_handler(control: u32) -> i32 {
        if matches!(
            control,
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
        ) {
            state().clean_if_armed();
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
        match classify_session_message(message, wparam) {
            Some(SessionMessage::Query | SessionMessage::Confirmed) => {
                state().clean_if_armed();
                if message == super::WM_QUERYENDSESSION {
                    1
                } else {
                    0
                }
            }
            Some(SessionMessage::Cancelled) => {
                state().cancel_if_armed();
                0
            }
            None => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn last_error(operation: &str) -> String {
        format!("{operation} failed with OS error {}", unsafe {
            GetLastError()
        })
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
mod platform {
    pub(super) fn install() -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn classifies_query_confirmation_and_cancellation() {
        assert_eq!(
            classify_session_message(WM_QUERYENDSESSION, 0),
            Some(SessionMessage::Query)
        );
        assert_eq!(
            classify_session_message(WM_ENDSESSION, 1),
            Some(SessionMessage::Confirmed)
        );
        assert_eq!(
            classify_session_message(WM_ENDSESSION, 0),
            Some(SessionMessage::Cancelled)
        );
        assert_eq!(classify_session_message(0, 0), None);
    }

    #[test]
    fn disarmed_callbacks_preserve_replacement_markers_and_do_not_resume() {
        let directory = tempfile::tempdir().unwrap();
        let paths = [
            directory.path().join("session"),
            directory.path().join("panic"),
        ];
        for path in &paths {
            std::fs::write(path, b"replacement").unwrap();
        }
        let resumed = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&resumed);
        let state = State {
            clean_paths: paths.clone(),
            ownership: AtomicU8::new(DISARMED),
            resume: Box::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
            }),
        };
        state.clean_if_armed();
        state.cancel_if_armed();
        assert!(paths.iter().all(|path| path.exists()));
        assert_eq!(resumed.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancellation_resumes_only_an_armed_owner() {
        let resumed = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&resumed);
        let state = State {
            clean_paths: [PathBuf::new(), PathBuf::new()],
            ownership: AtomicU8::new(ARMED),
            resume: Box::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
            }),
        };
        state.cancel_if_armed();
        assert_eq!(resumed.load(Ordering::Relaxed), 1);
    }
}
