//! Pure recording-editor copy and presentation for AppKit.
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use captures_app::recording_editor_ui::{self, EstimateInput};
use captures_media::ExportStage;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Title {
        mime_type: String,
    },
    TrimSummary {
        start_ms: u64,
        end_ms: u64,
        duration_ms: u64,
    },
    Time {
        ms: u64,
        duration_ms: u64,
    },
    FileSize {
        bytes: u64,
    },
    Estimate {
        #[serde(flatten)]
        input: EstimateInput,
    },
    Stage {
        stage: ExportStage,
    },
    Saved {
        gif: bool,
        size_bytes: u64,
    },
    FilenameError {
        stem: String,
    },
    DroppedFrames {
        count: u64,
    },
    Menus {
        gif: bool,
        base_width: u32,
        base_height: u32,
    },
}

fn respond(bytes: &[u8]) -> Result<Value, String> {
    Ok(
        match serde_json::from_slice::<Request>(bytes).map_err(|error| error.to_string())? {
            Request::Title { mime_type } => {
                json!({"title": recording_editor_ui::title(&mime_type)})
            }
            Request::TrimSummary {
                start_ms,
                end_ms,
                duration_ms,
            } => json!(recording_editor_ui::trim_summary(
                start_ms,
                end_ms,
                duration_ms
            )),
            Request::Time { ms, duration_ms } => {
                json!({"label": recording_editor_ui::format_editor_time(ms, duration_ms)})
            }
            Request::FileSize { bytes } => {
                json!({"label": recording_editor_ui::format_file_size(bytes)})
            }
            Request::Estimate { input } => {
                let shown = recording_editor_ui::estimate(&input);
                json!({
                    "label": shown.label,
                    "muted": shown.muted,
                    "delta": shown.delta.as_ref().map(|delta| json!({
                        "percent": delta.percent,
                        "label": delta.label,
                        "smaller": delta.smaller(),
                    })),
                })
            }
            Request::Stage { stage } => {
                json!({"label": recording_editor_ui::export_stage_label(stage)})
            }
            Request::Saved { gif, size_bytes } => {
                json!({"message": recording_editor_ui::saved_message(gif, size_bytes)})
            }
            Request::FilenameError { stem } => {
                json!({"error": recording_editor_ui::filename_error(&stem)})
            }
            Request::DroppedFrames { count } => {
                json!({"warning": recording_editor_ui::dropped_frames_warning(count)})
            }
            Request::Menus {
                gif,
                base_width,
                base_height,
            } => json!(recording_editor_ui::menus(gif, base_width, base_height)),
        },
    )
}

/// UI-thread-safe pure recording editor copy. See the header for operations.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 for this call; free the
/// returned owned JSON exactly once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_editor_ui_v1(
    request_json: *const c_char,
) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return json!({"ok":false,"error":"request pointer is null"});
        }
        // SAFETY: Caller upholds the same contract as the other JSON ABIs.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > 64 * 1024 {
            return json!({"ok":false,"error":"recording editor copy request exceeds 64 KiB"});
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

    fn call(request: Value) -> Value {
        let input = CString::new(request.to_string()).unwrap();
        let pointer = unsafe { captures_recording_editor_ui_v1(input.as_ptr()) };
        let value = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(pointer) };
        value
    }

    #[test]
    fn presents_shared_copy_and_rejects_bad_requests() {
        assert_eq!(
            call(json!({"operation":"title","mime_type":"image/gif"}))["result"]["title"],
            "Edit GIF"
        );
        let summary =
            call(json!({"operation":"trim_summary","start_ms":0,"end_ms":3000,"duration_ms":3000}));
        assert_eq!(summary["result"]["range"], "0:00.000 – 0:03.000");
        assert_eq!(summary["result"]["selected"], "0:03.000 selected");
        let estimate = call(json!({"operation":"estimate","estimate_bytes":7,
            "estimate_exact":true,"original_bytes":8}));
        assert_eq!(estimate["result"]["label"], "7 B");
        assert_eq!(estimate["result"]["delta"]["label"], "−12%");
        assert_eq!(estimate["result"]["delta"]["smaller"], true);
        let pending = call(json!({"operation":"estimate","estimating":true}));
        assert_eq!(pending["result"]["label"], "Estimating…");
        assert!(pending["result"]["delta"].is_null());
        assert_eq!(
            call(json!({"operation":"stage","stage":"verifying"}))["result"]["label"],
            "Checking file size…"
        );
        assert_eq!(
            call(json!({"operation":"saved","gif":true,"size_bytes":1500}))["result"]["message"],
            "GIF saved — 1.5 KB."
        );
        assert!(
            call(json!({"operation":"filename_error","stem":"clip"}))["result"]["error"].is_null()
        );
        assert_eq!(
            call(json!({"operation":"dropped_frames","count":2}))["result"]["warning"],
            "This source dropped 2 frames during capture. The original timing is preserved."
        );
        assert!(
            call(json!({"operation":"dropped_frames","count":0}))["result"]["warning"].is_null()
        );
        let menus =
            call(json!({"operation":"menus","gif":false,"base_width":640,"base_height":360}));
        assert_eq!(
            menus["result"]["resolutions"][0]["label"],
            "Original — 640 × 360"
        );
        assert_eq!(menus["result"]["quality_presets"][2]["label"], "Balanced");
        assert_eq!(call(json!({"operation":"unknown"}))["ok"], false);
        let pointer = unsafe { captures_recording_editor_ui_v1(std::ptr::null()) };
        let value: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(pointer) };
        assert_eq!(value["ok"], false);
    }
}
