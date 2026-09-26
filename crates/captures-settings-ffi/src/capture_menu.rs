//! Shipping New Capture menu copy and presentation policy for AppKit. Requests
//! are rare (menu open, mode or option changes); per-pointer guidance ducking
//! uses the allocation-free `captures_capture_guidance_pointer_over_v1`.
use super::region::{response, text};
use captures_app::capture_menu::{
    self, GuidanceTarget, MenuMode, PreferenceTarget, PrimaryState, RecordingToggle,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Default, Deserialize)]
#[serde(default)]
struct Pointer {
    show_cursor: bool,
    highlight_clicks: bool,
    system_audio: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Available {
    cursor_control: bool,
    click_highlights: bool,
    system_audio: bool,
}

#[derive(Deserialize)]
struct Device {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum MenuRequest {
    Copy,
    Menu {
        mode: MenuMode,
        auto_start: bool,
        can_exclude_controls: bool,
        controls_excluded: bool,
        #[serde(default)]
        state: PrimaryState,
        #[serde(default)]
        options: Pointer,
        #[serde(default)]
        available: Available,
    },
    Toggle {
        changed: String,
        show_cursor: bool,
        highlight_clicks: bool,
    },
    Microphones {
        available: bool,
        loading: bool,
        selected: Option<String>,
        devices: Vec<Device>,
    },
    DisplayIdentity {
        name: String,
        width: u32,
        height: u32,
        recording_fps: Option<u16>,
    },
}

fn target(target: Option<PreferenceTarget>) -> Value {
    target.map_or(
        Value::Null,
        |target| json!({"id": target.id(), "setting": target.setting_key()}),
    )
}

fn toggle_key(toggle: RecordingToggle) -> &'static str {
    match toggle {
        RecordingToggle::ShowCursor => "show_cursor",
        RecordingToggle::ShowClicks => "highlight_clicks",
        RecordingToggle::DesktopAudio => "system_audio",
    }
}

fn copy() -> Value {
    let guidance = |target, feedback| {
        let copy = capture_menu::guidance(target, feedback);
        json!({"title": copy.title, "hint": copy.hint})
    };
    json!({
        "fields": {
            "fps": capture_menu::FIELD_FPS,
            "fps_accessibility_label": capture_menu::FPS_ACCESSIBILITY_LABEL,
            "max_resolution": capture_menu::FIELD_MAX_RESOLUTION,
            "max_resolution_accessibility_label":
                capture_menu::MAX_RESOLUTION_ACCESSIBILITY_LABEL,
            "microphone": capture_menu::FIELD_MICROPHONE,
        },
        "fps_options": capture_menu::FPS_OPTIONS,
        "resolution_options": capture_menu::RESOLUTION_OPTIONS
            .iter()
            .map(|(value, label)| json!({"value": value, "label": label}))
            .collect::<Vec<_>>(),
        "toggles": RecordingToggle::ALL
            .iter()
            .map(|toggle| json!({
                "key": toggle_key(*toggle),
                "label": toggle.label(),
                "accessibility_label": toggle.accessibility_label(),
                "unavailable_reason": toggle.unavailable_reason(),
            }))
            .collect::<Vec<_>>(),
        "guidance": {
            "region": guidance(GuidanceTarget::Region, false),
            "region_feedback": guidance(GuidanceTarget::Region, true),
            "window": guidance(GuidanceTarget::Window, false),
            "display": guidance(GuidanceTarget::Display, false),
        },
        "separator": capture_menu::NOTE_SEPARATOR,
        "confirm": capture_menu::CONFIRM_NOTE,
        "auto_start": capture_menu::AUTO_START_NOTE,
        "microphone_loading": capture_menu::MICROPHONES_LOADING,
        "selected_microphone": capture_menu::SELECTED_MICROPHONE,
        "highlight_ms": capture_menu::PREFERENCE_HIGHLIGHT.as_millis() as u64,
    })
}

fn handle(request: MenuRequest) -> Result<Value, String> {
    Ok(match request {
        MenuRequest::Copy => copy(),
        MenuRequest::Menu {
            mode,
            auto_start,
            can_exclude_controls,
            controls_excluded,
            state,
            options,
            available,
        } => {
            let note = capture_menu::visibility_note(mode, can_exclude_controls, controls_excluded);
            let confirm = capture_menu::confirm_note(auto_start);
            let primary = capture_menu::primary_action(mode, auto_start, state);
            let toggles = [
                (options.show_cursor, available.cursor_control),
                (options.highlight_clicks, available.click_highlights),
                (options.system_audio, available.system_audio),
            ]
            .into_iter()
            .zip(RecordingToggle::ALL)
            .map(|((on, available), toggle)| {
                json!({
                    "key": toggle_key(toggle),
                    "on": available && on,
                    "available": available,
                    "status": capture_menu::toggle_status(available, on),
                })
            })
            .collect::<Vec<_>>();
            json!({
                "note": {
                    "lead": note.lead,
                    "emphasis": note.emphasis,
                    "trail": note.trail,
                    "hint": note.hint,
                    "text": note.text(),
                    "target": target(note.target),
                },
                "confirm": {"text": confirm.text, "target": target(confirm.target)},
                "primary": primary,
                "toggles": toggles,
            })
        }
        MenuRequest::Toggle {
            changed,
            show_cursor,
            highlight_clicks,
        } => {
            let changed = RecordingToggle::ALL
                .into_iter()
                .find(|toggle| toggle_key(*toggle) == changed)
                .ok_or("unknown recording toggle")?;
            let (show_cursor, highlight_clicks) =
                capture_menu::couple_pointer_options(changed, show_cursor, highlight_clicks);
            json!({"show_cursor": show_cursor, "highlight_clicks": highlight_clicks})
        }
        MenuRequest::Microphones {
            available,
            loading,
            selected,
            devices,
        } => {
            let devices = devices
                .iter()
                .map(|device| (device.id.as_str(), device.name.as_str()))
                .collect::<Vec<_>>();
            json!({
                "entries": capture_menu::microphone_entries(
                    available, loading, selected.as_deref(), &devices),
                "selected_label": capture_menu::microphone_selected_label(
                    available, loading, selected.as_deref(), &devices),
            })
        }
        MenuRequest::DisplayIdentity {
            name,
            width,
            height,
            recording_fps,
        } => json!(capture_menu::display_identity(
            &name,
            width,
            height,
            recording_fps
        )),
    })
}

/// Capture-menu copy and policy. Free the response with captures_settings_free_v1.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_capture_menu_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<MenuRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        handle(request)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    })
}

