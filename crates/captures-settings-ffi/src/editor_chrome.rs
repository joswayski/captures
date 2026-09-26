//! Shipping screenshot-editor chrome for AppKit, from `captures-app::editor_chrome`
//! (shared with the wgpu host): header, tool rail and Layers copy, icon names and
//! the responsive header rules. Requests are rare: one `copy` per editor window,
//! then small queries on resize, viewport changes or tool changes.
use super::region::{response, text};
use captures_app::{editor::Rect, editor_chrome as chrome};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum ChromeRequest {
    Copy,
    HeaderLayout {
        width: f64,
    },
    ShapesTooltip {
        current: String,
    },
    ToolLabel {
        key: String,
    },
    ZoomLabel {
        percent: f64,
    },
    CanvasOffscreen {
        viewport: [f64; 4],
        canvas: [f64; 4],
    },
}

fn tool(item: &chrome::RailTool) -> Value {
    json!({
        "key": item.key,
        "icon": item.icon,
        "label": item.label,
        "shortcut": item.shortcut,
        "name": item.name(),
    })
}

fn rect([x, y, width, height]: [f64; 4]) -> Result<Rect, String> {
    if [x, y, width, height].into_iter().all(f64::is_finite) && width >= 0. && height >= 0. {
        Ok(Rect {
            x,
            y,
            width,
            height,
        })
    } else {
        Err("invalid rect".into())
    }
}

fn copy() -> Value {
    use chrome::{header as h, layers as l};
    json!({
        "metrics": {
            "header_height": chrome::HEADER_HEIGHT,
            "rail_width": chrome::RAIL_WIDTH,
            "rail_button": chrome::RAIL_BUTTON,
            "header_control": chrome::HEADER_CONTROL,
            "zoom_button": chrome::ZOOM_BUTTON,
            "canvas_field": chrome::CANVAS_FIELD,
            "shape_flyout_columns": chrome::SHAPE_FLYOUT_COLUMNS,
            "shape_flyout_button": chrome::SHAPE_FLYOUT_BUTTON,
            "compact_width": chrome::COMPACT_WIDTH,
        },
        "rail": chrome::RAIL_TOOLS.iter().map(tool).collect::<Vec<_>>(),
        "shapes": chrome::SHAPE_TOOLS.iter().map(tool).collect::<Vec<_>>(),
        "header": {
            "canvas": h::CANVAS, "canvas_width": h::CANVAS_WIDTH,
            "canvas_height": h::CANVAS_HEIGHT, "trim": h::TRIM, "trim_tooltip": h::TRIM_TOOLTIP,
            "undo": h::UNDO, "redo": h::REDO, "zoom_group": h::ZOOM_GROUP,
            "fit": h::FIT, "fit_tooltip": h::FIT_TOOLTIP,
            "zoom_out": h::ZOOM_OUT, "zoom_in": h::ZOOM_IN,
            "zoom_slider": h::ZOOM_SLIDER, "zoom_slider_tooltip": h::ZOOM_SLIDER_TOOLTIP,
            "zoom_preset": h::ZOOM_PRESET, "zoom_preset_tooltip": h::ZOOM_PRESET_TOOLTIP,
            "zoom_presets": h::ZOOM_PRESETS,
            "add_images": h::ADD_IMAGES, "recenter": h::RECENTER,
            "draft_menu": h::DRAFT_MENU, "save_draft": h::SAVE_DRAFT,
            "discard_edits": h::DISCARD_EDITS, "draft_restored": h::DRAFT_RESTORED,
            "draft_discard": h::DRAFT_DISCARD, "draft_dismiss": h::DRAFT_DISMISS,
            "draft_dismiss_label": h::DRAFT_DISMISS_LABEL,
        },
        "layers": {
            "title": l::TITLE, "add": l::ADD, "hide": l::HIDE, "show": l::SHOW,
            "lock": l::LOCK, "unlock": l::UNLOCK, "menu": l::MENU, "drag": l::DRAG,
            "locked": l::LOCKED, "rename": l::RENAME,
        },
    })
}

