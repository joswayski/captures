use captures_settings::AppSettings;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
};

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Load {
        path: String,
    },
    Save {
        path: String,
        settings: Box<AppSettings>,
    },
    Theme {
        accent: String,
        signal: String,
        light: bool,
    },
    DefaultPath,
}

fn response(request: *const c_char) -> Value {
    if request.is_null() {
        return json!({"ok":false,"error":"request pointer is null"});
    }
    // SAFETY: The ABI contract requires a readable NUL-terminated C string.
    let bytes = unsafe { CStr::from_ptr(request) }.to_bytes();
    if bytes.len() > MAX_REQUEST_BYTES {
        return json!({"ok":false,"error":"request exceeds 8 MiB"});
    }
    let parsed = std::str::from_utf8(bytes)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str::<Request>(s).map_err(|e| e.to_string()));
    match parsed {
        Ok(Request::Load { path }) => captures_settings::load(Path::new(&path))
            .map(|settings| json!({"ok":true,"settings":settings}))
            .unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()})),
        Ok(Request::Save { path, settings }) => {
            captures_settings::save(Path::new(&path), &settings)
                .map(|settings| json!({"ok":true,"settings":settings}))
                .unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}))
        }
        Ok(Request::DefaultPath) => {
            json!({"ok":true,"path":captures_settings::default_native_settings_path()})
        }
        Ok(Request::Theme {
            accent,
            signal,
            light,
        }) => captures_settings::theme::custom_colors(&accent, &signal, light)
            .map(|colors| json!({"ok":true,"colors":colors}))
            .unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()})),
        Err(error) => json!({"ok":false,"error":error}),
    }
}

/// Handles one JSON request. See `include/captures_settings.h` for pointer ownership.
///
/// # Safety
/// `request_json` must point to a readable NUL-terminated C string for the
/// duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_settings_request_v1(request_json: *const c_char) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| response(request_json)))
        .unwrap_or_else(|_| json!({"ok":false,"error":"internal panic"}));
    let text = serde_json::to_string(&value)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"serialization failed\"}".into());
    CString::new(text)
        .expect("JSON contains no NUL bytes")
        .into_raw()
}

/// Releases a response returned by `captures_settings_request_v1`.
///
/// # Safety
/// `response` must be null or a pointer returned by
/// `captures_settings_request_v1` that has not previously been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_settings_free_v1(response: *mut c_char) {
    if !response.is_null() {
        // SAFETY: The ABI requires this pointer to come from request_v1, exactly once.
        unsafe {
            drop(CString::from_raw(response));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(s: &str) -> Value {
        let input = CString::new(s).unwrap();
        let ptr = unsafe { captures_settings_request_v1(input.as_ptr()) };
        assert!(!ptr.is_null());
        let result = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        unsafe { captures_settings_free_v1(ptr) };
        serde_json::from_str(&result).unwrap()
    }
    #[test]
    fn success_error_and_release() {
        assert_eq!(call(r#"{"operation":"default_path"}"#)["ok"], true);
        let theme =
            call(r##"{"operation":"theme","accent":"#123abc","signal":"#de4567","light":true}"##);
        assert_eq!(theme["ok"], true);
        assert!(theme["colors"]["theme-accent"].is_array());
        assert_eq!(call("not json")["ok"], false);
        let ptr = unsafe { captures_settings_request_v1(std::ptr::null()) };
        assert!(!ptr.is_null());
        unsafe { captures_settings_free_v1(ptr) };
        unsafe { captures_settings_free_v1(std::ptr::null_mut()) };
    }
}
