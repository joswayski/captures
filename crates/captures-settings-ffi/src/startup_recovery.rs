//! Main-thread lifetime owner for the AppKit startup companion.
use captures_app::updater::{HealthAcknowledgement, preview::recovery::Startup};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

thread_local! {
    static STARTUP: RefCell<Option<Startup>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Start {
        health: Option<HealthAcknowledgement>,
    },
    Finish,
}

fn response(request: Request) -> Result<Value, String> {
    STARTUP.with(|slot| {
        let mut slot = slot.borrow_mut();
        match request {
            Request::Start { health } => {
                if slot.is_some() {
                    return Err("Startup recovery already armed".into());
                }
                *slot = Startup::start(health).map_err(|error| error.to_string())?;
                Ok(json!({"health": slot.as_ref().map(|startup| &startup.acknowledgement)}))
            }
            Request::Finish => {
                if let Some(startup) = slot.take() {
                    startup.finish();
                }
                Ok(json!({}))
            }
        }
    })
}

/// Call start before migration, and finish only on normal Quit or forwarding.
/// Errors/crashes intentionally leave the helper to observe process exit.
///
/// # Safety
/// Main-thread-only. Input is a readable NUL-terminated UTF-8 string during the
/// call; free the returned JSON once with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_preview_recovery_request_v1(
    request: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if request.is_null() {
            return Err("request pointer is null".to_owned());
        }
        // SAFETY: caller supplies a readable terminated string for this call.
        let bytes = unsafe { CStr::from_ptr(request) }.to_bytes();
        if bytes.len() > 32 * 1024 {
            return Err("recovery request exceeds 32 KiB".into());
        }
        response(serde_json::from_slice(bytes).map_err(|error| error.to_string())?)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let envelope = match result {
        Ok(result) => json!({"ok":true, "result":result}),
        Err(error) => json!({"ok":false, "error":error}),
    };
    CString::new(envelope.to_string())
        .expect("JSON has no NUL bytes")
        .into_raw()
}
