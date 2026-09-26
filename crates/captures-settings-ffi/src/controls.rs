//! Shipping `NumberInput` stepping for AppKit, from `captures-app::controls`
//! (shared with the wgpu host). Called once per stepper click or arrow key.
use super::region::{response, text};
use captures_app::controls::number;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
};

fn one() -> f64 {
    1.
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum ControlsRequest {
    NumberStep {
        text: String,
        #[serde(default = "one")]
        step: f64,
        up: bool,
        min: Option<f64>,
        max: Option<f64>,
    },
    NumberBounds {
        text: String,
        min: Option<f64>,
        max: Option<f64>,
    },
}

fn handle(request: ControlsRequest) -> Result<Value, String> {
    Ok(match request {
        ControlsRequest::NumberStep {
            text,
            step,
            up,
            min,
            max,
        } => {
            if !(step.is_finite() && step > 0.) {
                return Err("step must be a positive number".into());
            }
            let value = number::step_from(&text, step, up, min, max);
            json!({"value": value, "text": number::format_stepped(value, step)})
        }
        ControlsRequest::NumberBounds { text, min, max } => {
            let (at_min, at_max) = number::at_bounds(&text, min, max);
            json!({"at_min": at_min, "at_max": at_max})
        }
    })
}

/// Shipping control behaviour. Free the response with captures_settings_free_v1.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_controls_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<ControlsRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        handle(request)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captures_settings_free_v1;
    use std::ffi::{CStr, CString};

    fn call(request: Value) -> Value {
        let input = CString::new(request.to_string()).unwrap();
        // SAFETY: input is a live NUL-terminated string for this call.
        let output = unsafe { captures_controls_v1(input.as_ptr()) };
        // SAFETY: output is an owned response string freed exactly once below.
        let value =
            serde_json::from_str(unsafe { CStr::from_ptr(output) }.to_str().unwrap()).unwrap();
        // SAFETY: returned by this ABI and not used afterwards.
        unsafe { captures_settings_free_v1(output) };
        value
    }

    #[test]
    fn header_declares_controls_abi() {
        let header = include_str!("../include/captures_settings.h");
        assert!(header.contains("char *captures_controls_v1(const char *request_json);"));
    }

    #[test]
    fn number_steps_and_bounds_cross_the_abi() {
        let up = call(json!({"operation": "number_step", "text": "9", "up": true,
            "min": 0.0, "max": 10.0}));
        assert_eq!(up["result"], json!({"value": 10.0, "text": "10"}));
        let clamped = call(json!({"operation": "number_step", "text": "10", "up": true,
            "min": 0.0, "max": 10.0}));
        assert_eq!(clamped["result"]["value"], 10.0);
        let blank = call(json!({"operation": "number_step", "text": "", "up": true, "min": 2.0}));
        assert_eq!(blank["result"]["text"], "3");
        let bounds = call(json!({"operation": "number_bounds", "text": "0", "min": 0.0}));
        assert_eq!(bounds["result"], json!({"at_min": true, "at_max": false}));
        let bad = call(json!({"operation": "number_step", "text": "1", "up": true, "step": 0.0}));
        assert_eq!(bad["ok"], false);
        assert_eq!(call(json!({"operation": "nope"}))["ok"], false);
    }
}
