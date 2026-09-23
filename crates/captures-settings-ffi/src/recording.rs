//! Versioned opaque recording lifecycle. Every handle call is blocking and must
//! be serialized on one host worker. Media bytes never cross this boundary.
use super::region::{response, text};
use captures_capture::DisplayDescriptor;
use captures_media::{CancelToken, MediaToolchain};
use captures_recording::RecordingOptions;
use captures_recording_platform::{RecordingCapabilities, RecordingSession, microphone_devices};
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::{c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr,
};

pub type RecordingIsCurrent = Option<unsafe extern "C" fn(*mut c_void, u64) -> bool>;

#[derive(Deserialize)]
struct PrepareRequest {
    recovery_root: PathBuf,
    options: RecordingOptions,
    display: DisplayDescriptor,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum RecordingRequest {
    Snapshot,
    MicrophoneLevel,
    Start {
        generation: u64,
        #[serde(default)]
        exclude_captures_app: bool,
    },
    Pause,
    SetMicrophoneMuted {
        muted: bool,
        generation: u64,
        #[serde(default)]
        exclude_captures_app: bool,
    },
    Restart,
    Stop,
    Finish {
        history_root: PathBuf,
        ffmpeg: PathBuf,
        ffprobe: PathBuf,
    },
    Discard,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum InfoRequest {
    Capabilities {
        #[serde(default)]
        include_recording_controls_in_captures: bool,
    },
    MicrophoneDevices,
    History {
        root: PathBuf,
    },
}

/// Query platform recording capabilities/devices off the UI thread. The result
/// contains descriptors only, never media. Free it with captures_settings_free_v1.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_info_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<InfoRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(match request {
            InfoRequest::Capabilities {
                include_recording_controls_in_captures,
            } => json!({"capabilities": RecordingCapabilities::current(
                include_recording_controls_in_captures
            )}),
            InfoRequest::MicrophoneDevices => json!({"devices": microphone_devices()}),
            InfoRequest::History { root } => {
                let recordings = captures_history::load(&root, chrono::Utc::now())
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .filter(|entry| entry.kind.is_recording())
                    .filter_map(|entry| {
                        let directory = captures_history::entry_directory(&root, &entry.id).ok()?;
                        let media_path = entry.recording_media_path(&root)?;
                        Some(json!({
                            "entry": entry,
                            "media_path": media_path,
                            "preview_path": directory.join(captures_history::HISTORY_PREVIEW_FILE),
                        }))
                    })
                    .collect::<Vec<_>>();
                json!({"recordings":recordings})
            }
        })
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Prepare durable recovery state on a worker before countdown. A successful
/// handle has unique mutable ownership and must remain on one serialized worker.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8. Non-null `output` is aligned
/// writable char-pointer storage. Free output JSON and the handle exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_prepare_v1(
    request_json: *const c_char,
    output: *mut *mut c_char,
) -> *mut RecordingSession {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<PrepareRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        RecordingSession::prepare(request.recovery_root, request.options, request.display)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(session) => {
            let snapshot = session.snapshot();
            (
                Box::into_raw(Box::new(session)),
                json!({"ok":true,"result":{"snapshot":snapshot}}),
            )
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable pointer storage.
    unsafe {
        output.write(response(value));
    }
    handle
}

/// Execute one blocking lifecycle operation. `is_current` is required for start
/// and set_microphone_muted and may be called more than once from this worker; it
/// must only read a thread-safe host cancellation gate. Other operations ignore
/// callback/context/generation.
///
/// # Safety
/// `handle` is uniquely owned, live, and serialized for the full call.
/// `request_json` is readable NUL-terminated UTF-8. Callback context remains valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_request_v1(
    handle: *mut RecordingSession,
    request_json: *const c_char,
    is_current: RecordingIsCurrent,
    context: *mut c_void,
) -> *mut c_char {
    let result: Result<serde_json::Value, String> = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return Err("recording session pointer is null".into());
        }
        // SAFETY: caller retains readable input and unique handle ownership.
        let request = serde_json::from_str::<RecordingRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { &mut *handle };
        match request {
            RecordingRequest::Snapshot => Ok(json!({"snapshot":session.snapshot()})),
            RecordingRequest::MicrophoneLevel => {
                Ok(json!({"microphone_peak":session.microphone_level()}))
            }
            RecordingRequest::Start {
                generation,
                exclude_captures_app,
            } => {
                let callback = is_current.ok_or("start requires an is_current callback")?;
                let snapshot = session.start(exclude_captures_app, || {
                    // SAFETY: callback/context validity is part of this function's contract.
                    let host_current = unsafe { callback(context, generation) };
                    host_current && captures_app::capture_flow::is_current(generation)
                })?;
                Ok(json!({"snapshot":snapshot}))
            }
            RecordingRequest::Pause => Ok(json!({"snapshot":session.pause()?})),
            RecordingRequest::SetMicrophoneMuted {
                muted,
                generation,
                exclude_captures_app,
            } => {
                let callback =
                    is_current.ok_or("set_microphone_muted requires an is_current callback")?;
                let snapshot = session.set_microphone_muted(muted, exclude_captures_app, || {
                    // SAFETY: callback/context validity is part of this function's contract.
                    let host_current = unsafe { callback(context, generation) };
                    host_current && captures_app::capture_flow::is_current(generation)
                })?;
                Ok(json!({"snapshot":snapshot}))
            }
            RecordingRequest::Restart => Ok(json!({"snapshot":session.restart()?})),
            RecordingRequest::Stop => Ok(json!({"snapshot":session.stop()?})),
            RecordingRequest::Finish {
                history_root,
                ffmpeg,
                ffprobe,
            } => {
                let tools = MediaToolchain::new(ffmpeg, ffprobe);
                let finalized =
                    session.finish(Path::new(&history_root), &tools, &CancelToken::default())?;
                Ok(json!({"finalized":finalized}))
            }
            RecordingRequest::Discard => Ok(json!({"snapshot":session.discard()?})),
        }
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Free once after all worker calls return. Null is permitted. Platform segment
/// Drop aborts/discards an active engine and may block, but this is not durable
/// Stop/finalization; hosts must explicitly stop/discard on their worker first.
///
/// # Safety
/// A non-null handle came from prepare, has not been freed, and is not in use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_free_v1(handle: *mut RecordingSession) {
    if !handle.is_null() {
        // SAFETY: caller returns unique ownership after all calls complete.
        unsafe {
            drop(Box::from_raw(handle));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    fn take(pointer: *mut c_char) -> serde_json::Value {
        assert!(!pointer.is_null());
        // SAFETY: tested entry points return one owned C string.
        let value = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { super::super::captures_settings_free_v1(pointer) };
        value
    }

    #[test]
    fn info_has_versioned_envelopes_and_null_input_is_explicit() {
        let request = CString::new(
            r#"{"operation":"capabilities","include_recording_controls_in_captures":false}"#,
        )
        .unwrap();
        let value = take(unsafe { captures_recording_info_v1(request.as_ptr()) });
        assert_eq!(value["ok"], true);
        assert!(value["result"]["capabilities"]["microphone"].is_boolean());
        let null = take(unsafe { captures_recording_info_v1(ptr::null()) });
        assert_eq!(null["ok"], false);
        assert_eq!(null["error"], "string pointer is null");
    }

    #[test]
    fn null_handle_operations_are_owned_errors_and_null_free_is_allowed() {
        let request = CString::new(r#"{"operation":"snapshot"}"#).unwrap();
        let value = take(unsafe {
            captures_recording_request_v1(ptr::null_mut(), request.as_ptr(), None, ptr::null_mut())
        });
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"], "recording session pointer is null");
        unsafe { captures_recording_free_v1(ptr::null_mut()) };
    }

    #[test]
    fn null_output_refuses_prepare_without_creating_recovery_state() {
        let request = CString::new("{}").unwrap();
        assert!(
            unsafe { captures_recording_prepare_v1(request.as_ptr(), ptr::null_mut()) }.is_null()
        );
    }

    #[repr(C)]
    struct CallbackState {
        calls: usize,
        generation: u64,
    }

    unsafe extern "C" fn cancel_start(context: *mut c_void, generation: u64) -> bool {
        assert!(!context.is_null());
        // SAFETY: the test retains this state through the synchronous request.
        let state = unsafe { &mut *context.cast::<CallbackState>() };
        state.calls += 1;
        state.generation = generation;
        false
    }

    unsafe extern "C" fn accept_start(context: *mut c_void, generation: u64) -> bool {
        assert!(!context.is_null());
        // SAFETY: the test retains this state through the synchronous request.
        let state = unsafe { &mut *context.cast::<CallbackState>() };
        state.calls += 1;
        state.generation = generation;
        true
    }

    fn prepared_session(root: &Path) -> (*mut RecordingSession, PathBuf) {
        let request = json!({
            "recovery_root": root,
            "options": {
                "kind":"video",
                "target":{"type":"display","display_id":"fixture"},
                "frames_per_second":30,
                "max_resolution":"original",
                "countdown_seconds":0,
                "show_cursor":true,
                "highlight_clicks":false,
                "show_keystrokes":false,
                "audio":{
                    "capture_system_audio":false,
                    "microphone_device_id":null,
                    "mono_output":false,
                    "system_volume_percent":100,
                    "microphone_volume_percent":100,
                    "microphone_muted":false
                },
                "gif":{"max_width":800,"max_colors":256,"optimize":true}
            },
            "display":{
                "id":"fixture","name":"Fixture","x":0,"y":0,
                "width":1280,"height":720,"scale_factor":1.0,"is_primary":true
            }
        });
        let request = CString::new(request.to_string()).unwrap();
        let mut output = ptr::null_mut();
        let handle = unsafe { captures_recording_prepare_v1(request.as_ptr(), &mut output) };
        assert!(!handle.is_null());
        let prepared = take(output);
        assert_eq!(prepared["ok"], true);
        let id = prepared["result"]["snapshot"]["id"].as_str().unwrap();
        let bundle = root.join(id);
        assert!(bundle.is_dir());
        (handle, bundle)
    }

    #[test]
    fn microphone_level_is_owned_read_only_json_without_snapshot_shape_changes() {
        let root = tempfile::tempdir().unwrap();
        let (handle, bundle) = prepared_session(root.path());
        let manifest = bundle.join("manifest.json");
        let before = std::fs::read(&manifest).unwrap();
        let level = CString::new(r#"{"operation":"microphone_level"}"#).unwrap();
        let snapshot = CString::new(r#"{"operation":"snapshot"}"#).unwrap();
        let initial = take(unsafe {
            captures_recording_request_v1(handle, snapshot.as_ptr(), None, ptr::null_mut())
        });
        assert!(
            initial["result"]["snapshot"]
                .get("microphone_peak")
                .is_none()
        );
        for _ in 0..10 {
            assert_eq!(
                take(unsafe {
                    captures_recording_request_v1(handle, level.as_ptr(), None, ptr::null_mut())
                }),
                json!({"ok":true,"result":{"microphone_peak":0.0}})
            );
        }
        assert_eq!(std::fs::read(&manifest).unwrap(), before);
        assert_eq!(
            take(unsafe {
                captures_recording_request_v1(handle, snapshot.as_ptr(), None, ptr::null_mut())
            })["result"]["snapshot"],
            initial["result"]["snapshot"]
        );
        let bad = CString::new(r#"{"operation":"microphone_level","unexpected":true"#).unwrap();
        assert_eq!(
            take(unsafe {
                captures_recording_request_v1(handle, bad.as_ptr(), None, ptr::null_mut())
            })["ok"],
            false
        );
        assert_eq!(
            take(unsafe {
                captures_recording_request_v1(handle, ptr::null(), None, ptr::null_mut())
            })["error"],
            "string pointer is null"
        );
        assert_eq!(
            take(unsafe {
                captures_recording_request_v1(
                    ptr::null_mut(),
                    level.as_ptr(),
                    None,
                    ptr::null_mut(),
                )
            })["error"],
            "recording session pointer is null"
        );
        unsafe { captures_recording_free_v1(handle) };
    }

    #[test]
    fn owned_session_cancels_before_engine_open_and_removes_recovery_bundle() {
        let root = tempfile::tempdir().unwrap();
        let (handle, bundle) = prepared_session(root.path());

        let generation = 0x1234_5678_u64;
        let start = CString::new(
            json!({"operation":"start","generation":generation,
                "exclude_captures_app":true})
            .to_string(),
        )
        .unwrap();
        let mut callback = CallbackState {
            calls: 0,
            generation: 0,
        };
        let cancelled = take(unsafe {
            captures_recording_request_v1(
                handle,
                start.as_ptr(),
                Some(cancel_start),
                (&raw mut callback).cast(),
            )
        });
        assert_eq!(cancelled["ok"], false);
        assert_eq!(cancelled["error"], "Recording cancelled");
        assert_eq!(
            callback.calls, 1,
            "engine must not open after the first stale check"
        );
        assert_eq!(callback.generation, generation);
        assert!(
            !bundle.exists(),
            "cancelled start discards durable recovery state"
        );
        unsafe { captures_recording_free_v1(handle) };
    }

    #[test]
    fn restart_request_is_serialized_and_rejects_countdown_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let (handle, bundle) = prepared_session(root.path());
        let restart = CString::new(r#"{"operation":"restart"}"#).unwrap();
        let response = take(unsafe {
            captures_recording_request_v1(handle, restart.as_ptr(), None, ptr::null_mut())
        });
        assert_eq!(response["ok"], false);
        assert_eq!(
            response["error"],
            "Recording is not running, paused, or failed"
        );
        assert!(bundle.is_dir());

        let discard = CString::new(r#"{"operation":"discard"}"#).unwrap();
        let discarded = take(unsafe {
            captures_recording_request_v1(handle, discard.as_ptr(), None, ptr::null_mut())
        });
        assert_eq!(discarded["ok"], true);
        assert!(!bundle.exists());
        unsafe { captures_recording_free_v1(handle) };
    }

    #[test]
    fn shared_flow_cancellation_rejects_start_even_when_host_gate_accepts() {
        let root = tempfile::tempdir().unwrap();
        let (handle, bundle) = prepared_session(root.path());
        let generation = 0x1234_567c_u64;
        assert!(!captures_app::capture_flow::is_current(generation));
        let start = CString::new(
            json!({"operation":"start","generation":generation,
                "exclude_captures_app":true})
            .to_string(),
        )
        .unwrap();
        let mut callback = CallbackState {
            calls: 0,
            generation: 0,
        };
        let cancelled = take(unsafe {
            captures_recording_request_v1(
                handle,
                start.as_ptr(),
                Some(accept_start),
                (&raw mut callback).cast(),
            )
        });
        assert_eq!(cancelled["ok"], false);
        assert_eq!(cancelled["error"], "Recording cancelled");
        assert_eq!(
            callback.calls, 1,
            "host and shared cancellation gates are both checked"
        );
        assert!(!bundle.exists());
        unsafe { captures_recording_free_v1(handle) };
    }

    #[test]
    fn microphone_mute_requires_both_host_and_shared_generation_gates() {
        let root = tempfile::tempdir().unwrap();
        let (handle, bundle) = prepared_session(root.path());
        let generation = 0x1234_5680_u64;
        let mute = CString::new(
            json!({
                "operation":"set_microphone_muted",
                "muted":true,
                "generation":generation,
                "exclude_captures_app":true
            })
            .to_string(),
        )
        .unwrap();

        let mut callback = CallbackState {
            calls: 0,
            generation: 0,
        };
        let host_stale = take(unsafe {
            captures_recording_request_v1(
                handle,
                mute.as_ptr(),
                Some(cancel_start),
                (&raw mut callback).cast(),
            )
        });
        assert_eq!(host_stale["ok"], false);
        assert_eq!(host_stale["error"], "Recording cancelled");
        assert_eq!(callback.calls, 1);
        assert_eq!(callback.generation, generation);

        callback.calls = 0;
        let shared_stale = take(unsafe {
            captures_recording_request_v1(
                handle,
                mute.as_ptr(),
                Some(accept_start),
                (&raw mut callback).cast(),
            )
        });
        assert_eq!(shared_stale["ok"], false);
        assert_eq!(shared_stale["error"], "Recording cancelled");
        assert_eq!(callback.calls, 1);
        assert!(bundle.is_dir(), "stale mute must not discard the session");

        let discard = CString::new(r#"{"operation":"discard"}"#).unwrap();
        let discarded = take(unsafe {
            captures_recording_request_v1(handle, discard.as_ptr(), None, ptr::null_mut())
        });
        assert_eq!(discarded["ok"], true);
        unsafe { captures_recording_free_v1(handle) };
    }
}
