use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
};

thread_local! {
    static SESSION: RefCell<Option<captures_app::crash::Session>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Start { history_root: Option<PathBuf> },
    Preview,
    Dismiss,
    CleanExit,
    Resume,
}

fn response(request: Request) -> Result<Value, String> {
    SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Request::Start { history_root } = request {
            if slot.is_some() {
                return Err("Local diagnostics are already started.".into());
            }
            *slot = Some(captures_app::crash::Session::start(
                &history_root.unwrap_or_else(captures_app::default_history_root),
            )?);
            return Ok(json!({"preview":slot.as_ref().unwrap().preview()}));
        }
        let session = slot.as_ref().ok_or("Local diagnostics are not started.")?;
        match request {
            Request::Preview => Ok(json!({"preview":session.preview()})),
            Request::Dismiss => {
                session.dismiss()?;
                Ok(json!({}))
            }
            Request::CleanExit => {
                session.clean_exit()?;
                Ok(json!({}))
            }
            Request::Resume => {
                session.resume()?;
                Ok(json!({}))
            }
            Request::Start { .. } => unreachable!(),
        }
    })
}

/// Main-thread-only. Start after election; no operation sends feedback.
/// # Safety
/// Input is readable NUL-terminated UTF-8 during this call. Free the owned
/// response once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_crash_request_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return Err("request pointer is null".into());
        }
        // SAFETY: caller supplies a readable terminated string during this call.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > 64 * 1024 {
            return Err("diagnostics request exceeds 64 KiB".into());
        }
        response(
            serde_json::from_slice(bytes).map_err(|_| "invalid diagnostics request".to_owned())?,
        )
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let value = match result {
        Ok(result) => json!({"ok":true, "result":result}),
        Err(error) => json!({"ok":false, "error":error}),
    };
    CString::new(value.to_string())
        .expect("JSON has no NUL")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unstarted_and_invalid_requests_never_read_profiles_or_send() {
        for request in [
            r#"{"operation":"preview"}"#,
            r#"{"operation":"clean_exit"}"#,
            r#"{"operation":"dismiss"}"#,
            r#"{"operation":"resume"}"#,
            r#"{"operation":"preview","profile":"other"}"#,
            "bad json",
        ] {
            let input = CString::new(request).unwrap();
            let pointer = unsafe { captures_crash_request_v1(input.as_ptr()) };
            let value: Value =
                serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
            unsafe { crate::captures_settings_free_v1(pointer) };
            assert_eq!(value["ok"], false);
        }
    }

    #[test]
    fn owned_ffi_session_preview_dismiss_and_failed_restart_resume_in_isolated_process() {
        const PROFILE: &str = "CAPTURES_CRASH_FFI_TEST_PROFILE";
        if let Some(root) = std::env::var_os(PROFILE) {
            let root = PathBuf::from(root);
            let call = |request: Value| {
                let input = CString::new(request.to_string()).unwrap();
                let pointer = unsafe { captures_crash_request_v1(input.as_ptr()) };
                let value: Value =
                    serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
                unsafe { crate::captures_settings_free_v1(pointer) };
                value
            };
            let start = call(json!({"operation":"start", "history_root":root}));
            assert_eq!(start["ok"], true, "{start}");
            assert_eq!(start["result"]["preview"]["has_exception_evidence"], true);
            assert!(
                start["result"]["preview"]["summary"]
                    .as_str()
                    .unwrap()
                    .contains("~/fixture.rs")
            );
            assert_eq!(
                call(json!({"operation":"start", "history_root":root}))["ok"],
                false
            );
            assert_eq!(call(json!({"operation":"clean_exit"}))["ok"], true);
            assert!(!root.join(".crash-diagnostics/current-session").exists());
            assert_eq!(call(json!({"operation":"resume"}))["ok"], true);
            assert!(root.join(".crash-diagnostics/current-session").exists());
            assert_eq!(call(json!({"operation":"dismiss"}))["ok"], true);
            assert!(call(json!({"operation":"preview"}))["result"]["preview"].is_null());
            assert!(root.join(".crash-diagnostics/current-session").exists());
            assert_eq!(call(json!({"operation":"clean_exit"}))["ok"], true);
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let prior = captures_feedback::crash_diagnostics::CrashSession::start(root.path()).unwrap();
        std::fs::write(
            prior.clean_exit_paths()[1].clone(),
            "Fixture panic at /home/private-user/fixture.rs",
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash::tests::owned_ffi_session_preview_dismiss_and_failed_restart_resume_in_isolated_process"])
            .env(PROFILE, root.path()).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