fn handle(request: ChromeRequest) -> Result<Value, String> {
    Ok(match request {
        ChromeRequest::Copy => copy(),
        ChromeRequest::HeaderLayout { width } => {
            if !width.is_finite() {
                return Err("invalid width".into());
            }
            let layout = chrome::header_layout(width);
            json!({
                "show_history": layout.show_history,
                "zoom_slider_width": layout.zoom_slider_width,
                "zoom_preset_width": layout.zoom_preset_width,
            })
        }
        ChromeRequest::ShapesTooltip { current } => json!(chrome::shapes_tooltip(&current)),
        ChromeRequest::ToolLabel { key } => json!(chrome::tool_label(&key)),
        ChromeRequest::ZoomLabel { percent } => {
            if !percent.is_finite() {
                return Err("invalid percent".into());
            }
            json!(chrome::zoom_label(percent))
        }
        ChromeRequest::CanvasOffscreen { viewport, canvas } => {
            json!(chrome::canvas_mostly_offscreen(
                rect(viewport)?,
                rect(canvas)?
            ))
        }
    })
}

/// Screenshot-editor chrome copy and policy. `request_json` is one of
/// `{"operation":"copy"}`, `header_layout {width}`, `shapes_tooltip {current}`,
/// `tool_label {key}`, `zoom_label {percent}` or
/// `canvas_offscreen {viewport:[x,y,w,h], canvas:[x,y,w,h]}`. Returns the owned
/// `{ok,result}` / `{ok,error}` envelope; free with captures_settings_free_v1.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_chrome_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<ChromeRequest>(unsafe { text(request_json) }?)
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
        let output = unsafe { captures_editor_chrome_v1(input.as_ptr()) };
        // SAFETY: output is an owned response string freed exactly once below.
        let value =
            serde_json::from_str(unsafe { CStr::from_ptr(output) }.to_str().unwrap()).unwrap();
        // SAFETY: returned by this ABI and not used afterwards.
        unsafe { captures_settings_free_v1(output) };
        value
    }

    #[test]
    fn header_declares_editor_chrome_abi() {
        let header = include_str!("../include/captures_settings.h");
        assert!(header.contains("char *captures_editor_chrome_v1(const char *request_json);"));
    }

    #[test]
    fn copy_carries_rail_shapes_header_and_layers() {
        let copy = call(json!({"operation": "copy"}));
        assert_eq!(copy["ok"], true);
        let result = &copy["result"];
        assert_eq!(result["rail"][6]["label"], "Eraser");
        assert_eq!(result["rail"][6]["name"], "Eraser (B)");
        assert_eq!(result["rail"][3]["shortcut"], Value::Null);
        assert_eq!(result["shapes"][3]["name"], "Triangle");
        assert_eq!(result["header"]["fit_tooltip"], "Fit canvas to window");
        assert_eq!(result["header"]["zoom_presets"], json!([50., 100., 200.]));
        assert_eq!(result["layers"]["menu"], "Layer settings and actions");
        assert_eq!(result["metrics"]["compact_width"], 1040.);
    }

    #[test]
    fn queries_round_trip_and_reject_invalid_input() {
        let narrow = call(json!({"operation": "header_layout", "width": 900.}));
        assert_eq!(
            narrow["result"],
            json!({"show_history": false, "zoom_slider_width": 72., "zoom_preset_width": 72.})
        );
        assert_eq!(
            call(json!({"operation": "shapes_tooltip", "current": "star"}))["result"],
            "Shapes · Star (S)"
        );
        assert_eq!(
            call(json!({"operation": "tool_label", "key": "wand"}))["result"],
            "Eraser"
        );
        assert_eq!(
            call(json!({"operation": "zoom_label", "percent": 12.25}))["result"],
            "12.3%"
        );
        assert_eq!(
            call(json!({"operation": "canvas_offscreen",
                        "viewport": [0., 0., 400., 300.], "canvas": [500., 0., 100., 100.]}))["result"],
            true
        );
        assert_eq!(
            call(json!({"operation": "canvas_offscreen",
                        "viewport": [0., 0., 400., 300.], "canvas": [0., 0., -1., 1.]}))["ok"],
            false
        );
        assert_eq!(call(json!({"operation": "nope"}))["ok"], false);
        // SAFETY: Null input is reported as an error envelope.
        let null = unsafe { captures_editor_chrome_v1(std::ptr::null()) };
        // SAFETY: owned response, read then freed once.
        let value: Value =
            serde_json::from_str(unsafe { CStr::from_ptr(null) }.to_str().unwrap()).unwrap();
        // SAFETY: returned by this ABI and not used afterwards.
        unsafe { captures_settings_free_v1(null) };
        assert_eq!(value["ok"], false);
    }
}
