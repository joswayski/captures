//! Serialized-worker recording editor and independently retained preview frames.

use super::region::{RegionPixels, response, text};
use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequest, RecordingEditorSession,
    RecordingSaveRequest,
};
use captures_media::{CancelToken, ExportProgress, MediaToolchain};
use image::RgbaImage;
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::{CString, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRequest {
    history_root: PathBuf,
    artifact_id: String,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

pub type RecordingEditorProgress = Option<unsafe extern "C" fn(*mut c_void, *const c_char)>;

/// Open and probe one real History recording on its serialized worker.
///
/// # Safety
/// Input is readable NUL-terminated UTF-8. Non-null output is aligned writable
/// pointer storage. Free output JSON and the returned session exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_open_v1(
    request_json: *const c_char,
    output: *mut *mut c_char,
) -> *mut RecordingEditorSession {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input for this call.
        let request = serde_json::from_str::<OpenRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        RecordingEditorSession::open(
            RecordingEditorOpenRequest {
                history_root: request.history_root,
                artifact_id: request.artifact_id,
            },
            MediaToolchain::new(request.ffmpeg, request.ffprobe),
        )
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(session) => {
            let snapshot = json!(session.snapshot());
            (
                Box::into_raw(Box::new(session)),
                json!({"ok":true,"result":snapshot}),
            )
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable output storage.
    unsafe { output.write(response(value)) };
    handle
}

/// Execute one atomic edit/seek request and return the accepted snapshot.
///
/// # Safety
/// Session is live, exclusively owned, and serialized for the call. Input is
/// readable NUL-terminated UTF-8. Free returned JSON with settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_request_v1(
    session: *mut RecordingEditorSession,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and exclusive session ownership.
        let request =
            serde_json::from_str::<RecordingEditorRequest>(unsafe { text(request_json) }?)
                .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_mut() }.ok_or("recording editor handle is null")?;
        session.execute(request)?;
        Ok::<_, String>(json!(session.snapshot()))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(snapshot) => json!({"ok":true,"result":snapshot}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Retain the last accepted preview without copying pixels.
///
/// # Safety
/// Non-null session is live and not concurrently accessed. The returned frame
/// is freed exactly once after all borrows and may outlive the session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_frame_v1(
    session: *const RecordingEditorSession,
) -> *mut Arc<RgbaImage> {
    // SAFETY: caller retains a live session for this call.
    unsafe { session.as_ref() }
        .map(|session| Box::into_raw(Box::new(session.frame())))
        .unwrap_or(ptr::null_mut())
}

/// Borrow top-down straight-alpha sRGB RGBA8. False leaves output unchanged.
///
/// # Safety
/// Frame remains live through every read. Output is aligned writable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_frame_pixels_v1(
    frame: *const Arc<RgbaImage>,
    output: *mut RegionPixels,
) -> bool {
    if frame.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains immutable frame and writable output.
    let image = unsafe { &*frame };
    unsafe {
        output.write(RegionPixels {
            data: image.as_ptr(),
            length: image.len(),
            width: image.width(),
            height: image.height(),
            bytes_per_row: image.width() as usize * 4,
        })
    };
    true
}

