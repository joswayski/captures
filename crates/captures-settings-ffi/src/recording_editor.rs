//! Serialized-worker recording editor and independently retained preview frames.

use super::region::{RegionPixels, response, text};
use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequest, RecordingEditorRequestV2,
    RecordingEditorSession, RecordingExportComparison, RecordingPlayback, RecordingSaveRequest,
    RecordingTimelineThumbnails,
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
pub struct RecordingEditorComparison(RecordingExportComparison);

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

/// Read-only accepted-session permanent path hint; no eligibility or file work.
///
/// # Safety
/// Session is live and serialized for this call. Free returned owned JSON with
/// captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_original_save_path_v1(
    session: *const RecordingEditorSession,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        Ok::<_, &str>(json!({"path":session.original_save_path().and_then(|path| path.to_str())}))
    }))
    .unwrap_or(Err("internal panic"));
    response(match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    })
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

/// Execute one atomic v2 edit/seek request and return accepted save + preview state.
///
/// # Safety
/// Session is live, exclusively owned, and serialized for the call. Input is
/// readable NUL-terminated UTF-8. Free returned JSON with settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_request_v2(
    session: *mut RecordingEditorSession,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and exclusive session ownership.
        let request =
            serde_json::from_str::<RecordingEditorRequestV2>(unsafe { text(request_json) }?)
                .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_mut() }.ok_or("recording editor handle is null")?;
        session.execute_v2(request)?;
        Ok::<_, String>(json!(session.snapshot_v2()))
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

/// Decode and retain the immutable full source at the accepted position.
///
/// # Safety
/// Session is live and serialized for this call. Cancel remains live until the
/// call returns and may be atomically cancelled elsewhere. Non-null output is
/// aligned writable pointer storage. Free output JSON and returned frame once;
/// the frame may outlive session and cancel owners.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_source_frame_v1(
    session: *const RecordingEditorSession,
    cancel: *const CancelToken,
    output: *mut *mut c_char,
) -> *mut Arc<RgbaImage> {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains live session/cancel handles for this call.
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        let cancel = unsafe { cancel.as_ref() }.ok_or("recording export cancel handle is null")?;
        let position_ms = session.snapshot().position_ms;
        session
            .source_frame(cancel)
            .map(|frame| (position_ms, frame))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok((position_ms, frame)) => {
            let value = json!({
                "ok": true,
                "result": {
                    "position_ms": position_ms,
                    "width": frame.width(),
                    "height": frame.height(),
                },
            });
            (Box::into_raw(Box::new(frame)), value)
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable output storage.
    unsafe { output.write(response(value)) };
    handle
}

/// Encode a read-only first-attempt comparison of the accepted trim position.
///
/// # Safety
/// Session is live and serialized, cancel remains live until return, and a
/// non-null output is aligned writable storage. JSON and comparison are freed
/// exactly once; cloned frames may outlive every other owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_comparison_v1(
    session: *const RecordingEditorSession,
    cancel: *const CancelToken,
    output: *mut *mut c_char,
) -> *mut RecordingEditorComparison {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains live serialized session and cancel handles.
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        let cancel = unsafe { cancel.as_ref() }.ok_or("recording export cancel handle is null")?;
        session.export_comparison(cancel)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (owner, value) = match result {
        Ok(comparison) => {
            let before = comparison.before_frame();
            let value = json!({"ok":true,"result":{
                "basis":"accepted_preview_first_attempt",
                "revision":comparison.revision,
                "position_ms":comparison.position_ms,
                "after_seek_position_ms":comparison.after_seek_position_ms,
                "sample_start_ms":comparison.sample_start_ms,
                "sample_duration_ms":comparison.sample_duration_ms,
                "attempts":comparison.attempts,
                "export":comparison.export,
                "width":before.width(),
                "height":before.height(),
            }});
            (
                Box::into_raw(Box::new(RecordingEditorComparison(comparison))),
                value,
            )
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller provides writable output storage.
    unsafe { output.write(response(value)) };
    owner
}

/// # Safety
/// Comparison is live for the call. Returned frame has independent ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_comparison_before_frame_v1(
    comparison: *const RecordingEditorComparison,
) -> *mut Arc<RgbaImage> {
    // SAFETY: caller retains comparison during this call.
    unsafe { comparison.as_ref() }
        .map(|comparison| Box::into_raw(Box::new(comparison.0.before_frame())))
        .unwrap_or(ptr::null_mut())
}

/// # Safety
/// Comparison is live for the call. Returned frame has independent ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_comparison_after_frame_v1(
    comparison: *const RecordingEditorComparison,
) -> *mut Arc<RgbaImage> {
    // SAFETY: caller retains comparison during this call.
    unsafe { comparison.as_ref() }
        .map(|comparison| Box::into_raw(Box::new(comparison.0.after_frame())))
        .unwrap_or(ptr::null_mut())
}

