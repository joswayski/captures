use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    /// Shared form copy and limits (`captures_app::feedback::copy`).
    Copy,
    /// Local app/system details plus the shared `system_label`.
    Context,
    Submit {
        draft: captures_feedback::FeedbackDraft,
    },
}

/// Worker-only, explicit-consent feedback. No endpoint override or attachments.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 for this call; free the returned
/// owned JSON exactly once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_feedback_request_v1(request_json: *const c_char) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        let result = (|| -> Result<Value, String> {
            if request_json.is_null() {
                return Err("request pointer is null".into());
            }
            // SAFETY: Caller upholds the same contract as the other JSON ABIs.
            let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
            if bytes.len() > 64 * 1024 {
                return Err("feedback request exceeds 64 KiB".into());
            }
            match serde_json::from_slice::<Request>(bytes).map_err(|error| error.to_string())? {
                Request::Copy => Ok(captures_app::feedback::copy()),
                Request::Context => {
                    let context = captures_feedback::native::context();
                    let label = captures_app::feedback::system_label(
                        &context.os,
                        &context.os_version,
                        &context.arch,
                    );
                    let mut value = json!(context);
                    value["system_label"] = json!(label);
                    Ok(value)
                }
                Request::Submit { draft } => {
                    captures_feedback::native::submit(draft)?;
                    Ok(json!({}))
                }
            }
        })();
        match result {
            Ok(result) => json!({"ok":true, "result":result}),
            Err(error) => json!({"ok":false, "error":error}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false, "error":"internal panic"}));
    CString::new(value.to_string())
        .expect("JSON contains no NUL bytes")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_and_invalid_requests_never_send_feedback() {
        for (request, success) in [
            (r#"{"operation":"context"}"#, true),
            (r#"{"operation":"copy"}"#, true),
            (
                r#"{"operation":"submit","draft":{"message":"  ","category":"bug"}}"#,
                false,
            ),
            (r#"{"operation":"submit"}"#, false),
            ("invalid json", false),
        ] {
            let input = CString::new(request).unwrap();
            let pointer = unsafe { captures_feedback_request_v1(input.as_ptr()) };
            let value: Value =
                serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
            unsafe { crate::captures_settings_free_v1(pointer) };
            assert_eq!(value["ok"], success, "{value}");
            if request.contains("copy") {
                assert_eq!(value["result"]["title"], "Send feedback");
                assert_eq!(value["result"]["categories"].as_array().unwrap().len(), 3);
            } else if success {
                let result = value["result"].as_object().unwrap();
                assert_eq!(result.len(), 5);
                assert_eq!(
                    result["system_label"],
                    captures_app::feedback::system_label(
                        result["os"].as_str().unwrap(),
                        result["os_version"].as_str().unwrap(),
                        result["arch"].as_str().unwrap(),
                    )
                );
            }
        }
    }
}
