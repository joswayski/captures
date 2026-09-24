use captures_app::instance::{Instance, Launch, OpenRequest};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
};

thread_local! {
    static INSTANCE: RefCell<Option<Instance>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Start {
        history_root: Option<PathBuf>,
        paths: Vec<PathBuf>,
    },
    Next,
    Stop,
    Close,
}

fn response(request: Request, wake: Option<extern "C" fn()>) -> Result<Value, String> {
    INSTANCE.with(|slot| {
        let mut slot = slot.borrow_mut();
        match request {
            Request::Start {
                history_root,
                paths,
            } => {
                if slot.is_some() {
                    return Err("Native instance is already started".into());
                }
                let wake = wake.ok_or("A native instance wake callback is required")?;
                let request = OpenRequest::from_paths(paths).map_err(|error| error.to_string())?;
                match Instance::start(
                    &history_root.unwrap_or_else(captures_app::default_history_root),
                    request,
                )
                .map_err(|error| error.to_string())?
                {
                    Launch::Primary(instance) => {
                        instance.set_wake(move || wake());
                        *slot = Some(instance);
                        Ok(json!({"primary": true}))
                    }
                    Launch::Forwarded => Ok(json!({"primary": false})),
                }
            }
            Request::Next => {
                let request = slot
                    .as_ref()
                    .map(Instance::next_request)
                    .transpose()
                    .map_err(|error| error.to_string())?
                    .flatten();
                Ok(json!({"request": request}))
            }
            Request::Stop => {
                if let Some(instance) = slot.as_mut() {
                    instance.stop_accepting();
                }
                Ok(json!({}))
            }
            Request::Close => {
                *slot = None;
                Ok(json!({}))
            }
        }
    })
}

/// Native-thread-only owner. Start BEFORE any native UI or capture workers,
/// only in live mode. A secondary returns primary=false after queue acceptance.
/// `wake` schedules native-thread work; it must never synchronously reenter.
/// Next consumes one bounded request; close joins the server before releasing
/// election. Callback storage must remain callable for the process lifetime.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 during this call. Free returned
/// JSON exactly once with captures_settings_free_v1. No pointers are retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_instance_request_v1(
    request_json: *const c_char,
    wake: Option<extern "C" fn()>,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return Err("request pointer is null".to_owned());
        }
        // SAFETY: caller supplies a readable terminated string for this call.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > crate::MAX_REQUEST_BYTES {
            return Err("request exceeds 8 MiB".into());
        }
        let request = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        response(request, wake)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let envelope = match result {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    CString::new(envelope.to_string())
        .expect("JSON has no NUL bytes")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn wake() {}

    #[test]
    fn instance_start_next_close_and_owned_envelopes() {
        let root = tempfile::tempdir().unwrap();
        let call = |request: Value| {
            let request = CString::new(request.to_string()).unwrap();
            // SAFETY: input remains owned, returned allocation freed exactly once.
            unsafe {
                let result = captures_instance_request_v1(request.as_ptr(), Some(wake));
                let value: Value =
                    serde_json::from_slice(CStr::from_ptr(result).to_bytes()).unwrap();
                crate::captures_settings_free_v1(result);
                value
            }
        };
        assert_eq!(
            call(json!({"operation":"start", "history_root": root.path(), "paths": []}))["result"]
                ["primary"],
            true
        );
        assert_eq!(
            call(json!({"operation":"start", "history_root": root.path(), "paths": []}))["ok"],
            false
        );
        assert!(matches!(
            Instance::start(
                root.path(),
                OpenRequest::from_paths(vec!["relative.mp4".into()]).unwrap()
            )
            .unwrap(),
            Launch::Forwarded
        ));
        assert_eq!(
            call(json!({"operation":"next"}))["result"]["request"]["paths"][0],
            std::env::current_dir()
                .unwrap()
                .join("relative.mp4")
                .to_str()
                .unwrap()
        );
        assert!(call(json!({"operation":"next"}))["result"]["request"].is_null());
        assert_eq!(call(json!({"operation":"close"}))["ok"], true);
        assert!(matches!(
            Instance::start(root.path(), OpenRequest { paths: vec![] }).unwrap(),
            Launch::Primary(_)
        ));
    }
}