/// # Safety
/// Null or one uniquely owned comparison, freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_comparison_free_v1(
    comparison: *mut RecordingEditorComparison,
) {
    if !comparison.is_null() {
        // SAFETY: caller transfers the uniquely owned box.
        drop(unsafe { Box::from_raw(comparison) });
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

/// Open playback with accepted audio when it is audible. GIF, audio-less,
/// muted, and zero-gain edits do not open an output device and report
/// `audio_enabled:false`.
///
/// # Safety
/// Session is live and serialized for this call on the worker that will drive
/// and free playback. Cancel may be atomically cancelled elsewhere and is
/// cloned by the returned stream. Non-null output is aligned writable pointer
/// storage. Free output JSON and playback exactly once; playback may outlive
/// session and cancel owners.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_playback_open_v2(
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
        session.playback_with_audio(position_ms, cancel)
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
                    "audio_enabled": playback.audio_enabled(),
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

/// Blocking estimate for the v2 session's accepted Save-new-copy export.
///
/// # Safety
/// Session is live and serialized for this call. Cancel is live until return
/// and may be atomically cancelled elsewhere. Free the returned response with
/// captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_estimate_v2(
    session: *const RecordingEditorSession,
    cancel: *const CancelToken,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains live session/cancel handles for this call.
        let session = unsafe { session.as_ref() }.ok_or("recording editor handle is null")?;
        let cancel = unsafe { cancel.as_ref() }.ok_or("recording export cancel handle is null")?;
        session.estimate_save_export(cancel)
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

/// Blocking replacement of this session's original permanent MP4/GIF. The
/// new response alone includes `requires_reopen` on every error.
///
/// # Safety
/// Session is live, exclusively owned and serialized on its worker. Cancel is
/// live until return and may be atomically cancelled elsewhere. Callback and
/// context remain callable during the operation and borrowed progress JSON
/// must be copied to retain it. Free the response with settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_replace_original_v1(
    session: *mut RecordingEditorSession,
    cancel: *const CancelToken,
    progress: RecordingEditorProgress,
    context: *mut c_void,
) -> *mut c_char {
    let mut session = unsafe { session.as_mut() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let session = session
            .as_deref_mut()
            .ok_or_else(|| ("recording editor handle is null".to_string(), false))?;
        let cancel = unsafe { cancel.as_ref() }
            .ok_or_else(|| ("recording export cancel handle is null".to_string(), false))?;
        let replaced = session
            .replace_original(cancel, |event| emit_progress(progress, context, &event))
            .map_err(|error| (error.message, error.requires_reopen))?;
        Ok::<_, (String, bool)>(json!({"replacement":replaced,"snapshot":session.snapshot_v2()}))
    }));
    response(match result {
        Ok(Ok(snapshot)) => json!({"ok":true,"result":snapshot}),
        Ok(Err((error, requires_reopen))) => {
            json!({"ok":false,"error":error,"requires_reopen":requires_reopen})
        }
        Err(_) => json!({"ok":false,"error":"internal panic",
            "requires_reopen":session.as_ref().is_some_and(|session| session.requires_reopen())}),
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

        // SAFETY: v2 has the same owned null-handle error contract.
        let response = unsafe {
            json(captures_recording_editor_request_v2(
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

        // SAFETY: v2 estimate preserves the v1 owned null-handle contract.
        let estimate = unsafe {
            json(captures_recording_editor_estimate_v2(
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
        let mut source_frame_response = ptr::null_mut();
        // SAFETY: output is writable; null session is an explicit owned error.
        let source_frame = unsafe {
            captures_recording_editor_source_frame_v1(
                ptr::null(),
                ptr::null(),
                &mut source_frame_response,
            )
        };
        assert!(source_frame.is_null());
        // SAFETY: failed generation returned one owned response.
        let source_frame_response = unsafe { json(source_frame_response) };
        assert_eq!(source_frame_response["ok"], false);
        assert_eq!(
            source_frame_response["error"],
            "recording editor handle is null"
        );
        // SAFETY: null output refuses work and every thumbnail free accepts null.
        assert!(
            unsafe {
                captures_recording_editor_thumbnails_v1(ptr::null(), ptr::null(), ptr::null_mut())
            }
            .is_null()
        );
        // SAFETY: null output refuses work before inspecting other handles.
        assert!(
            unsafe {
                captures_recording_editor_source_frame_v1(ptr::null(), ptr::null(), ptr::null_mut())
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
        // SAFETY: v2 also refuses work before inspecting null handles.
        assert!(
            unsafe {
                captures_recording_editor_playback_open_v2(
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
        let opened = unsafe { json(open_response) };
        assert_eq!(opened["ok"], true);
        // SAFETY: read-only hint is owned JSON, not a v1/v2 snapshot field.
        let path = unsafe { json(captures_recording_editor_original_save_path_v1(session)) };
        assert_eq!(path["result"]["path"], source.to_str().unwrap());
        assert_eq!(
            unsafe { json(captures_recording_editor_original_save_path_v1(ptr::null())) }["ok"],
            false
        );
        let v1_keys = opened["result"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();

        let accepted_v2 = CString::new(
            serde_json::json!({
                "operation": "update_preview",
                "edit": captures_media::EditSpec::default(),
                "export": {
                    "format": "mp4",
                    "quality": "preserve",
                    "max_size_bytes": 100_000,
                    "frames_per_second": null,
                    "gif_max_colors": null,
                },
            })
            .to_string(),
        )
        .unwrap();
        // SAFETY: session and request remain live and exclusive for the call.
        let accepted_v2 = unsafe {
            json(captures_recording_editor_request_v2(
                session,
                accepted_v2.as_ptr(),
            ))
        };
        assert_eq!(accepted_v2["ok"], true);
        assert_eq!(
            accepted_v2["result"]["save_export"]["max_size_bytes"],
            100_000
        );
        assert_eq!(
            accepted_v2["result"]["preview_export"]["max_size_bytes"],
            serde_json::Value::Null
        );

        let snapshot = CString::new(r#"{"operation":"snapshot"}"#).unwrap();
        // SAFETY: v1 remains callable after v2 and returns its original shape.
        let v1_after_v2 = unsafe {
            json(captures_recording_editor_request_v1(
                session,
                snapshot.as_ptr(),
            ))
        };
        assert_eq!(v1_after_v2["ok"], true);
        assert_eq!(
            v1_after_v2["result"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            v1_keys
        );
        assert!(v1_after_v2["result"].get("save_export").is_none());
        assert_eq!(
            v1_after_v2["result"]["preview_export"]["max_size_bytes"],
            serde_json::Value::Null
        );

        let rejected_v1 = CString::new(
            serde_json::json!({
                "operation": "update_preview",
                "edit": captures_media::EditSpec::default(),
                "export": {
                    "format": "mp4",
                    "quality": "preserve",
                    "max_size_bytes": 100_000,
                    "frames_per_second": null,
                    "gif_max_colors": null,
                },
            })
            .to_string(),
        )
        .unwrap();
        // SAFETY: v1 request remains an owned error and cannot alter v2 state.
        let rejected_v1 = unsafe {
            json(captures_recording_editor_request_v1(
                session,
                rejected_v1.as_ptr(),
            ))
        };
        assert_eq!(rejected_v1["ok"], false);
        // SAFETY: snapshot v2 verifies rejected v1 preserved accepted state.
        let after_rejection = unsafe {
            json(captures_recording_editor_request_v2(
                session,
                snapshot.as_ptr(),
            ))
        };
        assert_eq!(
            after_rejection["result"]["save_export"]["max_size_bytes"],
            100_000
        );
        assert_eq!(after_rejection["result"]["revision"], 1);
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
        // SAFETY: null output refuses to start a comparison.
        assert!(
            unsafe { captures_recording_editor_comparison_v1(session, cancel, ptr::null_mut()) }
                .is_null()
        );
        let mut comparison_response = ptr::null_mut();
        // SAFETY: live serialized session, cancel and writable output.
        let comparison = unsafe {
            captures_recording_editor_comparison_v1(session, cancel, &mut comparison_response)
        };
        assert!(!comparison.is_null());
        // SAFETY: comparison returned one owned response.
        let comparison_response = unsafe { json(comparison_response) };
        assert_eq!(
            comparison_response["result"]["basis"],
            "accepted_preview_first_attempt"
        );
        assert_eq!(comparison_response["result"]["position_ms"], 0);
        assert_eq!(comparison_response["result"]["revision"], 1);
        assert_eq!(comparison_response["result"]["attempts"], 1);
        assert!(comparison_response["result"]["export"]["max_size_bytes"].is_null());
        // SAFETY: both accessors clone independently retained frame owners.
        let before_comparison =
            unsafe { captures_recording_editor_comparison_before_frame_v1(comparison) };
        let after_comparison =
            unsafe { captures_recording_editor_comparison_after_frame_v1(comparison) };
        assert!(!before_comparison.is_null() && !after_comparison.is_null());
        // SAFETY: clones outlive their parent owner.
        unsafe { captures_recording_editor_comparison_free_v1(comparison) };
        let mut source_response = ptr::null_mut();
        // SAFETY: handles/output remain live for source-frame extraction.
        let source_frame = unsafe {
            captures_recording_editor_source_frame_v1(session, cancel, &mut source_response)
        };
        assert!(!source_frame.is_null());
        // SAFETY: extraction returned one owned response.
        let source_response = unsafe { json(source_response) };
        assert_eq!(source_response["result"]["position_ms"], 0);
        assert_eq!(source_response["result"]["width"], 32);
        assert_eq!(source_response["result"]["height"], 24);

        let cancelled = captures_recording_editor_cancel_create_v1();
        // SAFETY: token is live and independently owned.
        unsafe { captures_recording_editor_cancel_v1(cancelled) };
        let mut cancelled_comparison_response = ptr::null_mut();
        // SAFETY: pre-cancelled comparison returns an owned error, no frame.
        assert!(
            unsafe {
                captures_recording_editor_comparison_v1(
                    session,
                    cancelled,
                    &mut cancelled_comparison_response,
                )
            }
            .is_null()
        );
        // SAFETY: failed comparison returned one owned JSON response.
        assert_eq!(unsafe { json(cancelled_comparison_response) }["ok"], false);
        let mut cancelled_response = ptr::null_mut();
        // SAFETY: handles/output remain live; pre-cancellation is supported.
        let cancelled_frame = unsafe {
            captures_recording_editor_source_frame_v1(session, cancelled, &mut cancelled_response)
        };
        assert!(cancelled_frame.is_null());
        // SAFETY: failed extraction returned one owned response.
        let cancelled_response = unsafe { json(cancelled_response) };
        assert_eq!(cancelled_response["ok"], false);
        assert!(
            cancelled_response["error"]
                .as_str()
                .unwrap()
                .contains("cancelled")
        );
        // SAFETY: cancelled owner is released exactly once after the call.
        unsafe { captures_recording_editor_cancel_free_v1(cancelled) };

        let mut response = ptr::null_mut();
        // SAFETY: handles and output remain live for generation.
        let thumbnails =
            unsafe { captures_recording_editor_thumbnails_v1(session, cancel, &mut response) };
        assert!(!thumbnails.is_null());
        // SAFETY: generation returned one owned response.
        let response = unsafe { json(response) };
        assert_eq!(response["result"]["frame_count"], 12);
        assert_eq!(response["result"]["sprite_width"], 1_920);

        let mut audible_response = ptr::null_mut();
        // SAFETY: audio-less source must open v2 without touching a device.
        let audio_less_playback = unsafe {
            captures_recording_editor_playback_open_v2(session, 0, cancel, &mut audible_response)
        };
        assert!(!audio_less_playback.is_null());
        // SAFETY: v2 open returned one owned response.
        let audible_response = unsafe { json(audible_response) };
        assert_eq!(audible_response["result"]["audio_enabled"], false);
        // SAFETY: independent playback owner is released once.
        unsafe { captures_recording_editor_playback_free_v1(audio_less_playback) };

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

        // SAFETY: the new owned error envelope never changes old ABI shapes.
        let missing = unsafe {
            json(captures_recording_editor_replace_original_v1(
                session,
                ptr::null(),
                None,
                ptr::null_mut(),
            ))
        };
        assert_eq!(missing["ok"], false);
        assert_eq!(missing["requires_reopen"], false);
        // SAFETY: the pre-cancelled token is live throughout this call.
        let cancelled = captures_recording_editor_cancel_create_v1();
        unsafe { captures_recording_editor_cancel_v1(cancelled) };
        let rejected = unsafe {
            json(captures_recording_editor_replace_original_v1(
                session,
                cancelled,
                None,
                ptr::null_mut(),
            ))
        };
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["requires_reopen"], false);
        unsafe { captures_recording_editor_cancel_free_v1(cancelled) };
        // SAFETY: the live session and token are used exclusively for replace.
        let replaced = unsafe {
            json(captures_recording_editor_replace_original_v1(
                session,
                cancel,
                None,
                ptr::null_mut(),
            ))
        };
        assert_eq!(replaced["ok"], true, "{replaced}");
        assert_eq!(replaced["result"]["replacement"]["status"], "replaced");
        assert_eq!(replaced["result"]["snapshot"]["revision"], 2);
        assert_eq!(
            replaced["result"]["snapshot"]["save_export"]["max_size_bytes"],
            serde_json::Value::Null
        );

        // SAFETY: playback/generation are complete, so owners may be released.
        unsafe {
            captures_recording_editor_playback_free_v1(playback);
            captures_recording_editor_free_v1(session);
            captures_recording_editor_cancel_free_v1(cancel);
        }

        let mut source_pixels = MaybeUninit::<RegionPixels>::uninit();
        // SAFETY: retained source frame outlives session/cancel owners.
        assert!(unsafe {
            captures_recording_editor_frame_pixels_v1(source_frame, source_pixels.as_mut_ptr())
        });
        // SAFETY: successful access initialized the descriptor.
        let source_pixels = unsafe { source_pixels.assume_init() };
        assert_eq!((source_pixels.width, source_pixels.height), (32, 24));
        assert_eq!(source_pixels.length, 32 * 24 * 4);
        // SAFETY: retained frame is released once after the final borrow.
        unsafe { captures_recording_editor_frame_free_v1(source_frame) };

        for comparison_frame in [before_comparison, after_comparison] {
            let mut pixels = MaybeUninit::<RegionPixels>::uninit();
            // SAFETY: independent frame remains live after comparison/session/cancel free.
            assert!(unsafe {
                captures_recording_editor_frame_pixels_v1(comparison_frame, pixels.as_mut_ptr())
            });
            // SAFETY: successful pixel borrow initialized the descriptor.
            let pixels = unsafe { pixels.assume_init() };
            assert_eq!((pixels.width, pixels.height), (32, 24));
            assert_eq!(pixels.length, 32 * 24 * 4);
            // SAFETY: owner is released once after the borrow.
            unsafe { captures_recording_editor_frame_free_v1(comparison_frame) };
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

        let mut no_path_entry = entry;
        no_path_entry.saved_path = None;
        captures_history::update_metadata(&history_root, &no_path_entry).unwrap();
        let mut reopened_response = ptr::null_mut();
        // SAFETY: same serialized request and fresh owned output.
        let reopened = unsafe {
            captures_recording_editor_open_v1(open_request.as_ptr(), &mut reopened_response)
        };
        assert!(!reopened.is_null());
        assert_eq!(unsafe { json(reopened_response) }["ok"], true);
        assert_eq!(
            unsafe { json(captures_recording_editor_original_save_path_v1(reopened)) }["result"]["path"],
            serde_json::Value::Null
        );
        unsafe { captures_recording_editor_free_v1(reopened) };
    }
}
