//! Serialized-worker editor handles and independently retained immutable frames.

use super::region::{RegionPixels, response, text};
use captures_app::editor_session::{EditorSession, ExportOptions, OpenRequest, Request};
use image::RgbaImage;
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
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

#[derive(Deserialize)]
struct SaveNewRequest {
    history_root: PathBuf,
    destination: PathBuf,
    options: ExportOptions,
    mode: captures_capture::CaptureMode,
}

/// Publish the edited frame as a new file and distinct History artifact.
/// Does not mutate the session or save its draft. Run on the session worker.
///
/// # Safety
/// Non-null session is live and not accessed/freed concurrently. Input is
/// readable NUL-terminated UTF-8. Free owned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_save_new_v1(
    session: *const EditorSession,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and a live serialized session.
        let request = serde_json::from_str::<SaveNewRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_ref() }.ok_or("editor handle is null")?;
        captures_app::editor_output::save_new_export(
            &request.history_root,
            &session.pixels(),
            &request.destination,
            request.options,
            request.mode,
        )
        .map_err(|error| error.to_string())
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(saved) => json!({"ok":true,"result":saved}),
        Err(error) => json!({"ok":false,"error":error}),
    })
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

/// Encode on the session worker without changing editor state or doing I/O.
/// Returns independently owned bytes and an owned metadata/error JSON response.
///
/// # Safety
/// Non-null session is live and not accessed/freed concurrently. Input is readable
/// NUL-terminated UTF-8. Non-null output is aligned writable pointer storage; free
/// its JSON with captures_settings_free_v1. Null output refuses the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_encode_v1(
    session: *const EditorSession,
    options_json: *const c_char,
    output: *mut *mut c_char,
) -> *mut Vec<u8> {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and a live serialized session.
        let options = serde_json::from_str::<ExportOptions>(unsafe { text(options_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_ref() }.ok_or("editor handle is null")?;
        session.encode_export(options)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(bytes) => {
            let value = json!({"ok":true,"result":{"length":bytes.len()}});
            (Box::into_raw(Box::new(bytes)), value)
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable pointer storage.
    unsafe { output.write(response(value)) };
    handle
}

#[repr(C)]
pub struct EditorBytes {
    pub data: *const u8,
    pub length: usize,
}

/// Borrow encoded bytes; false leaves output unchanged.
///
/// # Safety
/// Non-null export is a live handle from captures_editor_encode_v1, retained
/// throughout all reads. Non-null output is aligned writable EditorBytes storage.
/// Never mutate/free the data pointer. The handle may outlive the editor session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_export_bytes_v1(
    export: *const Vec<u8>,
    output: *mut EditorBytes,
) -> bool {
    if export.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains live export storage and aligned writable output.
    let bytes = unsafe { &*export };
    unsafe {
        output.write(EditorBytes {
            data: bytes.as_ptr(),
            length: bytes.len(),
        })
    };
    true
}

/// # Safety
/// Null or a live export from captures_editor_encode_v1, released exactly once
/// after all byte borrows end. May run on a different thread from the session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_export_free_v1(export: *mut Vec<u8>) {
    if !export.is_null() {
        // SAFETY: caller transfers unique ownership after all reads finish.
        drop(unsafe { Box::from_raw(export) });
    }
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
    fn save_new_reports_publication_collision_and_partial_success_without_changing_session() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("history");
        let capture = captures_app::persist_screenshot(
            &root,
            &RgbaImage::from_fn(7, 3, |x, y| {
                image::Rgba([x as u8 * 31, y as u8 * 71, 9, 255])
            }),
            captures_capture::CaptureMode::Window,
        )
        .unwrap();
        let mut session = EditorSession::open(OpenRequest {
            history_root: root.clone(),
            drafts_root: data.path().join("drafts"),
            artifact_id: capture.entry.id,
        })
        .unwrap();
        session
            .execute(Request::Crop {
                rect: captures_app::editor::Rect {
                    x: 2.,
                    y: 1.,
                    width: 4.,
                    height: 2.,
                },
            })
            .unwrap();
        let before = json!(session.snapshot());
        let pixels = session.pixels();
        let destination = data.path().join("copy.png");
        let mut request = json!({"history_root":root,"destination":destination,"mode":"window","options":{"format":"png","quality":"preserve","quality_value":80,"png":{}}});
        let input = CString::new(request.to_string()).unwrap();
        // SAFETY: stack session and C strings remain live and serialized; every owned response is freed.
        unsafe {
            assert_eq!(
                take_json(captures_editor_save_new_v1(ptr::null(), input.as_ptr()))["ok"],
                false
            );
            assert_eq!(
                take_json(captures_editor_save_new_v1(&session, ptr::null()))["ok"],
                false
            );
            assert_eq!(
                take_json(captures_editor_save_new_v1(&session, c"{}".as_ptr()))["ok"],
                false
            );
            assert!(!destination.exists());
            let saved = take_json(captures_editor_save_new_v1(&session, input.as_ptr()));
            assert_eq!(saved["ok"], true);
            assert_eq!(saved["result"]["status"], "saved");
            assert_eq!(saved["result"]["path"], json!(destination));
            assert_eq!(saved["result"]["artifact"]["entry"]["mode"], "window");
            let bytes = std::fs::read(&destination).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
            assert_eq!(decoded.dimensions(), (4, 2));
            assert_eq!(decoded.get_pixel(0, 0).0, [62, 71, 9, 255]);
            assert_eq!(
                take_json(captures_editor_save_new_v1(&session, input.as_ptr()))["ok"],
                false
            );
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);

            let blocked = data.path().join("blocked-history");
            std::fs::write(&blocked, b"blocked").unwrap();
            request["history_root"] = json!(blocked);
            request["destination"] = json!(data.path().join("recovered.png"));
            let input = CString::new(request.to_string()).unwrap();
            let saved = take_json(captures_editor_save_new_v1(&session, input.as_ptr()));
            assert_eq!(saved["ok"], true);
            assert_eq!(saved["result"]["status"], "saved_without_history");
            assert!(!saved["result"]["warning"].as_str().unwrap().is_empty());
            assert_eq!(
                std::fs::read(data.path().join("recovered.png")).unwrap(),
                bytes
            );
        }
        assert_eq!(json!(session.snapshot()), before);
        assert!(Arc::ptr_eq(&pixels, &session.pixels()));
        assert!(!data.path().join("drafts").exists());
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
            let options =
                c"{\"format\":\"png\",\"quality\":\"preserve\",\"quality_value\":100,\"png\":{}}";
            let exported = captures_editor_encode_v1(session, options.as_ptr(), &mut output);
            assert!(!exported.is_null());
            let encoded_response = take_json(output);
            assert_eq!(encoded_response["ok"], true);
            assert_eq!(encoded_response["result"].as_object().unwrap().len(), 1);
            let before = take_json(captures_editor_request_v1(
                session,
                c"{\"operation\":\"snapshot\"}".as_ptr(),
            ));
            for invalid in [
                c"{bad",
                c"{\"format\":\"gif\",\"quality\":\"preserve\",\"quality_value\":100,\"png\":{}}",
                c"{\"format\":\"png\",\"quality\":\"unknown\",\"quality_value\":100,\"png\":{}}",
                c"{\"format\":\"png\",\"quality\":\"preserve\",\"quality_value\":256,\"png\":{}}",
                c"{\"format\":\"png\",\"quality\":\"maximum\",\"quality_value\":100,\"max_size_bytes\":0,\"png\":{}}",
            ] {
                assert!(captures_editor_encode_v1(session, invalid.as_ptr(), &mut output).is_null());
                assert_eq!(take_json(output)["ok"], false);
                assert_eq!(take_json(captures_editor_request_v1(session, c"{\"operation\":\"snapshot\"}".as_ptr())), before);
            }
            assert_eq!(
                take_json(captures_editor_request_v1(
                    session,
                    c"{\"operation\":\"undo\"}".as_ptr()
                ))["ok"],
                true
            );
            captures_editor_free_v1(session);
            let mut bytes = EditorBytes {
                data: ptr::null(),
                length: 0,
            };
            assert!(!captures_editor_export_bytes_v1(exported, ptr::null_mut()));
            assert!(captures_editor_export_bytes_v1(exported, &mut bytes));
            assert_eq!(encoded_response["result"]["length"], bytes.length);
            let decoded =
                image::load_from_memory(std::slice::from_raw_parts(bytes.data, bytes.length))
                    .unwrap()
                    .into_rgba8();
            assert_eq!(decoded.dimensions(), (2, 1));
            assert_eq!(decoded.as_raw(), &[40, 0, 9, 255, 80, 0, 9, 255]);
            captures_editor_export_free_v1(exported);
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
            assert!(captures_editor_encode_v1(ptr::null(), ptr::null(), ptr::null_mut()).is_null());
            for options in [
                ptr::null(),
                c"{\"format\":\"png\",\"quality\":\"preserve\",\"quality_value\":100,\"png\":{}}"
                    .as_ptr(),
            ] {
                assert!(captures_editor_encode_v1(ptr::null(), options, &mut output).is_null());
                assert_eq!(take_json(output)["ok"], false);
            }
            let mut bytes = EditorBytes {
                data: ptr::null(),
                length: 91,
            };
            assert!(!captures_editor_export_bytes_v1(ptr::null(), &mut bytes));
            assert_eq!(bytes.length, 91);
            assert!(bytes.data.is_null());
            captures_editor_export_free_v1(ptr::null_mut());
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
