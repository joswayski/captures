use std::{cell::Cell, ffi::c_void, ptr, thread, time::Instant};

use windows_sys::{
    Win32::{
        Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            },
            RemoteDesktop::{
                WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSActive, WTSConnectState,
                WTSFreeMemory, WTSQuerySessionInformationW,
            },
            StationsAndDesktops::{
                CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop,
                UOI_NAME,
            },
            Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_ESCAPE,
            },
            WindowsAndMessaging::{
                EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            },
        },
    },
    core::GUID,
};

use crate::shell_ui::{
    SHELL_UI_FADE_SETTLE, SHELL_UI_POLL_INTERVAL, process_is_transient_shell_host,
    shell_ui_dismiss_timed_out, should_send_second_escape, window_qualifies_as_shell_flyout,
};

const CLSID_APP_VISIBILITY: GUID = GUID {
    data1: 0x7E5FE3D9,
    data2: 0x985F,
    data3: 0x4908,
    data4: [0x91, 0xF9, 0xEE, 0x19, 0xF9, 0xFD, 0x15, 0x14],
};
const IID_IAPP_VISIBILITY: GUID = GUID {
    data1: 0x2246EA2D,
    data2: 0xCAEA,
    data3: 0x4441,
    data4: [0xA3, 0xC8, 0x6F, 0x41, 0xFF, 0xDD, 0xF3, 0xB7],
};

#[repr(C)]
struct IAppVisibility {
    vtable: *const IAppVisibilityVtbl,
}

#[repr(C)]
#[allow(dead_code)]
struct IAppVisibilityVtbl {
    query_interface: unsafe extern "system" fn(
        this: *mut IAppVisibility,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> i32,
    add_ref: unsafe extern "system" fn(this: *mut IAppVisibility) -> u32,
    release: unsafe extern "system" fn(this: *mut IAppVisibility) -> u32,
    get_app_visibility_on_monitor: unsafe extern "system" fn(
        this: *mut IAppVisibility,
        monitor: *mut c_void,
        value: *mut i32,
    ) -> i32,
    is_launcher_visible:
        unsafe extern "system" fn(this: *mut IAppVisibility, visible: *mut BOOL) -> i32,
    advise: unsafe extern "system" fn(
        this: *mut IAppVisibility,
        events: *mut c_void,
        cookie: *mut u32,
    ) -> i32,
    unadvise: unsafe extern "system" fn(this: *mut IAppVisibility, cookie: u32) -> i32,
}

thread_local! {
    static COM_INITIALIZED: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn capture_session_available() -> bool {
    let active = current_session_is_active();
    let default_desktop = input_desktop_is_default();
    session_allows_capture(active, default_desktop)
}

fn current_session_is_active() -> Option<bool> {
    let mut buffer = ptr::null_mut();
    let mut bytes = 0;
    let queried = unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            WTS_CURRENT_SESSION,
            WTSConnectState,
            &mut buffer,
            &mut bytes,
        )
    };
    if queried == 0 || buffer.is_null() || bytes < size_of::<i32>() as u32 {
        if !buffer.is_null() {
            unsafe { WTSFreeMemory(buffer.cast::<c_void>()) };
        }
        return None;
    }

    let state = unsafe { buffer.cast::<i32>().read_unaligned() };
    unsafe { WTSFreeMemory(buffer.cast::<c_void>()) };
    Some(state == WTSActive)
}

fn input_desktop_is_default() -> bool {
    let desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    if desktop.is_null() {
        return false;
    }

    let mut required_bytes = 0;
    unsafe {
        GetUserObjectInformationW(desktop, UOI_NAME, ptr::null_mut(), 0, &mut required_bytes)
    };
    let mut name = vec![0_u16; (required_bytes as usize).div_ceil(size_of::<u16>())];
    let read = required_bytes > 0
        && unsafe {
            GetUserObjectInformationW(
                desktop,
                UOI_NAME,
                name.as_mut_ptr().cast::<c_void>(),
                required_bytes,
                &mut required_bytes,
            )
        } != 0;
    unsafe { CloseDesktop(desktop) };
    if !read {
        return false;
    }

    let end = name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(name.len());
    String::from_utf16(&name[..end]).is_ok_and(|name| name.eq_ignore_ascii_case("Default"))
}

fn session_allows_capture(active: Option<bool>, default_desktop: bool) -> bool {
    active == Some(true) && default_desktop
}

