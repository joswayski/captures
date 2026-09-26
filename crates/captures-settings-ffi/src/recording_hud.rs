//! Pure recording HUD policy for AppKit: control states, copy, the inline error
//! line and tooltip placement. See `captures_app::recording_hud`.
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use captures_app::{
    recording_hud::{self, ErrorEvent, ErrorLine, Input},
    tray_notice::LogicalRect,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Present(Input),
    ErrorMessage {
        message: String,
    },
    ErrorLine {
        #[serde(default)]
        line: ErrorLine,
        event: ErrorEvent,
        session_error: Option<String>,
    },
    TooltipFrame {
        anchor: LogicalRect,
        text_width: f64,
        text_height: f64,
        right_aligned: bool,
        progress: f64,
        bounds: LogicalRect,
    },
}

fn finite(values: &[f64]) -> bool {
    values.iter().all(|value| value.is_finite())
}

fn respond(bytes: &[u8]) -> Result<Value, String> {
    match serde_json::from_slice::<Request>(bytes).map_err(|error| error.to_string())? {
        Request::Present(input) => Ok(json!(recording_hud::present(&input))),
        Request::ErrorMessage { message } => {
            Ok(json!({"message": recording_hud::error_message(&message)}))
        }
        Request::ErrorLine {
            mut line,
            event,
            session_error,
        } => {
            let changed = line.apply(event);
            let text = line.line(session_error.as_deref());
            Ok(json!({"line": line, "text": text, "changed": changed}))
        }
        Request::TooltipFrame {
            anchor,
            text_width,
            text_height,
            right_aligned,
            progress,
            bounds,
        } => {
            let rects = [anchor, bounds];
            if !rects
                .iter()
                .all(|rect| finite(&[rect.x, rect.y, rect.width, rect.height]))
                || !finite(&[text_width, text_height, progress])
            {
                return Err("Tooltip placement needs finite geometry".into());
            }
            Ok(json!(recording_hud::tooltip_frame(
                anchor,
                text_width.max(0.),
                text_height.max(0.),
                right_aligned,
                progress,
                bounds,
            )))
        }
    }
}

/// UI-thread-safe pure recording HUD requests. See the header for operations.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 for this call; free the
/// returned owned JSON exactly once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_hud_request_v1(
    request_json: *const c_char,
) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return json!({"ok":false,"error":"request pointer is null"});
        }
        // SAFETY: Caller upholds the same contract as the other JSON ABIs.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > 64 * 1024 {
            return json!({"ok":false,"error":"recording HUD request exceeds 64 KiB"});
        }
        match respond(bytes) {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false,"error":"internal panic"}));
    CString::new(value.to_string())
        .expect("JSON contains no NUL bytes")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(request: &str) -> Value {
        let input = CString::new(request).unwrap();
        // SAFETY: A valid C string; the owned response is read then freed once.
        unsafe {
            let pointer = captures_recording_hud_request_v1(input.as_ptr());
            let value = serde_json::from_slice(CStr::from_ptr(pointer).to_bytes()).unwrap();
            crate::captures_settings_free_v1(pointer);
            value
        }
    }

    #[test]
    fn presents_failed_takes_error_lines_and_tooltips_over_the_abi() {
        let failed = call(r#"{"operation":"present","state":"failed","has_microphone":true}"#);
        assert_eq!(failed["ok"], true);
        let result = &failed["result"];
        assert_eq!(result["status_label"], "Failed");
        assert_eq!(result["dot_token"], "glass-text-subtle");
        assert_eq!(result["restart_confirms"], false);
        let restart = &result["controls"][2];
        assert_eq!(restart["control"], "restart");
        assert_eq!(restart["label"], "Retry recording");
        assert_eq!(restart["enabled"], true);
        assert_eq!(result["controls"][0]["enabled"], false);
        assert_eq!(result["controls"][6]["tooltip"], "Hide controls");

        let message = call(r#"{"operation":"error_message","message":"error: the mic"}"#);
        assert_eq!(message["result"]["message"], "The mic");

        let warned =
            call(r#"{"operation":"error_line","event":{"event":"warning","warning":"Mic gone"}}"#);
        assert_eq!(warned["result"]["text"], "Mic gone");
        assert_eq!(warned["result"]["changed"], true);
        let line = &warned["result"]["line"];
        let cleared = call(
            &json!({"operation":"error_line","line":line,"event":{"event":"action_started"},
                "session_error":null})
            .to_string(),
        );
        assert_eq!(cleared["result"]["text"], Value::Null);
        let repeated = call(
            &json!({"operation":"error_line","line":cleared["result"]["line"],
                "event":{"event":"warning","warning":"Mic gone"}})
            .to_string(),
        );
        assert_eq!(repeated["result"]["text"], Value::Null);

        let frame = call(
            r#"{"operation":"tooltip_frame","anchor":{"x":384,"y":39,"width":38,"height":34},
            "text_width":70,"text_height":13,"right_aligned":true,"progress":1,
            "bounds":{"x":0,"y":0,"width":430,"height":102}}"#,
        );
        assert_eq!(frame["result"]["x"], 334.0);

        for request in [
            r#"{"operation":"present","state":"nope"}"#,
            r#"{"operation":"error_line","event":{"event":"nope"}}"#,
            "not json",
        ] {
            assert_eq!(call(request)["ok"], false, "{request}");
        }
        // SAFETY: Null is accepted and reported as an error.
        let pointer = unsafe { captures_recording_hud_request_v1(std::ptr::null()) };
        assert!(!pointer.is_null());
        // SAFETY: Owned response freed once.
        unsafe { crate::captures_settings_free_v1(pointer) };
    }
}
