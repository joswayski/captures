use captures_app::shortcuts::{
    CaptureShortcuts, ShortcutKeyEvent, ShortcutPlatform, record_shortcut, shortcut_display_tokens,
};
use captures_settings::AppSettings;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

thread_local! {
    static SHORTCUTS: RefCell<Option<CaptureShortcuts>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Configure {
        settings: Box<AppSettings>,
    },
    Enabled {
        enabled: bool,
    },
    Suspended {
        suspended: bool,
    },
    Selector {
        generation: Option<u64>,
    },
    Next,
    Close,
    Record {
        event: ShortcutKeyEvent,
        platform: ShortcutPlatform,
    },
    Display {
        shortcut: String,
        platform: ShortcutPlatform,
    },
}

fn response(request: Request, wake: Option<extern "C" fn()>) -> Result<Value, String> {
    SHORTCUTS.with(|slot| {
        let mut slot = slot.borrow_mut();
        match request {
            Request::Configure { settings } => {
                if let Some(shortcuts) = slot.as_mut() {
                    shortcuts.update(&settings)?;
                } else {
                    let wake = wake.ok_or("A shortcut wake callback is required")?;
                    *slot = Some(CaptureShortcuts::new(&settings, move || wake())?);
                }
                Ok(json!({}))
            }
            Request::Enabled { enabled } => {
                slot.as_ref()
                    .ok_or("Capture shortcuts are not configured")?
                    .set_enabled(enabled);
                Ok(json!({}))
            }
            Request::Suspended { suspended } => {
                slot.as_mut()
                    .ok_or("Capture shortcuts are not configured")?
                    .set_suspended(suspended)?;
                Ok(json!({}))
            }
            Request::Selector { generation } => {
                slot.as_ref()
                    .ok_or("Capture shortcuts are not configured")?
                    .set_selector_generation(generation);
                Ok(json!({}))
            }
            Request::Next => {
                Ok(json!({"action":slot.as_ref().and_then(CaptureShortcuts::next_action)}))
            }
            Request::Close => {
                *slot = None;
                Ok(json!({}))
            }
            Request::Record { event, platform } => Ok(json!(record_shortcut(&event, platform))),
            Request::Display { shortcut, platform } => {
                Ok(json!({"keys": shortcut_display_tokens(&shortcut, platform)}))
            }
        }
    })
}

/// Event-loop-thread-only shortcut registry. `wake` may run on a hotkey worker:
/// it must only schedule host work and must never synchronously reenter this ABI.
/// Configure copies settings. Close before app teardown; late wakes are harmless
/// because Next reads the current queue, not a callback's old action payload.
/// Record/Display are pure and work in fixture Preferences without an OS owner.
///
/// # Safety
/// `request_json` must be readable NUL-terminated UTF-8 for this call. `wake`
/// must remain callable for the process lifetime. Free returned JSON exactly
/// once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_shortcuts_request_v1(
    request_json: *const c_char,
    wake: Option<extern "C" fn()>,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return Err("request pointer is null".to_owned());
        }
        // SAFETY: caller provides a readable terminated string during this call.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > crate::MAX_REQUEST_BYTES {
            return Err("request exceeds 8 MiB".to_owned());
        }
        let request = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        response(request, wake)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let envelope = match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    CString::new(envelope.to_string())
        .expect("JSON has no NUL bytes")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(request: &str) -> Value {
        let request = CString::new(request).unwrap();
        // SAFETY: owned input lives through call; result is freed exactly once.
        unsafe {
            let pointer = captures_shortcuts_request_v1(request.as_ptr(), None);
            let value = serde_json::from_slice(CStr::from_ptr(pointer).to_bytes()).unwrap();
            crate::captures_settings_free_v1(pointer);
            value
        }
    }

    #[test]
    fn unconfigured_calls_do_not_touch_os_hotkeys_and_errors_are_enveloped() {
        assert_eq!(
            call(r#"{"operation":"next"}"#)["result"]["action"],
            Value::Null
        );
        assert_eq!(
            call(r#"{"operation":"enabled","enabled":true}"#)["ok"],
            false
        );
        assert_eq!(
            call(r#"{"operation":"suspended","suspended":true}"#)["error"],
            "Capture shortcuts are not configured"
        );
        for generation in [json!(2), Value::Null] {
            assert_eq!(
                call(&json!({"operation":"selector","generation":generation}).to_string())["error"],
                "Capture shortcuts are not configured"
            );
        }
        assert_eq!(call(r#"{"operation":"close"}"#)["ok"], true);
        assert_eq!(call(r#"{"operation":"unsupported"}"#)["ok"], false);
        assert_eq!(call("not JSON")["ok"], false);
        let request = json!({"operation":"configure","settings":AppSettings::default()});
        assert_eq!(
            call(&request.to_string())["error"],
            "A shortcut wake callback is required"
        );
    }

    #[test]
    fn fixture_recording_and_display_need_neither_registration_nor_wake_callback() {
        let mut request = json!({"operation":"record", "platform":"macos", "event": {
            "code":"KeyQ", "ctrlKey":false, "shiftKey":true, "altKey":false, "metaKey":true
        }});
        assert_eq!(
            call(&request.to_string())["result"],
            json!({
                "kind":"complete", "keys":["Shift","Cmd","Q"], "shortcut":"Shift+Super+KeyQ"
            })
        );
        request["event"]["code"] = json!("Escape");
        assert_eq!(
            call(&request.to_string())["result"],
            json!({"kind":"cancel"})
        );
        assert_eq!(
            call(r#"{"operation":"display","platform":"windows","shortcut":"Super+Shift+KeyS"}"#)["result"],
            json!({"keys":["Win","Shift","S"]})
        );
        assert_eq!(
            call(r#"{"operation":"enabled","enabled":true}"#)["ok"],
            false
        );
    }
}