/// # Safety
/// Null or a live frame owner, released once after all pixel reads finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_frame_free_v1(frame: *mut Arc<RgbaImage>) {
    if !frame.is_null() {
        // SAFETY: caller transfers unique box ownership.
        drop(unsafe { Box::from_raw(frame) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn captures_recording_editor_cancel_create_v1() -> *mut CancelToken {
    Box::into_raw(Box::new(CancelToken::default()))
}

/// Thread-safe cancellation request. Null is a no-op.
///
/// # Safety
/// Non-null token remains live for this call and until the export worker returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_cancel_v1(cancel: *const CancelToken) {
    if let Some(cancel) = unsafe { cancel.as_ref() } {
        cancel.cancel();
    }
}

/// # Safety
/// Null or a live token, freed once only after the export call has returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_cancel_free_v1(cancel: *mut CancelToken) {
    if !cancel.is_null() {
        // SAFETY: caller transfers unique ownership after export completion.
        drop(unsafe { Box::from_raw(cancel) });
    }
}

/// Blocking Save new copy. Progress JSON is borrowed only during each callback.
///
/// # Safety
/// Session is live and serialized for this call. JSON is readable NUL-terminated
/// UTF-8. Cancel is live until return and may be atomically cancelled elsewhere.
/// Callback/context remain callable for the call and callback must copy any JSON
/// it retains. Free returned response with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_save_new_v1(
    session: *const RecordingEditorSession,
    request_json: *const c_char,
    cancel: *const CancelToken,
    progress: RecordingEditorProgress,
    context: *mut c_void,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable JSON and live session/cancel handles.
        let request = serde_json::from_str::<RecordingSaveRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        let cancel = unsafe { cancel.as_ref() }.ok_or("recording export cancel handle is null")?;
        session.save_new(request, cancel, |event| {
            emit_progress(progress, context, &event);
        })
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(saved) => json!({"ok":true,"result":saved}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

fn emit_progress(
    callback: RecordingEditorProgress,
    context: *mut c_void,
    progress: &ExportProgress,
) {
    let Some(callback) = callback else {
        return;
    };
    let Ok(json) = serde_json::to_string(progress) else {
        return;
    };
    let Ok(json) = CString::new(json) else {
        return;
    };
    // SAFETY: callback validity is part of save_new's caller contract; JSON is
    // live and readable for exactly this synchronous callback.
    unsafe { callback(context, json.as_ptr()) };
}

/// No implicit export or persistent draft on close.
///
/// # Safety
/// Null or a live exclusive session, released once after all calls return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_free_v1(session: *mut RecordingEditorSession) {
    if !session.is_null() {
        // SAFETY: caller transfers unique session ownership.
        drop(unsafe { Box::from_raw(session) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captures_settings_free_v1;
    use std::{ffi::CStr, mem::MaybeUninit};

    unsafe fn json(ptr: *mut c_char) -> serde_json::Value {
        assert!(!ptr.is_null());
        // SAFETY: each ABI response is live NUL-terminated UTF-8 until freed below.
        let value = serde_json::from_slice(unsafe { CStr::from_ptr(ptr).to_bytes() }).unwrap();
        // SAFETY: this response is transferred exactly once to the matching free.
        unsafe { captures_settings_free_v1(ptr) };
        value
    }

    #[test]
    fn null_or_closed_owner_contracts_fail_without_touching_outputs() {
        let request = CString::new(r#"{"operation":"snapshot"}"#).unwrap();
        // SAFETY: input is readable and the null handle is an explicitly supported error.
        let response = unsafe {
            json(captures_recording_editor_request_v1(
                ptr::null_mut(),
                request.as_ptr(),
            ))
        };
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"], "recording editor handle is null");

        let sentinel = RegionPixels {
            data: 1_usize as *const u8,
            length: 2,
            width: 3,
            height: 4,
            bytes_per_row: 5,
        };
        let mut output = sentinel;
        // SAFETY: output is writable and null is explicitly rejected.
        assert!(!unsafe { captures_recording_editor_frame_pixels_v1(ptr::null(), &mut output) });
        assert_eq!(output.data, sentinel.data);
        assert_eq!(output.length, sentinel.length);
        assert_eq!(output.width, sentinel.width);
        assert_eq!(output.height, sentinel.height);
        assert_eq!(output.bytes_per_row, sentinel.bytes_per_row);
        // SAFETY: every explicit free accepts null as a no-op.
        unsafe {
            captures_recording_editor_frame_free_v1(ptr::null_mut());
            captures_recording_editor_cancel_free_v1(ptr::null_mut());
            captures_recording_editor_free_v1(ptr::null_mut());
        }
    }

    #[test]
    fn retained_frame_and_cancel_owners_are_independent() {
        let image = Arc::new(RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 4])));
        let frame = Box::into_raw(Box::new(image));
        let mut output = MaybeUninit::<RegionPixels>::uninit();
        // SAFETY: frame and output are live for the call.
        assert!(unsafe { captures_recording_editor_frame_pixels_v1(frame, output.as_mut_ptr()) });
        // SAFETY: successful call initializes the descriptor.
        let output = unsafe { output.assume_init() };
        assert_eq!((output.width, output.height, output.length), (3, 2, 24));
        // SAFETY: the retained frame is released once after its borrow ends.
        unsafe { captures_recording_editor_frame_free_v1(frame) };

        let cancel = captures_recording_editor_cancel_create_v1();
        assert!(!cancel.is_null());
        // SAFETY: token stays live through both calls and is uniquely freed after.
        unsafe {
            captures_recording_editor_cancel_v1(cancel);
            assert!((*cancel).is_cancelled());
            captures_recording_editor_cancel_free_v1(cancel);
        }
    }
}
