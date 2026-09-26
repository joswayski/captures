//! Screenshot editor export bar: shared save target/copy/estimate policy.

use super::region::{response, text};
use captures_app::{
    editor_export::{
        self, EstimateState, ExportSource, ExportTarget, SavePlan, estimate_export, present,
    },
    editor_session::{EditorSession, ExportOptions},
};
use image::RgbaImage;
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetInit {
    source: Option<ExportSource>,
    default_directory: PathBuf,
    default_stem: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TargetAction {
    SetStem { stem: String },
    SetDirectory { directory: PathBuf },
    SetSaveAsNew { enabled: bool },
    Adopt { source: ExportSource },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportBarRequest {
    target: Option<ExportTarget>,
    init: Option<TargetInit>,
    action: Option<TargetAction>,
    options: ExportOptions,
    document_size: (u32, u32),
    #[serde(default)]
    transparent_background: bool,
    #[serde(default)]
    estimate: EstimateState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveRequest {
    history_root: PathBuf,
    plan: SavePlan,
    options: ExportOptions,
    mode: captures_capture::CaptureMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EstimateRequest {
    options: ExportOptions,
    original_bytes: Option<u64>,
}

/// Apply one export-bar action and present the bar. Pure: no I/O besides
/// checking whether a saved source still exists when a target is created.
///
/// # Safety
/// Input is null or readable NUL-terminated UTF-8 for the call. Free the owned
/// JSON response with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_export_bar_v1(request_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input for this call.
        let request = serde_json::from_str::<ExportBarRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let mut target = match (request.target, request.init) {
            (Some(target), None) => target,
            (None, Some(init)) => {
                ExportTarget::new(init.source, &init.default_directory, &init.default_stem)
            }
            _ => return Err("provide exactly one of target or init".to_owned()),
        };
        match request.action {
            Some(TargetAction::SetStem { stem }) => target.set_stem(stem),
            Some(TargetAction::SetDirectory { directory }) => target.set_directory(directory),
            Some(TargetAction::SetSaveAsNew { enabled }) => {
                target.set_save_as_new(enabled, request.options.format);
            }
            Some(TargetAction::Adopt { source }) => target.adopt(source),
            None => {}
        }
        let view = present(
            &target,
            request.options,
            request.document_size,
            request.transparent_background,
            request.estimate,
        );
        Ok::<_, String>(json!({"target": target, "view": view}))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Save the edited frame exactly as an export-bar plan says: overwrite the
/// planned source (History and path revalidated from disk) or publish a new
/// file that never replaces anything. Does not mutate session or draft state.
///
/// # Safety
/// Non-null session is live and not accessed/freed concurrently. Input is
/// readable NUL-terminated UTF-8. Free owned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_save_v1(
    session: *const EditorSession,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and a live serialized session.
        let request = serde_json::from_str::<SaveRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_ref() }.ok_or("editor handle is null")?;
        if session.snapshot().active_text_input.is_some() {
            return Err("Finish or cancel text input before saving.".to_owned());
        }
        let saved = editor_export::publish(
            &request.history_root,
            &session.pixels(),
            &request.plan,
            request.options,
            request.mode,
        )?;
        let notice = editor_export::saved_notice(&request.plan, &saved);
        let mut value = serde_json::to_value(saved).map_err(|error| error.to_string())?;
        value["notice"] = json!(notice);
        Ok(value)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(saved) => json!({"ok":true,"result":saved}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Estimate the saved size of a retained frame, off the session worker.
///
/// # Safety
/// Non-null frame is a live handle from captures_editor_frame_v1 retained for
/// the call; it may be used from any thread. Input is readable UTF-8. Free the
/// owned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_estimate_v1(
    frame: *const Arc<RgbaImage>,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and a live immutable frame.
        let request = serde_json::from_str::<EstimateRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let frame = unsafe { frame.as_ref() }.ok_or("editor frame is null")?;
        estimate_export(frame, request.options, request.original_bytes)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(estimate) => json!({"ok":true,"result":estimate}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::editor_session::OpenRequest;
    use std::{
        ffi::{CStr, CString},
        ptr,
    };

    unsafe fn take_json(value: *mut c_char) -> serde_json::Value {
        // SAFETY: tests pass only live Rust-owned response strings.
        let json = serde_json::from_slice(unsafe { CStr::from_ptr(value) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(value) };
        json
    }

    fn bar(request: &serde_json::Value) -> serde_json::Value {
        let input = CString::new(request.to_string()).unwrap();
        // SAFETY: live C string; the owned response is freed by take_json.
        unsafe { take_json(captures_editor_export_bar_v1(input.as_ptr())) }
    }

    const PNG: &str = r#"{"format":"png","quality":"preserve","quality_value":100,"png":{}}"#;

    #[test]
    fn export_bar_round_trips_target_actions_and_presents_shared_copy() {
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("Shot.png");
        std::fs::write(&source, b"png").unwrap();
        let options: serde_json::Value = serde_json::from_str(PNG).unwrap();
        let first = bar(&json!({
            "init": {"source": {"artifact_id": "shot", "path": source},
                     "default_directory": "/exports", "default_stem": "unused"},
            "options": options, "document_size": [1920, 1080],
            "estimate": {"bytes": 240_000, "baseline_bytes": 300_000},
        }));
        assert_eq!(first["ok"], true, "{first}");
        let view = &first["result"]["view"];
        assert_eq!(view["summary"], "PNG · 1920 × 1080 · ≈ 240 KB");
        assert_eq!(view["suffix"], ".png");
        assert_eq!(view["delta"]["label"], "−20%");
        assert_eq!(view["plan"]["kind"], "overwrite");
        assert_eq!(view["plan"]["artifact_id"], "shot");
        assert_eq!(view["saving_copy"], false);
        assert_eq!(
            view["hint"],
            "Save keeps original quality as PNG and overwrites the original."
        );

        let renamed = bar(&json!({
            "target": first["result"]["target"], "action": {"kind": "set_stem", "stem": "Other"},
            "options": options, "document_size": [1920, 1080],
        }));
        assert_eq!(renamed["result"]["target"]["save_as_new"], true);
        assert_eq!(renamed["result"]["view"]["plan"]["kind"], "new_file");
        assert_eq!(
            renamed["result"]["view"]["summary"],
            "PNG · 1920 × 1080 · —"
        );
        let restored = bar(&json!({
            "target": renamed["result"]["target"],
            "action": {"kind": "set_stem", "stem": "Shot"},
            "options": options, "document_size": [1920, 1080],
        }));
        let restored = bar(&json!({
            "target": restored["result"]["target"],
            "action": {"kind": "set_save_as_new", "enabled": false},
            "options": options, "document_size": [1920, 1080],
        }));
        assert_eq!(restored["result"]["view"]["plan"]["kind"], "overwrite");
        let adopted = bar(&json!({
            "target": restored["result"]["target"],
            "action": {"kind": "adopt", "source": {"artifact_id": "next", "path": source}},
            "options": options, "document_size": [10, 10],
        }));
        assert_eq!(adopted["result"]["view"]["plan"]["artifact_id"], "next");

        for invalid in [
            json!({"options": options, "document_size": [1, 1]}),
            json!({"init": {"source": null, "default_directory": "/x", "default_stem": "a"},
                   "action": {"kind": "replace_everything"}, "options": options,
                   "document_size": [1, 1]}),
        ] {
            assert_eq!(bar(&invalid)["ok"], false);
        }
        // SAFETY: a null request returns an owned error response.
        assert_eq!(
            unsafe { take_json(captures_editor_export_bar_v1(ptr::null())) }["ok"],
            false
        );
    }

    #[test]
    fn save_and_estimate_use_the_session_frame_without_changing_it() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("history");
        let capture = captures_app::persist_screenshot(
            &root,
            &RgbaImage::from_fn(7, 3, |x, y| {
                image::Rgba([x as u8 * 31, y as u8 * 71, 9, 255])
            }),
            captures_capture::CaptureMode::Window,
        )
        .unwrap();
        let session = EditorSession::open(OpenRequest {
            history_root: root.clone(),
            drafts_root: data.path().join("drafts"),
            artifact_id: capture.entry.id,
        })
        .unwrap();
        let before = json!(session.snapshot());
        let destination = data.path().join("exports/edited.png");
        let options: serde_json::Value = serde_json::from_str(PNG).unwrap();
        let request = CString::new(
            json!({"history_root": root, "plan": {"kind": "new_file", "path": destination},
                   "options": options, "mode": "window"})
            .to_string(),
        )
        .unwrap();
        let frame = Box::into_raw(Box::new(session.pixels()));
        let estimate =
            CString::new(json!({"options": options, "original_bytes": 55}).to_string()).unwrap();
        // SAFETY: session, frame and strings stay live; responses and frame are freed once.
        unsafe {
            assert_eq!(
                take_json(captures_editor_save_v1(ptr::null(), request.as_ptr()))["ok"],
                false
            );
            let saved = take_json(captures_editor_save_v1(&session, request.as_ptr()));
            assert_eq!(saved["result"]["status"], "saved", "{saved}");
            assert_eq!(saved["result"]["artifact"]["entry"]["mode"], "window");
            assert_eq!(
                saved["result"]["notice"],
                format!("Saved {}", destination.display())
            );
            let collision = take_json(captures_editor_save_v1(&session, request.as_ptr()));
            assert_eq!(
                collision["error"],
                "edited.png already exists. Choose another filename."
            );
            let estimated = take_json(captures_editor_estimate_v1(frame, estimate.as_ptr()));
            assert_eq!(
                estimated["result"]["bytes"],
                std::fs::metadata(&destination).unwrap().len()
            );
            assert_eq!(estimated["result"]["baseline_bytes"], 55);
            assert_eq!(
                take_json(captures_editor_estimate_v1(ptr::null(), estimate.as_ptr()))["ok"],
                false
            );
            drop(Box::from_raw(frame));
        }
        assert_eq!(json!(session.snapshot()), before);
    }
}
