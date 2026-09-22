//! Serialized-worker recording editor and independently retained preview frames.

use super::region::{RegionPixels, response, text};
use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequest, RecordingEditorSession, RecordingPlayback,
    RecordingSaveRequest, RecordingTimelineThumbnails,
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

pub struct RecordingEditorThumbnails(RecordingTimelineThumbnails);
pub struct RecordingEditorPlayback(RecordingPlayback);

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

/// Open persistent silent playback of the accepted edit and preview export.
///
/// # Safety
/// Session is live and serialized for this call. Cancel may be atomically
/// cancelled elsewhere and is cloned by the returned stream. Non-null output
/// is aligned writable pointer storage. Free output JSON and playback exactly
/// once; playback may outlive session and cancel owners.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_playback_open_v1(
    session: *const RecordingEditorSession,
    position_ms: u64,
    cancel: *const CancelToken,
    output: *mut *mut c_char,
) -> *mut RecordingEditorPlayback {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains live session/cancel handles for this call.
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        let cancel = unsafe { cancel.as_ref() }.ok_or("recording export cancel handle is null")?;
        session.playback(position_ms, cancel)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(playback) => {
            let value = json!({
                "ok": true,
                "result": {
                    "start_position_ms": playback.start_position_ms(),
                    "width": playback.width(),
                    "height": playback.height(),
                    "frames_per_second": playback.frames_per_second(),
                },
            });
            (
                Box::into_raw(Box::new(RecordingEditorPlayback(playback))),
                value,
            )
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable output storage.
    unsafe { output.write(response(value)) };
    handle
}

/// Return one clock-paced frame, clean EOF, or an owned error response.
///
/// # Safety
/// Playback is live, exclusive, and serialized for this call. Non-null output
/// is aligned writable pointer storage. Null output refuses work without
/// advancing playback. Free response JSON and any returned frame exactly once;
/// the frame may outlive playback and session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_playback_next_v1(
    playback: *mut RecordingEditorPlayback,
    output: *mut *mut c_char,
) -> *mut Arc<RgbaImage> {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains exclusive playback ownership for this call.
        let playback = unsafe { playback.as_mut() }.ok_or("recording playback handle is null")?;
        playback.0.next_frame()
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(Some(frame)) => {
            let position_ms = frame.position_ms;
            (
                Box::into_raw(Box::new(frame.pixels())),
                json!({"ok":true,"result":{"eof":false,"position_ms":position_ms}}),
            )
        }
        Ok(None) => (ptr::null_mut(), json!({"ok":true,"result":{"eof":true}})),
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable output storage.
    unsafe { output.write(response(value)) };
    handle
}

/// # Safety
/// Null or a live exclusive playback owner. Drop stops, kills if necessary,
/// reaps, and joins the decoder without cancelling the caller's token.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_playback_free_v1(
    playback: *mut RecordingEditorPlayback,
) {
    if !playback.is_null() {
        // SAFETY: caller transfers unique box ownership.
        drop(unsafe { Box::from_raw(playback) });
    }
}

