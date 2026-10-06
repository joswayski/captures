//! Signed native development checks and opt-in temporary acquisition. No install.
use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::Arc,
};

use captures_app::updater::{
    Renderer,
    checks::{CheckWorker, client_from_key_file},
};
use serde::Deserialize;
use serde_json::json;

pub struct CapturesUpdateChecks {
    worker: CheckWorker,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    endpoint: String,
    key_file: PathBuf,
    renderer: Renderer,
    current_version: String,
    staging_directory: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Check { show_changelog: Option<bool> },
    Poll { show_changelog: Option<bool> },
    DownloadVerify { show_changelog: Option<bool> },
    CancelDownload { show_changelog: Option<bool> },
}

/// Construct an idle checker from explicit configuration, without HTTP.
///
/// # Safety
/// `configuration_json` must be readable NUL-terminated UTF-8 for this call.
/// Exclusively own the returned handle and free it once with the matching free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_update_checks_create_v1(
    configuration_json: *const c_char,
) -> *mut CapturesUpdateChecks {
    catch_unwind(AssertUnwindSafe(|| {
        if configuration_json.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: Caller upholds the documented string lifetime.
        let bytes = unsafe { CStr::from_ptr(configuration_json) }.to_bytes();
        if bytes.len() > 64 * 1024 {
            return std::ptr::null_mut();
        }
        let Ok(config) = serde_json::from_slice::<Configuration>(bytes) else {
            return std::ptr::null_mut();
        };
        let Ok(client) = client_from_key_file(
            &config.endpoint,
            &config.key_file,
            config.renderer,
            &config.current_version,
        ) else {
            return std::ptr::null_mut();
        };
        let Ok(worker) = CheckWorker::new(client, Arc::new(|| {}), config.staging_directory) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(CapturesUpdateChecks { worker }))
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Nonblocking Check/Poll on the owning UI thread. Parse errors never echo input.
///
/// # Safety
/// `handle` is live and exclusively accessed; `request_json` is readable
/// NUL-terminated UTF-8. Free the returned owned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_update_checks_request_v1(
    handle: *mut CapturesUpdateChecks,
    request_json: *const c_char,
) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        let result = (|| {
            if handle.is_null() || request_json.is_null() {
                return Err("Update-check pointer is null.");
            }
            // SAFETY: Caller upholds the exclusive handle/string contracts.
            let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
            if bytes.len() > 64 * 1024 {
                return Err("Update-check request exceeds 64 KiB.");
            }
            let request = serde_json::from_slice::<Request>(bytes)
                .map_err(|_| "Invalid update-check command.")?;
            let worker = &mut unsafe { &mut *handle }.worker;
            worker.poll();
            let show_changelog = match request {
                Request::Check { show_changelog }
                | Request::Poll { show_changelog }
                | Request::DownloadVerify { show_changelog }
                | Request::CancelDownload { show_changelog } => show_changelog.unwrap_or(true),
            };
            let accepted = match request {
                Request::Check { .. } => worker.check(),
                Request::DownloadVerify { .. } => worker.download(),
                Request::CancelDownload { .. } => worker.cancel_download(),
                Request::Poll { .. } => false,
            };
            Ok(json!({"accepted":accepted, "checking":worker.checking(),
                "status":worker.status(), "presentation":worker.presentation(),
                "generation":worker.generation(), "notice":worker.notice(show_changelog)}))
        })();
        match result {
            Ok(result) => json!({"ok":true, "result":result}),
            Err(error) => json!({"ok":false, "error":error}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false, "error":"Update-check bridge failed."}));
    CString::new(value.to_string())
        .expect("JSON contains no NUL")
        .into_raw()
}

/// Cancel/join bounded HTTP off the UI thread before host termination.
///
/// # Safety
/// `handle` is null or an exclusively owned live handle from create. No requests
/// may overlap freeing or use it afterwards. A blocked request can take 60 s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_update_checks_free_v1(handle: *mut CapturesUpdateChecks) {
    if !handle.is_null() {
        // SAFETY: Caller transfers exclusive ownership exactly once.
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::net::TcpListener;

    fn call(handle: *mut CapturesUpdateChecks, request: &str) -> Value {
        let input = CString::new(request).unwrap();
        let pointer = unsafe { captures_update_checks_request_v1(handle, input.as_ptr()) };
        let value = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(pointer) };
        value
    }

    #[test]
    fn idle_poll_and_rejected_install_commands_never_request_or_expose_configuration() {
        let root = tempfile::tempdir().unwrap();
        let key_file = root.path().join("public.key");
        // Public key from the upstream Minisign compatibility fixture.
        std::fs::write(&key_file, "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let config = json!({"endpoint":format!("http://{}/private-endpoint", listener.local_addr().unwrap()),
            "key_file":key_file, "renderer":"wgpu", "current_version":"2026.9.99"});
        let input = CString::new(config.to_string()).unwrap();
        let handle = unsafe { captures_update_checks_create_v1(input.as_ptr()) };
        assert!(!handle.is_null());
        let reply = call(handle, r#"{"operation":"poll"}"#);
        assert_eq!(reply["result"]["status"]["state"], "idle");
        assert_eq!(
            reply["result"]["presentation"]["version"],
            "Native development 2026.9.99"
        );
        assert_eq!(reply["result"]["checking"], false);
        assert_eq!(reply["result"]["generation"], 0);
        assert!(reply["result"]["notice"].is_null());
        assert!(!reply.to_string().contains("private-endpoint"));
        for request in [
            r#"{"operation":"download_verify"}"#,
            r#"{"operation":"cancel_download"}"#,
        ] {
            let reply = call(handle, request);
            assert_eq!(reply["result"]["accepted"], false);
            assert_eq!(reply["result"]["generation"], 0);
            assert!(reply["result"]["presentation"]["acquisition"].is_null());
        }
        for request in [
            r#"{"operation":"install"}"#,
            r#"{"operation":"download"}"#,
            r#"{"operation":"check","endpoint":"private-secret"}"#,
        ] {
            assert_eq!(
                call(handle, request),
                json!({"ok":false,"error":"Invalid update-check command."})
            );
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        unsafe { captures_update_checks_free_v1(handle) };
        for path in [
            "relative".into(),
            key_file.clone(),
            root.path().join("missing"),
        ] {
            let mut invalid = config.clone();
            invalid["staging_directory"] = json!(path);
            let input = CString::new(invalid.to_string()).unwrap();
            assert!(unsafe { captures_update_checks_create_v1(input.as_ptr()) }.is_null());
        }
        let mut staged = config.clone();
        staged["staging_directory"] = json!(root.path());
        let input = CString::new(staged.to_string()).unwrap();
        let handle = unsafe { captures_update_checks_create_v1(input.as_ptr()) };
        assert!(!handle.is_null());
        assert!(
            call(handle, r#"{"operation":"poll"}"#)["result"]["presentation"]["acquisition"]
                .is_null()
        );
        unsafe { captures_update_checks_free_v1(handle) };
        let mut invalid = config;
        invalid["current_version"] = json!("invalid");
        let input = CString::new(invalid.to_string()).unwrap();
        assert!(unsafe { captures_update_checks_create_v1(input.as_ptr()) }.is_null());
    }
}
