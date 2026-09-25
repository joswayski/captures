//! The shipping vector icon set for native hosts.
use std::ffi::c_char;

use serde_json::json;

use crate::region::{response, text};

/// Polylines for a named shipping icon in its 24-unit viewBox (y down), as
/// owned JSON `{ok,result:[[[x,y],...],...]}` or `{ok:false,error}`. Stroke
/// every polyline with round caps and joins at 1.8 units. Free with
/// captures_settings_free_v1.
///
/// # Safety
/// `name` is null or a readable NUL-terminated UTF-8 string during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_icon_polylines_v1(name: *const c_char) -> *mut c_char {
    // SAFETY: The caller guarantees a readable C string or null.
    let name = match unsafe { text(name) } {
        Ok(name) => name,
        Err(error) => return response(json!({"ok":false,"error":error})),
    };
    match captures_app::icons::polylines(name) {
        Some(lines) => response(json!({"ok":true,"result":lines})),
        None => response(json!({"ok":false,"error":format!("unknown icon {name}")})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    #[test]
    fn named_icons_cross_the_abi_as_owned_json() {
        let take = |pointer: *mut c_char| {
            // SAFETY: Each export returns one owned string, read then freed once.
            unsafe {
                let value: serde_json::Value =
                    serde_json::from_slice(CStr::from_ptr(pointer).to_bytes()).unwrap();
                crate::captures_settings_free_v1(pointer);
                value
            }
        };
        let name = CString::new("pause").unwrap();
        // SAFETY: A valid C string for the duration of the call.
        let pause = take(unsafe { captures_icon_polylines_v1(name.as_ptr()) });
        assert_eq!(pause["ok"], true);
        assert_eq!(pause["result"], json!([[[8., 5.], [8., 19.]], [[16., 5.], [16., 19.]]]));
        let unknown = CString::new("nope").unwrap();
        // SAFETY: A valid C string for the duration of the call.
        let missing = take(unsafe { captures_icon_polylines_v1(unknown.as_ptr()) });
        assert_eq!(missing["ok"], false);
        // SAFETY: Null is accepted and reported as an error.
        let null = take(unsafe { captures_icon_polylines_v1(std::ptr::null()) });
        assert_eq!(null["ok"], false);
    }
}