pub(super) fn dismiss_transient_shell_ui_before_capture() {
    if !transient_shell_ui_is_visible() {
        return;
    }

    send_escape();
    let started = Instant::now();
    let mut resent = false;
    while transient_shell_ui_is_visible() && !shell_ui_dismiss_timed_out(started.elapsed()) {
        if should_send_second_escape(started.elapsed(), resent) {
            send_escape();
            resent = true;
        }
        thread::sleep(SHELL_UI_POLL_INTERVAL);
    }
    // IsLauncherVisible / cloaked state can flip before the fade finishes.
    thread::sleep(SHELL_UI_FADE_SETTLE);
}

fn transient_shell_ui_is_visible() -> bool {
    launcher_visible_via_com() == Some(true) || shell_host_flyout_is_visible()
}

fn launcher_visible_via_com() -> Option<bool> {
    ensure_com();
    let mut visibility = ptr::null_mut::<IAppVisibility>();
    // SAFETY: `visibility` is a valid out-pointer for the COM instance. The
    // class and interface IDs match IAppVisibility in shobjidl_core.h.
    let created = unsafe {
        CoCreateInstance(
            &CLSID_APP_VISIBILITY,
            ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IAPP_VISIBILITY,
            ptr::from_mut(&mut visibility).cast(),
        )
    };
    if created < 0 || visibility.is_null() {
        return None;
    }

    let mut visible: BOOL = 0;
    // SAFETY: `visibility` is a live IAppVisibility from CoCreateInstance. The
    // vtable layout matches the interface, and Release balances the implicit
    // AddRef from construction.
    let queried = unsafe {
        let vtable = (*visibility).vtable;
        let result = ((*vtable).is_launcher_visible)(visibility, &mut visible);
        ((*vtable).release)(visibility);
        result
    };
    if queried < 0 {
        return None;
    }
    Some(visible != 0)
}

fn ensure_com() {
    COM_INITIALIZED.with(|ready| {
        if ready.get() {
            return;
        }
        // SAFETY: first COM init on this thread; a failed or already-initialized
        // HRESULT is ignored because later CoCreateInstance reports its own errors.
        unsafe {
            CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
        }
        ready.set(true);
    });
}

fn shell_host_flyout_is_visible() -> bool {
    let mut found = false;
    // SAFETY: the callback only reads window handles supplied by EnumWindows
    // and writes the `found` bool through lParam for the duration of the call.
    unsafe {
        EnumWindows(Some(enum_shell_flyout), &raw mut found as LPARAM);
    }
    found
}

unsafe extern "system" fn enum_shell_flyout(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let found = unsafe { &mut *(lparam as *mut bool) };
    if window_is_visible_shell_flyout(hwnd) {
        *found = true;
        return 0;
    }
    1
}

fn window_is_visible_shell_flyout(hwnd: HWND) -> bool {
    // SAFETY: `hwnd` is a top-level window from EnumWindows. The RECT and
    // process-id out-pointers are valid for the duration of each call.
    let (visible, iconic, width, height, pid) = unsafe {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let sized = GetWindowRect(hwnd, &mut rect) != 0;
        let mut pid = 0_u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        (
            IsWindowVisible(hwnd) != 0,
            IsIconic(hwnd) != 0,
            if sized {
                rect.right.saturating_sub(rect.left)
            } else {
                0
            },
            if sized {
                rect.bottom.saturating_sub(rect.top)
            } else {
                0
            },
            pid,
        )
    };
    if iconic || pid == 0 {
        return false;
    }
    if !window_qualifies_as_shell_flyout(visible, window_is_cloaked(hwnd), width, height) {
        return false;
    }
    process_image_path(pid).is_some_and(|path| process_is_transient_shell_host(&path))
}

fn window_is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0_u32;
    // SAFETY: `cloaked` is a DWORD out-buffer for DWMWA_CLOAKED.
    let queried = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            ptr::from_mut(&mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    };
    queried >= 0 && cloaked != 0
}

fn process_image_path(pid: u32) -> Option<String> {
    // SAFETY: the handle is either null or a process handle we close below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buffer = [0_u16; 512];
    let mut size = buffer.len() as u32;
    // SAFETY: `buffer` and `size` are valid for QueryFullProcessImageNameW, and
    // `handle` remains open until CloseHandle.
    let read = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(handle) };
    if read == 0 || size == 0 {
        return None;
    }
    String::from_utf16(&buffer[..size as usize]).ok()
}

fn send_escape() {
    let key_down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_ESCAPE,
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
                wVk: VK_ESCAPE,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key_down, key_up];
    // SAFETY: `inputs` is a live two-element keyboard down/up pair.
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
    use super::session_allows_capture;

    #[test]
    fn capture_requires_an_active_accessible_input_desktop() {
        assert!(session_allows_capture(Some(true), true));
        assert!(!session_allows_capture(Some(true), false));
        assert!(!session_allows_capture(Some(false), true));
        assert!(!session_allows_capture(None, true));
    }
}
