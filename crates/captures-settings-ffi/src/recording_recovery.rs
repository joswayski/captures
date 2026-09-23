//! Additive blocking interrupted-recording recovery. Hosts serialize these
//! calls with their live recording worker and derive roots from their profile.
use super::region::{response, text};
use captures_media::{CancelToken, MediaToolchain};
use captures_recording_platform::{RecordingRecovery, RecoveryProgress};
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::{c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    history_root: PathBuf,
    #[serde(default)]
    ffmpeg: Option<PathBuf>,
    #[serde(default)]
    ffprobe: Option<PathBuf>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    expected_identity: Option<String>,
}

type Progress = Option<unsafe extern "C" fn(*mut c_void, *const c_char)>;

fn parse(request: *const c_char) -> Result<(Request, RecordingRecovery), String> {
    // SAFETY: caller retains the readable NUL-terminated string for this call.
    let request: Request =
        serde_json::from_str(unsafe { text(request) }?).map_err(|error| error.to_string())?;
    let tools = MediaToolchain::new(
        request.ffmpeg.clone().unwrap_or_else(|| "ffmpeg".into()),
        request.ffprobe.clone().unwrap_or_else(|| "ffprobe".into()),
    );
    let recovery = RecordingRecovery::new(request.history_root.clone(), tools);
    Ok((request, recovery))
}

fn action(request: &Request) -> Result<(&str, &str), String> {
    Ok((
        request
            .session_id
            .as_deref()
            .ok_or("session_id is required")?,
        request
            .expected_identity
            .as_deref()
            .ok_or("expected_identity is required")?,
    ))
}

/// Owned JSON response; free with captures_settings_free_v1.
/// # Safety
/// Request is readable UTF-8 through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_recovery_list_v1(
    request: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let (_, recovery) = parse(request)?;
        recovery.list().map(|drafts| json!({"drafts":drafts}))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// The cancel owner may be signaled concurrently; do not free it until return.
/// Progress JSON is borrowed for the callback only.
/// # Safety
/// Request/cancel/context stay live through the call; callback may be invoked
/// from the calling worker and must copy progress it retains.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_recovery_recover_v1(
    request: *const c_char,
    cancel: *const CancelToken,
    progress: Progress,
    context: *mut c_void,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let (request, recovery) = parse(request)?;
        let (session_id, identity) = action(&request)?;
        // SAFETY: the cancel owner is retained by the caller until return.
        let cancel = unsafe { cancel.as_ref() }.ok_or("recovery cancel handle is null")?;
        recovery.recover(session_id, identity, cancel, |stage: RecoveryProgress| {
            if let Some(callback) = progress
                && let Ok(bytes) = std::ffi::CString::new(json!({"stage":stage}).to_string())
            {
                // SAFETY: callback/context remain valid for the call.
                unsafe { callback(context, bytes.as_ptr()) };
            }
        })
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Explicit permanent discard of a listed, unchanged, owned bundle only.
/// # Safety
/// Request is readable UTF-8 through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_recovery_discard_v1(
    request: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let (request, recovery) = parse(request)?;
        let (session_id, identity) = action(&request)?;
        recovery
            .discard(session_id, identity)
            .map(|status| json!({"status":status}))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    fn owned(ptr: *mut c_char) -> serde_json::Value {
        let result = serde_json::from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(ptr) };
        result
    }

    #[test]
    fn recovery_abi_envelopes_and_owned_json() {
        let root = tempfile::tempdir().unwrap();
        let request =
            CString::new(json!({"history_root": root.path().join("history")}).to_string()).unwrap();
        let list = owned(unsafe { captures_recording_recovery_list_v1(request.as_ptr()) });
        assert_eq!(list, json!({"ok":true,"result":{"drafts":[]}}));
        let recover = owned(unsafe {
            captures_recording_recovery_recover_v1(
                request.as_ptr(),
                std::ptr::null(),
                None,
                std::ptr::null_mut(),
            )
        });
        assert_eq!(recover["ok"], false);
        assert!(recover["error"].as_str().unwrap().contains("session_id"));
        let discard = owned(unsafe { captures_recording_recovery_discard_v1(request.as_ptr()) });
        assert_eq!(discard["ok"], false);
        let malformed = CString::new("not JSON").unwrap();
        assert_eq!(
            owned(unsafe { captures_recording_recovery_list_v1(malformed.as_ptr()) })["ok"],
            false
        );
        assert_eq!(
            owned(unsafe { captures_recording_recovery_list_v1(std::ptr::null()) })["ok"],
            false
        );
    }
}
