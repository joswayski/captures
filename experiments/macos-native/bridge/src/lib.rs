mod engine;
mod image_encoder;
mod protocol;
mod storage;

use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

use protocol::failure_json;

fn allocate_response(response: String) -> *mut c_char {
    let response = CString::new(response).unwrap_or_else(|_| {
        CString::new(failure_json("bridge produced an invalid response"))
            .expect("static bridge error has no NUL")
    });
    response.into_raw()
}

/// Dispatch one synchronous JSON request to the bridge's background engine.
///
/// The returned UTF-8 string belongs to the caller and must be released with
/// [`captures_native_free`]. A null input is returned as a JSON error.
///
/// # Safety
///
/// A non-null `json` must point to a valid NUL-terminated byte string that
/// remains alive for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_native_request(json: *const c_char) -> *mut c_char {
    let response = catch_unwind(AssertUnwindSafe(|| {
        if json.is_null() {
            return failure_json("request pointer is null");
        }
        // SAFETY: the C contract requires a non-null, NUL-terminated string
        // that remains valid for this call. We do not retain the borrowed data.
        let request = unsafe { CStr::from_ptr(json) };
        let request = match request.to_str() {
            Ok(request) => request,
            Err(_) => return failure_json("request is not valid UTF-8"),
        };
        engine::request(request.to_owned())
    }))
    .unwrap_or_else(|_| failure_json("bridge request panicked"));
    allocate_response(response)
}

/// Release a string returned by [`captures_native_request`]. Null is allowed.
///
/// # Safety
///
/// A non-null `value` must be a pointer returned by
/// [`captures_native_request`] that has not previously been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_native_free(value: *mut c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !value.is_null() {
            // SAFETY: the C contract only permits pointers returned by
            // captures_native_request, exactly once.
            drop(unsafe { CString::from_raw(value) });
        }
    }));
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};

    use super::{captures_native_free, captures_native_request};

    fn ffi(request: &str) -> serde_json::Value {
        let request = CString::new(request).unwrap();
        // SAFETY: the input is a live CString and the returned allocation is
        // read before being returned to the bridge exactly once.
        let response = unsafe { captures_native_request(request.as_ptr()) };
        let json = unsafe { CStr::from_ptr(response) }
            .to_str()
            .unwrap()
            .to_owned();
        unsafe { captures_native_free(response) };
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn ffi_reports_malformed_json_and_unknown_operations() {
        assert_eq!(ffi("{")["ok"], false);
        assert!(ffi("{")["error"].as_str().unwrap().contains("valid JSON"));

        let unknown = ffi(r#"{"op":"launch_missiles"}"#);
        assert_eq!(unknown["ok"], false);
        assert_eq!(unknown["error"], "unknown operation 'launch_missiles'");
    }

    #[test]
    fn ffi_rejects_missing_operation_and_null_input() {
        assert_eq!(ffi("{}")["error"], "request field 'op' is required");
        // SAFETY: null is an explicitly supported error input.
        let response = unsafe { captures_native_request(std::ptr::null()) };
        let parsed: serde_json::Value =
            serde_json::from_str(unsafe { CStr::from_ptr(response) }.to_str().unwrap()).unwrap();
        assert_eq!(parsed["error"], "request pointer is null");
        unsafe { captures_native_free(response) };
    }
}