/// Shipping guidance-chip pointer ducking with enter/leave hysteresis. Chip
/// bounds and pointer share one top-left logical space. No allocation.
#[unsafe(no_mangle)]
pub extern "C" fn captures_capture_guidance_pointer_over_v1(
    x: f64,
    y: f64,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    currently_over: bool,
) -> bool {
    capture_menu::pointer_over_guidance(x, y, left, top, right, bottom, currently_over)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captures_settings_free_v1;
    use std::ffi::{CStr, CString};

    fn call(request: Value) -> Value {
        let input = CString::new(request.to_string()).unwrap();
        // SAFETY: input is a live NUL-terminated string for this call.
        let output = unsafe { captures_capture_menu_v1(input.as_ptr()) };
        // SAFETY: output is an owned response string freed exactly once below.
        let value =
            serde_json::from_str(unsafe { CStr::from_ptr(output) }.to_str().unwrap()).unwrap();
        // SAFETY: returned by this ABI and not used afterwards.
        unsafe { captures_settings_free_v1(output) };
        value
    }

    #[test]
    fn header_declares_capture_menu_abi() {
        let header = include_str!("../include/captures_settings.h");
        assert!(header.contains("char *captures_capture_menu_v1(const char *request_json);"));
        assert!(header.contains("bool captures_capture_guidance_pointer_over_v1("));
    }

    #[test]
    fn menu_reports_note_links_primary_and_toggles() {
        let value = call(json!({
            "operation": "menu", "mode": "recording", "auto_start": true,
            "can_exclude_controls": true, "controls_excluded": false,
            "state": {"error": true},
            "options": {"show_cursor": true, "highlight_clicks": true},
            "available": {"cursor_control": true, "click_highlights": false, "system_audio": true},
        }));
        let result = &value["result"];
        assert_eq!(
            result["note"]["text"],
            "These controls will show in recordings"
        );
        assert_eq!(
            result["note"]["target"]["setting"],
            "include_recording_controls_in_captures"
        );
        assert_eq!(result["confirm"]["target"]["id"], "auto-start-on-selection");
        assert_eq!(result["primary"]["label"], "Retry recording");
        assert_eq!(result["primary"]["hidden"], false);
        assert_eq!(result["toggles"][0]["status"], "On");
        assert_eq!(result["toggles"][1]["status"], "Unavailable");
        assert_eq!(result["toggles"][1]["on"], false);
        assert_eq!(result["toggles"][2]["status"], "Off");
    }

    #[test]
    fn copy_toggle_microphones_and_identity_round_trip() {
        let copy = call(json!({"operation": "copy"}));
        assert_eq!(copy["result"]["fps_options"], json!([60, 30, 15]));
        assert_eq!(
            copy["result"]["guidance"]["display"]["title"],
            "Click to capture this display"
        );
        assert_eq!(copy["result"]["toggles"][1]["key"], "highlight_clicks");
        let toggle = call(json!({"operation": "toggle", "changed": "highlight_clicks",
            "show_cursor": false, "highlight_clicks": true}));
        assert_eq!(toggle["result"]["show_cursor"], true);
        let microphones = call(json!({"operation": "microphones", "available": true,
            "loading": false, "selected": "gone", "devices": [{"id": "a", "name": "Mic"}]}));
        assert_eq!(
            microphones["result"]["selected_label"],
            "Selected microphone"
        );
        assert_eq!(microphones["result"]["entries"][2]["label"], "Mic");
        let identity = call(json!({"operation": "display_identity", "name": "Built-in",
            "width": 1512, "height": 982, "recording_fps": 30}));
        assert_eq!(identity["result"]["detail"], "1512 × 982 · 30 FPS");
        assert_eq!(call(json!({"operation": "nope"}))["ok"], false);
    }

    #[test]
    fn pointer_over_uses_shipping_pad_and_slack() {
        assert!(captures_capture_guidance_pointer_over_v1(
            72., 50., 100., 40., 300., 90., false
        ));
        assert!(!captures_capture_guidance_pointer_over_v1(
            71., 50., 100., 40., 300., 90., false
        ));
        assert!(captures_capture_guidance_pointer_over_v1(
            61., 50., 100., 40., 300., 90., true
        ));
    }
}
