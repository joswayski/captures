//! Opaque, immutable session ownership. Pixel storage never enters JSON or a file.
use captures_app::{region::RegionSession, selection::Rect};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    ptr,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RegionPixels {
    pub(super) data: *const u8,
    pub(super) length: usize,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) bytes_per_row: usize,
}

pub(super) fn response(value: Value) -> *mut c_char {
    CString::new(value.to_string())
        .expect("JSON contains no NUL bytes")
        .into_raw()
}

// Callers establish C-string validity before entering this helper.
pub(super) unsafe fn text<'a>(input: *const c_char) -> Result<&'a str, String> {
    if input.is_null() {
        return Err("string pointer is null".into());
    }
    // SAFETY: caller guarantees NUL-terminated readable storage during the call.
    let bytes = unsafe { CStr::from_ptr(input) }.to_bytes();
    if bytes.len() > super::MAX_REQUEST_BYTES {
        return Err("string exceeds 8 MiB".into());
    }
    std::str::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Prepare off the UI thread. Null on failure; `output` receives the JSON result
/// containing the selected display or an error. Retain the main-thread flow guard.
///
/// # Safety
/// `display_id` is readable NUL-terminated UTF-8 for this call. Non-null `output`
/// is writable aligned storage for one char pointer; its previous value is not
/// freed. Release the returned JSON with captures_settings_free_v1. Release a
/// successful handle once with captures_region_free_v1, after all borrows finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_region_prepare_v1(
    display_id: *const c_char,
    generation: u64,
    freeze: bool,
    include_cursor: bool,
    output: *mut *mut c_char,
) -> *mut RegionSession {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller supplies readable input throughout this call.
        let display_id = unsafe { text(display_id) }?;
        RegionSession::prepare(display_id, generation, freeze, include_cursor)
            .map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (session, result) = match result {
        Ok(session) => {
            let result = json!({"ok":true,"result":{"display":session.display()}});
            (Box::into_raw(Box::new(session)), result)
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned, writable pointer storage.
    unsafe {
        output.write(response(result));
    }
    session
}

/// Borrow frozen straight-alpha RGBA8/sRGB, top-to-bottom rows; no copy/allocation.
/// False leaves output unchanged for null handles/output or a non-frozen session.
///
/// # Safety
/// Non-null `session` must be a live handle from prepare, not freed concurrently.
/// Non-null `output` is writable aligned RegionPixels storage. The returned data
/// is read-only and valid only while the session remains alive. Do not free it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_region_pixels_v1(
    session: *const RegionSession,
    output: *mut RegionPixels,
) -> bool {
    if session.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains the immutable handle for the duration of this read.
    let Some(image) = (unsafe { &*session }).frozen_image() else {
        return false;
    };
    let pixels = RegionPixels {
        data: image.as_ptr(),
        length: image.len(),
        width: image.width(),
        height: image.height(),
        bytes_per_row: image.width() as usize * 4,
    };
    // SAFETY: caller supplies aligned, writable output storage.
    unsafe {
        output.write(pixels);
    }
    true
}

/// Capture/crop/save off the UI thread, after hiding selection/countdown windows.
/// Countdown selects fresh pixels even for a frozen session. Settings are fixed
/// at prepare. Returns the same captured-artifact/error envelope as app requests.
///
/// # Safety
/// Non-null `session` is live and retained until the call returns. `root` is a
/// readable NUL-terminated UTF-8 path during this call. Free the JSON response with
/// captures_settings_free_v1. Do not free the session while a worker/provider uses it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_region_capture_v1(
    session: *const RegionSession,
    root: *const c_char,
    rect: Rect,
    after_countdown: bool,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if session.is_null() {
            return Err("session pointer is null".into());
        }
        // SAFETY: caller retains the handle and readable path during the call.
        let root = unsafe { text(root) }?;
        let session = unsafe { &*session };
        session
            .capture(Path::new(root), rect, after_countdown)
            .map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(artifact) => json!({"ok":true,"result":captures_app::Response::Captured { artifact }}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Release the frozen image and session. Null is permitted. This does not finish
/// the capture flow; finish its main-thread guard separately even on cancellation.
///
/// # Safety
/// A non-null handle was returned by prepare and has not been freed. All pixel
/// borrows and capture calls must finish before freeing; never race this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_region_free_v1(session: *mut RegionSession) {
    if !session.is_null() {
        // SAFETY: the caller transfers back this unique Box after all borrows end.
        unsafe {
            drop(Box::from_raw(session));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn take(pointer: *mut c_char) -> Value {
        assert!(!pointer.is_null());
        let value = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe {
            super::super::captures_settings_free_v1(pointer);
        }
        value
    }
    #[test]
    fn failed_preparation_returns_an_owned_error_without_capture_or_files() {
        for id in [Some("not-a-display"), None] {
            let id = id.map(|s| CString::new(s).unwrap());
            let mut result = ptr::null_mut();
            let handle = unsafe {
                captures_region_prepare_v1(
                    id.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
                    0,
                    true,
                    true,
                    &mut result,
                )
            };
            assert!(handle.is_null());
            let error = take(result);
            assert_eq!(error["ok"], false);
            assert!(error["error"].as_str().unwrap().contains(if id.is_some() {
                "cancelled"
            } else {
                "null"
            }));
        }
        assert!(
            unsafe { captures_region_prepare_v1(ptr::null(), 0, false, false, ptr::null_mut()) }
                .is_null()
        );
    }
    #[test]
    fn null_capture_and_borrow_have_explicit_failure_and_ownership() {
        let mut pixels = RegionPixels {
            data: ptr::null(),
            length: 17,
            width: 3,
            height: 9,
            bytes_per_row: 12,
        };
        assert!(!unsafe { captures_region_pixels_v1(ptr::null(), &mut pixels) });
        assert_eq!(pixels.length, 17);
        assert_eq!(pixels.width, 3);
        let result = take(unsafe {
            captures_region_capture_v1(ptr::null(), ptr::null(), Rect::default(), false)
        });
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"], "session pointer is null");
        unsafe {
            captures_region_free_v1(ptr::null_mut());
        }
    }
}