/// Generate and retain the immutable source timeline strip.
///
/// # Safety
/// Session is live and serialized for this call. Cancel remains live until the
/// call returns and may be atomically cancelled elsewhere. Non-null output is
/// aligned writable pointer storage. Free output JSON and the returned owner
/// exactly once; the owner and its pixels may outlive the session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_thumbnails_v1(
    session: *const RecordingEditorSession,
    cancel: *const CancelToken,
    output: *mut *mut c_char,
) -> *mut RecordingEditorThumbnails {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains live session/cancel handles for this call.
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        let cancel = unsafe { cancel.as_ref() }.ok_or("recording export cancel handle is null")?;
        session.timeline_thumbnails(cancel)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(thumbnails) => {
            let value = json!({
                "ok": true,
                "result": {
                    "frame_count": thumbnails.frame_count,
                    "frame_width": thumbnails.frame_width,
                    "frame_height": thumbnails.frame_height,
                    "sprite_width": thumbnails.sprite_width,
                    "sprite_height": thumbnails.sprite_height,
                },
            });
            (
                Box::into_raw(Box::new(RecordingEditorThumbnails(thumbnails))),
                value,
            )
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable output storage.
    unsafe { output.write(response(value)) };
    handle
}

/// Borrow top-down straight-alpha sRGB RGBA8 timeline pixels.
///
/// # Safety
/// Owner remains live through every read. Output is aligned writable storage.
/// False leaves output unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_thumbnails_pixels_v1(
    thumbnails: *const RecordingEditorThumbnails,
    output: *mut RegionPixels,
) -> bool {
    if thumbnails.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains immutable owner and writable output.
    let image = unsafe { &*thumbnails }.0.pixels();
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
/// Null or a live thumbnail owner, released once after all pixel reads finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_thumbnails_free_v1(
    thumbnails: *mut RecordingEditorThumbnails,
) {
    if !thumbnails.is_null() {
        // SAFETY: caller transfers unique box ownership.
        drop(unsafe { Box::from_raw(thumbnails) });
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

/// Blocking estimate for the session's accepted edit and preview export.
///
/// # Safety
/// Session is live and serialized for this call. Cancel is live until return
/// and may be atomically cancelled elsewhere. Free the returned response with
/// captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_estimate_v1(
    session: *const RecordingEditorSession,
    cancel: *const CancelToken,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains live session/cancel handles for this call.
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        let cancel = unsafe { cancel.as_ref() }.ok_or("recording export cancel handle is null")?;
        session.estimate_export(cancel)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(estimate) => json!({"ok":true,"result":estimate}),
        Err(error) => json!({"ok":false,"error":error}),
    })
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
    use captures_history::{ArtifactKind, HistoryEntry};
    use captures_recording::RecordingTarget;
    use std::{ffi::CStr, fs, mem::MaybeUninit, path::PathBuf, process::Command};

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

        // SAFETY: null session/cancel handles are explicit owned JSON errors.
        let estimate = unsafe {
            json(captures_recording_editor_estimate_v1(
                ptr::null(),
                ptr::null(),
            ))
        };
        assert_eq!(estimate["ok"], false);
        assert_eq!(estimate["error"], "recording editor handle is null");

        let mut thumbnails_response = ptr::null_mut();
        // SAFETY: output storage is writable; null handles are an explicit owned error.
        let thumbnails = unsafe {
            captures_recording_editor_thumbnails_v1(
                ptr::null(),
                ptr::null(),
                &mut thumbnails_response,
            )
        };
        assert!(thumbnails.is_null());
        // SAFETY: successful response publication returns owned JSON.
        let thumbnails_response = unsafe { json(thumbnails_response) };
        assert_eq!(thumbnails_response["ok"], false);
        assert_eq!(
            thumbnails_response["error"],
            "recording editor handle is null"
        );
        // SAFETY: null output refuses work and every thumbnail free accepts null.
        assert!(
            unsafe {
                captures_recording_editor_thumbnails_v1(ptr::null(), ptr::null(), ptr::null_mut())
            }
            .is_null()
        );
        let mut playback_response = ptr::null_mut();
        // SAFETY: output is writable; null session is an explicit owned error.
        let playback = unsafe {
            captures_recording_editor_playback_open_v1(
                ptr::null(),
                0,
                ptr::null(),
                &mut playback_response,
            )
        };
        assert!(playback.is_null());
        // SAFETY: failed open returned one owned response.
        let playback_response = unsafe { json(playback_response) };
        assert_eq!(playback_response["ok"], false);
        assert_eq!(
            playback_response["error"],
            "recording editor handle is null"
        );
        // SAFETY: null output refuses work and null playback returns an owned error.
        assert!(
            unsafe {
                captures_recording_editor_playback_open_v1(
                    ptr::null(),
                    0,
                    ptr::null(),
                    ptr::null_mut(),
                )
            }
            .is_null()
        );
        let mut playback_next_response = ptr::null_mut();
        assert!(
            unsafe {
                captures_recording_editor_playback_next_v1(
                    ptr::null_mut(),
                    &mut playback_next_response,
                )
            }
            .is_null()
        );
        // SAFETY: null playback returned one owned response.
        let playback_next_response = unsafe { json(playback_next_response) };
        assert_eq!(playback_next_response["ok"], false);
        assert_eq!(
            playback_next_response["error"],
            "recording playback handle is null"
        );

        let sentinel = RegionPixels {
            data: ptr::dangling(),
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
        // SAFETY: null thumbnail owner is rejected before touching writable output.
        assert!(!unsafe {
            captures_recording_editor_thumbnails_pixels_v1(ptr::null(), &mut output)
        });
        assert_eq!(output.data, sentinel.data);
        assert_eq!(output.length, sentinel.length);
        assert_eq!(output.width, sentinel.width);
        assert_eq!(output.height, sentinel.height);
        assert_eq!(output.bytes_per_row, sentinel.bytes_per_row);
        // SAFETY: every explicit free accepts null as a no-op.
        unsafe {
            captures_recording_editor_frame_free_v1(ptr::null_mut());
            captures_recording_editor_playback_free_v1(ptr::null_mut());
            captures_recording_editor_thumbnails_free_v1(ptr::null_mut());
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

    #[test]
    fn retained_thumbnail_pixels_outlive_the_session() {
        let ffmpeg = std::env::var_os("CAPTURES_TEST_FFMPEG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ffmpeg"));
        let ffprobe = std::env::var_os("CAPTURES_TEST_FFPROBE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ffprobe"));
        if MediaToolchain::new(ffmpeg.clone(), ffprobe.clone())
            .verify()
            .is_err()
        {
            return;
        }
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("source.mp4");
        let status = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:size=32x24:rate=10:duration=1",
                "-c:v",
                "mpeg4",
                "-an",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let id = "00000000-0000-4000-8000-000000000001".to_owned();
        let history_root = data.path().join("history");
        let entry = HistoryEntry {
            id: id.clone(),
            kind: ArtifactKind::Video,
            preview_url: String::new(),
            full_url: String::new(),
            width: 32,
            height: 24,
            size_bytes: fs::metadata(&source).unwrap().len(),
            created_at: "2026-09-22T00:00:00Z".into(),
            mode: None,
            saved_path: Some(source.to_string_lossy().into_owned()),
            mime_type: Some("video/mp4".into()),
            duration_ms: Some(1_000),
            target: Some(RecordingTarget::Display {
                display_id: "test-display".into(),
            }),
            has_system_audio: false,
            has_microphone_audio: false,
            dropped_frames: 0,
        };
        captures_history::save_recording(&history_root, &entry, b"poster", &source).unwrap();
        let open_request = CString::new(
            serde_json::json!({
                "history_root": history_root,
                "artifact_id": id,
                "ffmpeg": ffmpeg,
                "ffprobe": ffprobe,
            })
            .to_string(),
        )
        .unwrap();
        let mut open_response = ptr::null_mut();
        // SAFETY: request/output remain live for the synchronous open call.
        let session =
            unsafe { captures_recording_editor_open_v1(open_request.as_ptr(), &mut open_response) };
        assert!(!session.is_null());
        // SAFETY: open returned one owned response.
        assert_eq!(unsafe { json(open_response) }["ok"], true);
        let mut missing_cancel_response = ptr::null_mut();
        // SAFETY: session/output are live; null cancel is an explicit owned error.
        let missing_cancel = unsafe {
            captures_recording_editor_thumbnails_v1(
                session,
                ptr::null(),
                &mut missing_cancel_response,
            )
        };
        assert!(missing_cancel.is_null());
        // SAFETY: failed generation returned one owned response.
        let missing_cancel_response = unsafe { json(missing_cancel_response) };
        assert_eq!(missing_cancel_response["ok"], false);
        assert_eq!(
            missing_cancel_response["error"],
            "recording export cancel handle is null"
        );
        let cancel = captures_recording_editor_cancel_create_v1();
        let mut response = ptr::null_mut();
        // SAFETY: handles and output remain live for generation.
        let thumbnails =
            unsafe { captures_recording_editor_thumbnails_v1(session, cancel, &mut response) };
        assert!(!thumbnails.is_null());
        // SAFETY: generation returned one owned response.
        let response = unsafe { json(response) };
        assert_eq!(response["result"]["frame_count"], 12);
        assert_eq!(response["result"]["sprite_width"], 1_920);

        let mut playback_response = ptr::null_mut();
        // SAFETY: handles/output stay live through playback open.
        let playback = unsafe {
            captures_recording_editor_playback_open_v1(session, 0, cancel, &mut playback_response)
        };
        assert!(!playback.is_null());
        // SAFETY: open returned one owned response.
        let playback_response = unsafe { json(playback_response) };
        assert_eq!(playback_response["result"]["start_position_ms"], 0);
        assert_eq!(playback_response["result"]["width"], 32);
        assert_eq!(playback_response["result"]["height"], 24);
        // SAFETY: null output refuses work without advancing the live playback.
        assert!(
            unsafe { captures_recording_editor_playback_next_v1(playback, ptr::null_mut()) }
                .is_null()
        );
        let mut next_response = ptr::null_mut();
        // SAFETY: playback/output remain live for the exclusive next call.
        let playback_frame =
            unsafe { captures_recording_editor_playback_next_v1(playback, &mut next_response) };
        assert!(!playback_frame.is_null());
        // SAFETY: next returned one owned response.
        let next_response = unsafe { json(next_response) };
        assert_eq!(next_response["result"]["eof"], false);
        assert_eq!(next_response["result"]["position_ms"], 0);

        // SAFETY: playback/generation are complete, so owners may be released.
        unsafe {
            captures_recording_editor_playback_free_v1(playback);
            captures_recording_editor_free_v1(session);
            captures_recording_editor_cancel_free_v1(cancel);
        }

        let mut playback_pixels = MaybeUninit::<RegionPixels>::uninit();
        // SAFETY: retained frame remains live after playback/session/cancel free.
        assert!(unsafe {
            captures_recording_editor_frame_pixels_v1(playback_frame, playback_pixels.as_mut_ptr())
        });
        // SAFETY: successful access initialized the descriptor.
        let playback_pixels = unsafe { playback_pixels.assume_init() };
        assert_eq!((playback_pixels.width, playback_pixels.height), (32, 24));
        // SAFETY: retained frame is released once after the final borrow.
        unsafe { captures_recording_editor_frame_free_v1(playback_frame) };

        let mut pixels = MaybeUninit::<RegionPixels>::uninit();
        // SAFETY: the independent thumbnail owner remains live.
        assert!(unsafe {
            captures_recording_editor_thumbnails_pixels_v1(thumbnails, pixels.as_mut_ptr())
        });
        // SAFETY: successful pixel access initialized the descriptor.
        let pixels = unsafe { pixels.assume_init() };
        assert_eq!((pixels.width, pixels.height), (1_920, 90));
        assert_eq!(pixels.length, 1_920 * 90 * 4);
        // SAFETY: owner is released once after the final pixel borrow.
        unsafe { captures_recording_editor_thumbnails_free_v1(thumbnails) };
    }
}
