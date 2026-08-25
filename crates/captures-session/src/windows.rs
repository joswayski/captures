use std::{ffi::c_void, ptr};

use windows_sys::Win32::System::{
    RemoteDesktop::{
        WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSActive, WTSConnectState, WTSFreeMemory,
        WTSQuerySessionInformationW,
    },
    StationsAndDesktops::{
        CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
    },
};

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
