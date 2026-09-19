//! Versioned opaque window sessions; the borrowed pixel layout matches regions.
use super::region::{RegionPixels, response, text};
use captures_app::selection::Point;
use captures_app::window::{Target, WindowSession};
use serde_json::json;
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    ptr,
};

/// Allocation-free shared macOS radius fallback; the host discovers its OS version.
#[unsafe(no_mangle)]
pub extern "C" fn captures_macos_window_corner_radius_v1(major_version: i64) -> f64 {
    captures_app::window::macos_window_corner_radius_for_major_version(major_version)
}

/// Prepare on a worker after hiding capture windows. Retain the event-loop flow.
/// The response contains display/windows/shell_chrome descriptors, never pixels.
///
/// # Safety
/// `display_id` is readable NUL-terminated UTF-8. Non-null `output` is aligned,
/// writable char-pointer storage; the old value is not freed. Free JSON with
/// captures_settings_free_v1 and the handle once with captures_window_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_window_prepare_v1(
    display_id: *const c_char,
    generation: u64,
    freeze: bool,
    include_cursor: bool,
    fallback_corner_radius: f64,
    output: *mut *mut c_char,
) -> *mut WindowSession {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains a readable string during this call.
        let display_id = unsafe { text(display_id) }?;
        WindowSession::prepare(
            display_id,
            generation,
            freeze,
            include_cursor,
            fallback_corner_radius,
        )
        .map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, result) = match result {
        Ok(session) => {
            let result = json!({"ok":true,"result":{
                "display":session.display(), "windows":session.windows(), "shell_chrome":session.shell_chrome()
            }});
            (Box::into_raw(Box::new(session)), result)
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies writable aligned storage.
    unsafe {
        output.write(response(result));
    }
    handle
}

/// Borrow immutable RGBA8 without copying. False leaves output unchanged.
///
/// # Safety
/// Non-null session is a live handle, retained for every draw/worker borrowing
/// pixels. Non-null output is aligned writable RegionPixels storage. Never free
/// or mutate the returned pixels, or race the handle's destruction with this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_window_pixels_v1(
    session: *const WindowSession,
    output: *mut RegionPixels,
) -> bool {
    if session.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains the immutable handle until all borrows finish.
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
    // SAFETY: caller supplies writable aligned storage.
    unsafe {
        output.write(pixels);
    }
    true
}

/// Allocation-free pointer hit testing in display-local overlay coordinates.
/// -1 selects the display; nonnegative values index the prepared windows array.
/// False for nulls/nonfinite points leaves output unchanged.
///
/// # Safety
/// A non-null session is retained throughout this call. Non-null output is
/// aligned, writable i64 storage. No references or input storage are retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_window_hit_test_v1(
    session: *const WindowSession,
    point: Point,
    output: *mut i64,
) -> bool {
    if session.is_null() || output.is_null() || !point.x.is_finite() || !point.y.is_finite() {
        return false;
    }
    // SAFETY: caller retains the immutable session and aligned output storage.
    let index = unsafe { &*session }
        .hit_test(point)
        .map_or(-1, |index| index as i64);
    unsafe {
        output.write(index);
    }
    true
}

/// Capture/save after hiding the selector/countdown. Target JSON is
/// {"kind":"window","id":"…"}, {"kind":"display"}, or
/// {"kind":"region","rect":{"x":N,"y":N,"width":N,"height":N}} in
/// display-local logical units. Only listed window IDs are accepted. A countdown
/// refreshes source geometry/pixels/cursor for every target.
///
/// # Safety
/// Non-null session remains alive throughout this call; root and target_json are
/// readable NUL-terminated UTF-8. Free the returned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_window_capture_v1(
    session: *const WindowSession,
    root: *const c_char,
    target_json: *const c_char,
    after_countdown: bool,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if session.is_null() {
            return Err("session pointer is null".into());
        }
        // SAFETY: caller retains both readable strings and the handle.
        let root = unsafe { text(root) }?;
        let target = serde_json::from_str::<Target>(unsafe { text(target_json) }?)
            .map_err(|e| e.to_string())?;
        let session = unsafe { &*session };
        session
            .capture(Path::new(root), &target, after_countdown)
            .map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(artifact) => json!({"ok":true,"result":captures_app::Response::Captured { artifact }}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Release once after all providers/workers stop borrowing; null is permitted.
/// This does not finish the capture-flow guard on the event-loop thread.
///
/// # Safety
/// A non-null handle came from prepare and has not been freed. No outstanding
/// pixel borrows or capture calls may outlive or race this destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_window_free_v1(session: *mut WindowSession) {
    if !session.is_null() {
        // SAFETY: caller transfers unique ownership after all borrows finish.
        unsafe {
            drop(Box::from_raw(session));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    #[test]
    fn macos_radius_abi_preserves_the_version_boundary() {
        assert_eq!(captures_macos_window_corner_radius_v1(25), 10.);
        assert_eq!(captures_macos_window_corner_radius_v1(26), 25.);
    }

    fn take(pointer: *mut c_char) -> serde_json::Value {
        assert!(!pointer.is_null());
        // SAFETY: each tested response is an owned NUL-terminated allocation.
        let result = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe {
            super::super::captures_settings_free_v1(pointer);
        }
        result
    }
    #[test]
    fn cancelled_and_null_preparation_return_owned_errors_without_screen_access() {
        let id = CString::new("missing").unwrap();
        for id in [id.as_ptr(), ptr::null()] {
            let mut output = ptr::null_mut();
            let handle = unsafe { captures_window_prepare_v1(id, 0, true, true, 0., &mut output) };
            assert!(handle.is_null());
            let result = take(output);
            assert_eq!(result["ok"], false);
            assert_eq!(
                result["error"],
                if id.is_null() {
                    "string pointer is null"
                } else {
                    "Capture cancelled"
                }
            );
        }
        assert!(
            unsafe { captures_window_prepare_v1(id.as_ptr(), 0, true, false, 0., ptr::null_mut()) }
                .is_null()
        );
    }
    #[test]
    fn failed_pixel_borrow_preserves_output_and_null_capture_is_an_error() {
        let mut pixels = RegionPixels {
            data: ptr::null(),
            length: 77,
            width: 11,
            height: 7,
            bytes_per_row: 44,
        };
        assert!(!unsafe { captures_window_pixels_v1(ptr::null(), &mut pixels) });
        assert_eq!(
            (
                pixels.length,
                pixels.width,
                pixels.height,
                pixels.bytes_per_row
            ),
            (77, 11, 7, 44)
        );
        let result = take(unsafe {
            captures_window_capture_v1(ptr::null(), ptr::null(), ptr::null(), false)
        });
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"], "session pointer is null");
        unsafe {
            captures_window_free_v1(ptr::null_mut());
        }
    }

    #[test]
    fn failed_hit_test_preserves_the_previous_target() {
        let mut index = 37;
        assert!(!unsafe {
            captures_window_hit_test_v1(ptr::null(), Point { x: 13., y: 19. }, &mut index)
        });
        assert_eq!(index, 37);
        assert!(!unsafe {
            captures_window_hit_test_v1(ptr::null(), Point { x: 13., y: 19. }, ptr::null_mut())
        });
    }
}
