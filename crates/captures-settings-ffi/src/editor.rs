//! Serialized-worker editor handles and independently retained immutable frames.

use super::region::{RegionPixels, response, text};
use captures_app::editor_session::{EditorSession, OpenRequest, Request};
use image::RgbaImage;
use serde_json::json;
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::Arc,
};

/// Open from isolated native History/draft roots on a host worker.
///
/// # Safety
/// Input is readable NUL-terminated UTF-8 during the call. Non-null output is
/// aligned writable pointer storage; free its JSON with captures_settings_free_v1.
/// Serialize all returned session calls, including free, on one worker.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_open_v1(
    request_json: *const c_char,
    output: *mut *mut c_char,
) -> *mut EditorSession {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<OpenRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        EditorSession::open(request)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(session) => {
            let value = json!({"ok":true,"result":session.snapshot()});
            (Box::into_raw(Box::new(session)), value)
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable pointer storage.
    unsafe { output.write(response(value)) };
    handle
}

/// Apply one command and return a snapshot, never image bytes.
///
/// # Safety
/// Non-null handle is live and exclusively owned for the call. Input is readable
/// NUL-terminated UTF-8. Free the owned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_request_v1(
    handle: *mut EditorSession,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains valid input and exclusive handle ownership.
        let request = serde_json::from_str::<Request>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { handle.as_mut() }.ok_or("editor handle is null")?;
        session.execute(request)?;
        Ok::<_, String>(json!(session.snapshot()))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(snapshot) => json!({"ok":true,"result":snapshot}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Retain the current frame without copying pixels; null input returns null.
///
/// # Safety
/// Non-null handle is live and is not accessed/freed concurrently. Release the
/// returned frame once with captures_editor_frame_free_v1. It outlives edits and
/// session destruction and may be transferred to the UI thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_frame_v1(
    handle: *const EditorSession,
) -> *mut Arc<RgbaImage> {
    // SAFETY: caller retains a live session without concurrent mutations.
    unsafe { handle.as_ref() }
        .map(|session| Box::into_raw(Box::new(session.pixels())))
        .unwrap_or(ptr::null_mut())
}

/// Borrow top-down straight-alpha sRGB RGBA8. False leaves output unchanged.
///
/// # Safety
/// Non-null frame is live and remains so throughout every pixel read. Non-null
/// output is aligned writable RegionPixels storage. Never modify/free data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_frame_pixels_v1(
    frame: *const Arc<RgbaImage>,
    output: *mut RegionPixels,
) -> bool {
    if frame.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains a valid immutable frame through all pixel reads.
    let image = unsafe { &*frame };
    let pixels = RegionPixels {
        data: image.as_ptr(),
        length: image.len(),
        width: image.width(),
        height: image.height(),
        bytes_per_row: image.width() as usize * 4,
    };
    // SAFETY: output is aligned writable storage.
    unsafe { output.write(pixels) };
    true
}

/// # Safety
/// Null or a live frame from captures_editor_frame_v1, released exactly once
/// after every borrow has ended. May run on the UI thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_frame_free_v1(frame: *mut Arc<RgbaImage>) {
    if !frame.is_null() {
        // SAFETY: caller transfers unique box ownership; shared pixels use Arc.
        drop(unsafe { Box::from_raw(frame) });
    }
}

/// No implicit save on close. Hosts explicitly flush drafts before freeing.
///
/// # Safety
/// Null or live exclusive session from open, released once on its owner worker.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_free_v1(handle: *mut EditorSession) {
    if !handle.is_null() {
        // SAFETY: caller transfers unique ownership after all calls complete.
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    unsafe fn take_json(value: *mut c_char) -> serde_json::Value {
        // SAFETY: tests pass only live Rust-owned response strings.
        let json = serde_json::from_slice(unsafe { CStr::from_ptr(value) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(value) };
        json
    }

    #[test]
    fn frames_survive_edit_and_close_and_failed_requests_preserve_state() {
        let data = tempfile::tempdir().unwrap();
        let original = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 40, y as u8 * 80, 9, 255])
        });
        let capture = captures_app::persist_screenshot(
            &data.path().join("history"),
            &original,
            captures_capture::CaptureMode::Region,
        )
        .unwrap();
        let request = CString::new(
            json!({
                "history_root":data.path().join("history"),
                "drafts_root":data.path().join("drafts"),
                "artifact_id":capture.entry.id,
            })
            .to_string(),
        )
        .unwrap();
        // SAFETY: test retains all input, handle, frame and response ownership.
        unsafe {
            let mut output = ptr::null_mut();
            let session = captures_editor_open_v1(request.as_ptr(), &mut output);
            assert!(!session.is_null());
            assert_eq!(take_json(output)["ok"], true);
            let old = captures_editor_frame_v1(session);
            let request =
                c"{\"operation\":\"crop\",\"rect\":{\"x\":1,\"y\":0,\"width\":2,\"height\":1}}";
            assert_eq!(
                take_json(captures_editor_request_v1(session, request.as_ptr()))["result"]["document"]
                    ["width"],
                2.
            );
            assert_eq!(
                take_json(captures_editor_request_v1(session, c"{bad".as_ptr()))["ok"],
                false
            );
            let current = captures_editor_frame_v1(session);
            captures_editor_free_v1(session);
            let mut pixels = RegionPixels {
                data: ptr::null(),
                length: 0,
                width: 0,
                height: 0,
                bytes_per_row: 0,
            };
            assert!(captures_editor_frame_pixels_v1(old, &mut pixels));
            assert_eq!(
                (pixels.width, pixels.height, pixels.bytes_per_row),
                (3, 2, 12)
            );
            assert_eq!(
                std::slice::from_raw_parts(pixels.data, pixels.length),
                original.as_raw()
            );
            assert!(captures_editor_frame_pixels_v1(current, &mut pixels));
            assert_eq!((pixels.width, pixels.height), (2, 1));
            assert_eq!(
                std::slice::from_raw_parts(pixels.data, pixels.length),
                &[40, 0, 9, 255, 80, 0, 9, 255]
            );
            captures_editor_frame_free_v1(old);
            captures_editor_frame_free_v1(current);
        }
    }

    #[test]
    fn null_handles_and_bad_open_have_owned_errors_and_leave_pixel_output_unchanged() {
        // SAFETY: null pointers are explicitly accepted; output lives throughout.
        unsafe {
            assert!(captures_editor_open_v1(ptr::null(), ptr::null_mut()).is_null());
            let mut output = ptr::null_mut();
            assert!(captures_editor_open_v1(ptr::null(), &mut output).is_null());
            assert_eq!(take_json(output)["ok"], false);
            assert_eq!(
                take_json(captures_editor_request_v1(
                    ptr::null_mut(),
                    c"{\"operation\":\"snapshot\"}".as_ptr()
                ))["ok"],
                false
            );
            assert!(captures_editor_frame_v1(ptr::null()).is_null());
            let mut pixels = RegionPixels {
                data: ptr::null(),
                length: 91,
                width: 7,
                height: 13,
                bytes_per_row: 28,
            };
            assert!(!captures_editor_frame_pixels_v1(ptr::null(), &mut pixels));
            assert_eq!((pixels.length, pixels.width, pixels.height), (91, 7, 13));
            captures_editor_frame_free_v1(ptr::null_mut());
            captures_editor_free_v1(ptr::null_mut());
        }
    }
}
