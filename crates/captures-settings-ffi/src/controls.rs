//! Shipping `NumberInput` stepping and `CustomSelect` listbox keys and
//! placement for AppKit, from `captures-app::controls` (shared with the wgpu
//! host). Called once per stepper click, key or listbox opening.
use super::region::{response, text};
use captures_app::controls::{number, select};
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
    /// A key on a focused select trigger, open or closed.
    SelectKey {
        open: bool,
        active: usize,
        selected: usize,
        disabled: Vec<bool>,
        key: String,
    },
    /// Where an opening listbox goes, in window points (y down).
    SelectLayout {
        trigger: [f64; 4],
        menu_width: f64,
        menu_height: f64,
        viewport_width: f64,
        viewport_height: f64,
        option_count: usize,
    },
}

fn select_key(name: &str) -> Result<select::Key, String> {
    Ok(match name {
        "arrow_down" => select::Key::ArrowDown,
        "arrow_up" => select::Key::ArrowUp,
        "home" => select::Key::Home,
        "end" => select::Key::End,
        "enter" => select::Key::Enter,
        "space" => select::Key::Space,
        "escape" => select::Key::Escape,
        other => return Err(format!("unknown select key {other}")),
    })
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
        ControlsRequest::SelectKey {
            open,
            active,
            selected,
            disabled,
            key,
        } => {
            let outcome = select::key(
                select::State { open, active },
                &disabled,
                selected,
                select_key(&key)?,
            );
            json!({
                "open": outcome.state.open,
                "active": outcome.state.active,
                "chosen": outcome.chosen,
                "handled": outcome.handled,
            })
        }
        ControlsRequest::SelectLayout {
            trigger,
            menu_width,
            menu_height,
            viewport_width,
            viewport_height,
            option_count,
        } => {
            let [left, top, width, height] = trigger;
            let layout = select::layout(
                select::Rect {
                    left,
                    top,
                    width,
                    height,
                },
                menu_width,
                menu_height,
                viewport_width,
                viewport_height,
                option_count,
            );
            serde_json::to_value(layout).map_err(|error| error.to_string())?
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

    #[test]
    fn select_keys_and_layout_cross_the_abi() {
        let disabled = json!([false, true, false, false]);
        let opened = call(json!({"operation": "select_key", "open": false, "active": 0,
            "selected": 2, "disabled": disabled, "key": "arrow_down"}));
        assert_eq!(opened["result"], json!({"open": true, "active": 2, "chosen": null, "handled": true}));
        let end = call(json!({"operation": "select_key", "open": true, "active": 0,
            "selected": 0, "disabled": disabled, "key": "end"}));
        assert_eq!(end["result"]["active"], 3);
        let home = call(json!({"operation": "select_key", "open": true, "active": 3,
            "selected": 0, "disabled": [true, false, false], "key": "home"}));
        assert_eq!(home["result"]["active"], 1, "Home skips disabled options");
        let closed_home = call(json!({"operation": "select_key", "open": false, "active": 0,
            "selected": 0, "disabled": disabled, "key": "home"}));
        assert_eq!(closed_home["result"]["handled"], false, "Home/End only move an open listbox");
        let chosen = call(json!({"operation": "select_key", "open": true, "active": 2,
            "selected": 0, "disabled": disabled, "key": "enter"}));
        assert_eq!(chosen["result"], json!({"open": false, "active": 2, "chosen": 2, "handled": true}));
        let bad = call(json!({"operation": "select_key", "open": true, "active": 0,
            "selected": 0, "disabled": disabled, "key": "tab"}));
        assert_eq!(bad["ok"], false);
        let layout = call(json!({"operation": "select_layout", "trigger": [100.0, 700.0, 120.0, 32.0],
            "menu_width": 200.0, "menu_height": 150.0, "viewport_width": 800.0,
            "viewport_height": 800.0, "option_count": 4}));
        let result = &layout["result"];
        assert_eq!(result["above"], true, "no room below: the listbox opens above");
        assert_eq!(result["width"], 200.0);
        assert_eq!(result["left"], 20.0, "right-aligned to the trigger");
        assert_eq!(result["top"], 544.0);
    }
}
