mod capture_menu;
mod controls;
mod editor;
mod editor_chrome;
mod editor_export;
mod feedback;
mod icons;
mod instance;
mod preferences;
mod preview;
mod recording;
mod recording_editor;
mod recording_editor_ui;
mod recording_geometry;
mod recording_hud;
mod recording_recovery;
mod recording_timeline;
mod region;
mod selection;
mod shortcuts;
mod tray_notice;
mod update_notice;
mod window;

use captures_settings::AppSettings;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    ffi::{CStr, CString},
    os::raw::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::Mutex,
    time::Instant,
};

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
static ONBOARDING: Mutex<captures_app::onboarding::Session> =
    Mutex::new(captures_app::onboarding::Session::new());

thread_local! {
    // Native key registration and destruction must stay on the event-loop thread.
    static CAPTURE_FLOW: RefCell<Option<captures_app::capture_flow::CaptureFlow>> = const { RefCell::new(None) };
    static RECORDING_SCREENSHOT_FLOW: RefCell<Option<captures_app::capture_flow::CaptureFlow>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum FlowRequest {
    Begin { seconds: u8 },
    BeginRecordingScreenshot { parent_generation: u64, seconds: u8 },
    StartCountdown { generation: u64, seconds: u8 },
    RestartCountdown { generation: u64, seconds: u8 },
    Poll { generation: u64 },
    DisarmEscape { generation: u64 },
    Finish { generation: u64 },
}

fn flow_response(request: FlowRequest) -> Result<Value, String> {
    CAPTURE_FLOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        RECORDING_SCREENSHOT_FLOW.with(|child_slot| {
            let mut child_slot = child_slot.borrow_mut();
            match request {
                FlowRequest::Begin { seconds } => {
                    if slot.is_some() || child_slot.is_some() {
                        return Err("A capture is already in progress".into());
                    }
                    let flow = captures_app::capture_flow::CaptureFlow::begin(seconds)?;
                    let generation = flow.generation();
                    *slot = Some(flow);
                    Ok(json!({"generation":generation}))
                }
                FlowRequest::BeginRecordingScreenshot {
                    parent_generation,
                    seconds,
                } => {
                    if child_slot.is_some() {
                        return Err("A recording screenshot is already in progress".into());
                    }
                    let parent = slot
                        .as_ref()
                        .filter(|flow| {
                            flow.generation() == parent_generation && flow.is_current()
                        })
                        .ok_or("The recording is no longer active")?;
                    let flow = parent.begin_recording_screenshot(seconds)?;
                    let generation = flow.generation();
                    *child_slot = Some(flow);
                    Ok(json!({"generation":generation}))
                }
                FlowRequest::StartCountdown {
                    generation,
                    seconds,
                } => {
                    let flow = child_slot
                        .as_mut()
                        .filter(|flow| flow.generation() == generation)
                        .or_else(|| {
                            slot.as_mut()
                                .filter(|flow| flow.generation() == generation)
                        })
                        .ok_or("Capture is no longer active")?;
                    flow.start_countdown(seconds)?;
                    Ok(json!({}))
                }
                FlowRequest::RestartCountdown {
                    generation,
                    seconds,
                } => {
                    let flow = slot
                        .as_mut()
                        .filter(|flow| flow.generation() == generation)
                        .ok_or("Capture is no longer active")?;
                    flow.restart_countdown(seconds)?;
                    Ok(json!({}))
                }
                FlowRequest::Poll { generation } => {
                    let flow = child_slot
                        .as_ref()
                        .filter(|flow| flow.generation() == generation)
                        .or_else(|| {
                            slot.as_ref()
                                .filter(|flow| flow.generation() == generation)
                        })
                        .ok_or("Capture is no longer active")?;
                    Ok(json!({"current":flow.is_current(),"remaining":flow.countdown().remaining(Instant::now())}))
                }
                FlowRequest::DisarmEscape { generation } => {
                    let flow = slot
                        .as_mut()
                        .filter(|flow| flow.generation() == generation)
                        .ok_or("Capture is no longer active")?;
                    flow.disarm_escape()?;
                    Ok(json!({}))
                }
                FlowRequest::Finish { generation } => {
                    if child_slot
                        .as_ref()
                        .is_some_and(|flow| flow.generation() == generation)
                    {
                        *child_slot = None;
                    }
                    if slot
                        .as_ref()
                        .is_some_and(|flow| flow.generation() == generation)
                    {
                        *slot = None;
                    }
                    Ok(json!({}))
                }
            }
        })
    })
}

/// Main/event-loop-thread-only capture guard ABI. Never call from a worker.
/// Begin temporarily registers Escape; finish must release it, including at quit.
///
/// # Safety
/// `request_json` is a readable NUL-terminated UTF-8 string during this call.
/// Release the result exactly once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_flow_request_v1(request_json: *const c_char) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return json!({"ok":false,"error":"request pointer is null"});
        }
        // SAFETY: This entry point uses the same pointer ownership as app requests.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > MAX_REQUEST_BYTES {
            return json!({"ok":false,"error":"request exceeds 8 MiB"});
        }
        let result = serde_json::from_slice::<FlowRequest>(bytes)
            .map_err(|e| e.to_string())
            .and_then(flow_response);
        match result {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false,"error":"internal panic"}));
    CString::new(value.to_string())
        .expect("JSON contains no NUL bytes")
        .into_raw()
}

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
    LoginItem {
        history_root: String,
        settings_file: String,
        enabled: Option<bool>,
    },
    Onboarding {
        path: String,
        action: captures_app::onboarding::Action,
    },
    OnboardingCopy,
    OnboardingPresentation {
        state: Box<captures_app::onboarding::State>,
    },
    HistoryCopy,
    HistoryCards {
        cards: Vec<Value>,
    },
    HistoryGrid {
        width: f64,
        /// The window is at or below the shipping 720 px breakpoint.
        #[serde(default)]
        compact: bool,
    },
    /// Shipping keyframes and transitions (`captures_app::motion`).
    Motion,
    /// Shipping `report_capture_error` dialog copy for a failed capture.
    CaptureErrorCopy,
    /// Where Screenshot Display goes for the recording's state, if any
    /// (`captures_app::capture_error::display_route`). Region and window
    /// follow the same states (`screenshot_route`): "capture_display" means a
    /// screenshot beside the take for them too.
    DisplayCaptureRoute {
        recording: Option<captures_recording::RecordingState>,
    },
    /// Where New Capture goes (`captures_app::capture_error::new_capture_route`).
    NewCaptureRoute {
        recording: Option<captures_recording::RecordingState>,
        #[serde(default)]
        controls_hidden: bool,
    },
    /// Shipping Screen Recording recovery dialog for a denied capture.
    PermissionRecoveryPrompt,
    PermissionRecoveryClassify {
        message: String,
    },
    PermissionRecoverySchedule {
        path: String,
        mode: captures_capture::CaptureMode,
    },
    PermissionRecoveryTake {
        path: String,
    },
    PermissionRecoveryReset {
        path: String,
        bundle_id: String,
    },
}

/// One History entry plus the host's off-main `missing` result. Unknown fields
/// (paths added by the history operations) are ignored.
#[derive(Deserialize)]
struct HistoryCardInput {
    entry: captures_history::HistoryEntry,
    #[serde(default)]
    missing: bool,
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
        Ok(Request::Onboarding { path, action }) => ONBOARDING
            .lock()
            .map_err(|_| "The onboarding service is unavailable. Restart Captures.".to_owned())
            .and_then(|mut session| session.execute(Path::new(&path), action))
            .map(|state| {
                // Hosts render the shared presentation instead of re-deriving copy.
                let mut value = json!(state);
                value["presentation"] = json!(state.presentation());
                json!({"ok":true,"state":value})
            })
            .unwrap_or_else(|error| json!({"ok":false,"error":error})),
        Ok(Request::OnboardingPresentation { state }) => {
            json!({"ok":true,"presentation":state.presentation()})
        }
        Ok(Request::HistoryCopy) => {
            use captures_app::history_view::CardAction;
            let actions = CardAction::ALL
                .into_iter()
                .map(|action| {
                    (
                        json!(action).as_str().unwrap_or_default().to_owned(),
                        json!({
                            "label":action.label(),
                            "busy":action.busy_label(),
                            "done":action.done_label(),
                            "tooltip":action.tooltip(),
                            "icon":action.icon(),
                        }),
                    )
                })
                .collect::<serde_json::Map<_, _>>();
            json!({
                "ok":true,
                "copy":captures_app::history_view::copy(),
                "actions":actions,
                "confirm_timeout_ms":captures_app::history_view::CONFIRM_TIMEOUT_MS,
                "feedback_ms":captures_app::history_view::ACTION_FEEDBACK_MS,
            })
        }
        // A malformed entry yields null for that card only.
        Ok(Request::HistoryCards { cards }) => json!({
            "ok":true,
            "cards":cards
                .into_iter()
                .map(|input| {
                    serde_json::from_value::<HistoryCardInput>(input)
                        .ok()
                        .map(|input| captures_app::history_view::card(&input.entry, input.missing))
                })
                .collect::<Vec<_>>(),
        }),
        Ok(Request::HistoryGrid { width, compact }) => {
            json!({"ok":true,"grid":captures_app::history_view::grid_in_window(width, compact)})
        }
        Ok(Request::Motion) => {
            json!({"ok":true,"motion":captures_app::motion::catalog()})
        }
        Ok(Request::CaptureErrorCopy) => json!({
            "ok":true,
            "copy":{
                "title":captures_app::capture_error::TITLE,
                "button":captures_app::capture_error::OK,
            },
        }),
        Ok(Request::DisplayCaptureRoute { recording }) => {
            use captures_app::capture_error::{DisplayRoute, display_route};
            let route = match display_route(recording) {
                DisplayRoute::CaptureMenu => "capture_menu",
                DisplayRoute::CaptureDisplay => "capture_display",
                DisplayRoute::Ignore => "ignore",
            };
            json!({"ok":true,"route":route})
        }
        Ok(Request::NewCaptureRoute {
            recording,
            controls_hidden,
        }) => {
            use captures_app::capture_error::{
                CAPTURE_IN_PROGRESS, NewCaptureRoute, message, new_capture_route,
            };
            match new_capture_route(recording, controls_hidden) {
                NewCaptureRoute::CaptureMenu => json!({"ok":true,"route":"capture_menu"}),
                NewCaptureRoute::RestoreControls => {
                    json!({"ok":true,"route":"restore_controls"})
                }
                NewCaptureRoute::InProgress => json!({
                    "ok":true,"route":"in_progress","message":message(CAPTURE_IN_PROGRESS),
                }),
            }
        }
        Ok(Request::PermissionRecoveryPrompt) => ONBOARDING
            .lock()
            .map_err(|_| "The onboarding service is unavailable. Restart Captures.".to_owned())
            .map(|session| {
                use captures_app::permission_recovery as recovery;
                json!({
                    "ok":true,
                    "supported":recovery::supported(),
                    "prompt":recovery::prompt(session.screen_requested_this_launch()),
                })
            })
            .unwrap_or_else(|error| json!({"ok":false,"error":error})),
        Ok(Request::PermissionRecoveryClassify { message }) => json!({
            "ok":true,
            "denied":captures_app::permission_recovery::is_permission_denied(&message),
            "failure":captures_app::permission_recovery::failure_message(&message),
        }),
        Ok(Request::PermissionRecoverySchedule { path, mode }) => {
            captures_app::permission_recovery::schedule_retry(Path::new(&path), mode)
                .map(|()| json!({"ok":true}))
                .unwrap_or_else(|error| json!({"ok":false,"error":error}))
        }
        Ok(Request::PermissionRecoveryTake { path }) => {
            captures_app::permission_recovery::take_pending_capture(Path::new(&path))
                .map(|mode| json!({"ok":true,"mode":mode}))
                .unwrap_or_else(|error| json!({"ok":false,"error":error}))
        }
        Ok(Request::PermissionRecoveryReset { path, bundle_id }) => {
            permission_recovery_reset(Path::new(&path), &bundle_id)
                .map(|()| json!({"ok":true}))
                .unwrap_or_else(|error| json!({"ok":false,"error":error}))
        }
        Ok(Request::OnboardingCopy) => {
            json!({"ok":true,"copy":captures_app::onboarding::copy()})
        }
        Ok(Request::LoginItem {
            history_root,
            settings_file,
            enabled,
        }) => captures_app::login_item::configure(
            Path::new(&history_root),
            Path::new(&settings_file),
            enabled,
        )
        .map(|enabled| json!({"ok":true,"enabled":enabled}))
        .unwrap_or_else(|error| json!({"ok":false,"error":error})),
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

/// Shipping resets only on macOS; elsewhere the dialog is never offered.
fn permission_recovery_reset(path: &Path, bundle_id: &str) -> Result<(), String> {
    if !captures_app::permission_recovery::supported() {
        return Err("Screen Recording recovery is only available on macOS.".into());
    }
    captures_app::permission_recovery::reset_screen_permission(
        path,
        bundle_id,
        &mut captures_app::permission_recovery::Tccutil,
    )?;
    ONBOARDING
        .lock()
        .map_err(|_| "The onboarding service is unavailable. Restart Captures.".to_owned())?
        .forget_screen_request();
    Ok(())
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

/// Runs a native application command. Heavy work must be scheduled off the UI thread.
/// Success is {"ok":true,"result":{...}}; failures have `error` text.
///
/// # Safety
/// `request_json` must be a readable NUL-terminated UTF-8 string during this call.
/// Release the result exactly once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_app_request_v1(request_json: *const c_char) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return json!({"ok":false,"error":"request pointer is null"});
        }
        // SAFETY: The caller upholds the same pointer contract as settings requests.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > MAX_REQUEST_BYTES {
            return json!({"ok":false,"error":"request exceeds 8 MiB"});
        }
        match serde_json::from_slice::<captures_app::Request>(bytes) {
            Ok(request) => match captures_app::execute(request) {
                Ok(response) => json!({"ok":true,"result":response}),
                Err(error) => json!({"ok":false,"error":error.to_string()}),
            },
            Err(error) => json!({"ok":false,"error":error.to_string()}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false,"error":"internal panic"}));
    CString::new(value.to_string())
        .expect("JSON contains no NUL bytes")
        .into_raw()
}

/// Releases a response returned by either native JSON request entry point.
///
/// # Safety
/// `response` must be null or a pointer returned by
/// one of this library's JSON request entry points that has not previously been freed.
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

    #[test]
    fn onboarding_abi_checks_without_creating_settings_and_rejects_unknown_actions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh settings.json");
        for (action, succeeds) in [("check", true), ("grant_everything", false)] {
            let input = CString::new(
                json!({"operation":"onboarding", "path":path,
                "action":action})
                .to_string(),
            )
            .unwrap();
            let pointer = unsafe { captures_settings_request_v1(input.as_ptr()) };
            let result: Value =
                serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
            unsafe { captures_settings_free_v1(pointer) };
            assert_eq!(result["ok"], succeeds);
            if succeeds {
                assert_eq!(result["state"]["onboarding_completed"], false);
                assert_eq!(result["state"]["platform"], std::env::consts::OS);
                assert!(result["state"]["screen_recording_required"].is_boolean());
                let presentation = &result["state"]["presentation"];
                assert!(presentation["title"].is_string());
                assert!(presentation["screen_ready"].is_boolean());
                assert!(presentation["primary_label"].is_string());
            }
            assert!(!path.exists());
        }
    }

    fn settings_request(request: Value) -> Value {
        let input = CString::new(request.to_string()).unwrap();
        let pointer = unsafe { captures_settings_request_v1(input.as_ptr()) };
        let result: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { captures_settings_free_v1(pointer) };
        result
    }

    #[test]
    fn permission_recovery_abi_shares_copy_and_pending_retry() {
        let prompt = settings_request(json!({"operation":"permission_recovery_prompt"}));
        assert_eq!(prompt["ok"], true);
        assert_eq!(prompt["supported"], cfg!(target_os = "macos"));
        assert_eq!(prompt["prompt"]["title"], "Captures Setup");
        assert_eq!(prompt["prompt"]["cancel"], "Not Now");
        assert!(
            ["restart", "reset_and_restart"]
                .contains(&prompt["prompt"]["recovery"].as_str().unwrap())
        );

        let denied = settings_request(json!({"operation":"permission_recovery_classify",
            "message":"Couldn’t start capture: screen capture permission was denied"}));
        assert_eq!(denied["denied"], true);
        assert_eq!(
            denied["failure"],
            "Captures could not reset or restart its Screen Recording setup: Couldn’t start capture: screen capture permission was denied"
        );
        let other = settings_request(json!({"operation":"permission_recovery_classify",
            "message":"screen capture permission was requested"}));
        assert_eq!(other["denied"], false);

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        captures_settings::write_atomic(&path, &AppSettings::default()).unwrap();
        let path = path.to_str().unwrap();
        let empty = settings_request(json!({"operation":"permission_recovery_take","path":path}));
        assert_eq!(empty, json!({"ok":true,"mode":null}));
        let scheduled = settings_request(json!({"operation":"permission_recovery_schedule",
            "path":path,"mode":"display"}));
        assert_eq!(scheduled["ok"], true);
        let taken = settings_request(json!({"operation":"permission_recovery_take","path":path}));
        assert_eq!(taken, json!({"ok":true,"mode":"display"}));
        let again = settings_request(json!({"operation":"permission_recovery_take","path":path}));
        assert_eq!(again["mode"], Value::Null);
        let invalid = settings_request(json!({"operation":"permission_recovery_schedule",
            "path":path,"mode":"everything"}));
        assert_eq!(invalid["ok"], false);
        // Never run tccutil from tests: off macOS the reset is refused outright.
        if !cfg!(target_os = "macos") {
            let reset = settings_request(json!({"operation":"permission_recovery_reset",
                "path":path,"bundle_id":"dev.captures.native"}));
            assert_eq!(reset["ok"], false);
        }
    }

    #[test]
    fn capture_error_abi_shares_the_shipping_dialog_copy() {
        let copy = settings_request(json!({"operation":"capture_error_copy"}));
        assert_eq!(copy["ok"], true);
        assert_eq!(copy["copy"]["title"], "Captures");
        assert_eq!(copy["copy"]["button"], "OK");
    }

    #[test]
    fn display_capture_route_abi_shares_the_recording_rule() {
        for (recording, route) in [
            (Value::Null, "capture_menu"),
            (json!("failed"), "capture_menu"),
            (json!("recording"), "capture_display"),
            (json!("paused"), "capture_display"),
            (json!("countdown"), "ignore"),
            (json!("finalizing"), "ignore"),
        ] {
            let response = settings_request(
                json!({"operation":"display_capture_route","recording":recording}),
            );
            assert_eq!(response["ok"], true);
            assert_eq!(response["route"], route, "{recording}");
        }
        // AppKit omits the key when no recording session exists.
        let idle = settings_request(json!({"operation":"display_capture_route"}));
        assert_eq!(idle["route"], "capture_menu");
    }

    #[test]
    fn new_capture_route_abi_shares_the_recording_rule() {
        // Region and window share `display_capture_route`'s recording states
        // (`capture_error::screenshot_route`); New Capture has its own rule.
        let idle = settings_request(json!({"operation":"new_capture_route"}));
        assert_eq!(idle["route"], "capture_menu");
        let hidden = settings_request(
            json!({"operation":"new_capture_route","recording":"paused","controls_hidden":true}),
        );
        assert_eq!(hidden["route"], "restore_controls");
        let busy =
            settings_request(json!({"operation":"new_capture_route","recording":"recording"}));
        assert_eq!(busy["route"], "in_progress");
        assert_eq!(
            busy["message"],
            "Captures could not start the capture: capture already in progress"
        );
    }

    #[test]
    fn history_presentation_abi_shares_copy_cards_and_grid() {
        let copy = settings_request(json!({"operation":"history_copy"}));
        assert_eq!(copy["ok"], true);
        assert_eq!(copy["copy"]["title"], "Capture History");
        assert_eq!(copy["copy"]["eyebrow"], "On this device");
        assert_eq!(copy["copy"]["delete_all_confirm"], "Delete all forever");
        assert_eq!(copy["copy"]["recovery_title"], "Interrupted recordings");
        assert_eq!(copy["confirm_timeout_ms"], 4_000);
        assert_eq!(copy["actions"]["edit"]["label"], "Edit");
        assert_eq!(copy["actions"]["save_file"]["busy"], "Saving…");
        assert_eq!(
            copy["actions"]["save_file"]["tooltip"],
            "Save a permanent copy to your Captures folder"
        );
        assert_eq!(copy["actions"]["show_in_folder"]["label"], "Show in Folder");
        assert_eq!(copy["actions"]["show_in_folder"]["done"], Value::Null);
        assert_eq!(copy["actions"]["restore"]["label"], "Restore");
        assert_eq!(copy["actions"]["restore"]["busy"], "Restoring…");
        assert_eq!(copy["actions"]["restore"]["done"], "Restored");
        assert_eq!(
            copy["actions"]["restore"]["tooltip"],
            "Bring this screenshot back as a floating preview"
        );
        assert_eq!(copy["actions"]["restore"]["icon"], "restore");
        assert_eq!(copy["feedback_ms"], 2_500);

        let entry = |kind: &str, id: &str| {
            json!({"id":id,"kind":kind,"preview_url":"","full_url":"","width":640,
                "height":480,"size_bytes":2_048,"created_at":"2026-09-26T15:04:05Z",
                "duration_ms":65_000,"dropped_frames":2})
        };
        let cards = settings_request(json!({"operation":"history_cards","cards":[
            {"entry":entry("screenshot","s"),"image_path":"/ignored.png"},
            {"entry":entry("video","v"),"missing":true,"media_path":"/gone.mp4"},
            {"entry":entry("gif","g"),"missing":false},
        ]}));
        assert_eq!(cards["ok"], true);
        let cards = cards["cards"].as_array().unwrap();
        assert_eq!(cards.len(), 3);
        assert_eq!(cards[0]["details"], "640 × 480 · 2.0 KB");
        assert_eq!(cards[0]["actions"], json!(["edit", "restore"]));
        assert_eq!(cards[0]["warning"], Value::Null);
        assert_eq!(cards[1]["missing"], true);
        assert_eq!(cards[1]["actions"], json!([]));
        assert_eq!(cards[1]["delete_label"], "Remove missing entry");
        assert_eq!(cards[2]["details"], "640 × 480 · 2.0 KB · 1:05");
        assert_eq!(cards[2]["warning"], "2 frames dropped while recording");
        assert_eq!(cards[2]["actions"], json!(["edit", "save_file"]));

        let grid = settings_request(json!({"operation":"history_grid","width":952.0}));
        assert_eq!(grid["ok"], true);
        assert_eq!(grid["grid"]["columns"], 3);
        let compact =
            settings_request(json!({"operation":"history_grid","width":588.0,"compact":true}));
        assert_eq!(compact["grid"]["columns"], 1);
        assert_eq!(
            grid["grid"]["card_height"],
            captures_app::history_view::CARD_HEIGHT
        );
        let partial = settings_request(json!({"operation":"history_cards","cards":[
            {"entry":{}}, {"entry":entry("screenshot","s")},
        ]}));
        assert_eq!(partial["ok"], true);
        assert_eq!(partial["cards"][0], Value::Null);
        assert_eq!(partial["cards"][1]["kind_label"], "Screenshot");
    }

    #[test]
    fn onboarding_copy_abi_is_available_without_a_settings_path() {
        let input = CString::new(r#"{"operation":"onboarding_copy"}"#).unwrap();
        let pointer = unsafe { captures_settings_request_v1(input.as_ptr()) };
        let result: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { captures_settings_free_v1(pointer) };
        assert_eq!(result["ok"], true);
        assert_eq!(result["copy"]["eyebrow"], "Welcome to Captures");
        assert_eq!(result["copy"]["lede"], captures_app::onboarding::LEDE);
        assert_eq!(result["copy"]["refresh"], "Refresh status");
        assert_eq!(result["copy"]["poll_interval_ms"], 1_500);
        assert_eq!(result["copy"]["settings_away_ms"], 2_500);

        let input = CString::new(
            json!({"operation":"onboarding_presentation","state":{
                "platform":"macos","onboarding_completed":false,
                "screen_recording_required":true,"screen_recording_granted":false,
                "screen_recording_can_request":false,
                "screen_recording_requested_this_launch":true,
                "microphone_granted":false,"microphone_can_request":false}})
            .to_string(),
        )
        .unwrap();
        let pointer = unsafe { captures_settings_request_v1(input.as_ptr()) };
        let result: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { captures_settings_free_v1(pointer) };
        assert_eq!(result["ok"], true);
        let presentation = &result["presentation"];
        assert_eq!(presentation["title"], "Required permissions");
        assert_eq!(presentation["restart_required"], true);
        assert_eq!(presentation["primary_label"], "Restart Captures");
        assert_eq!(presentation["screen_status"]["label"], "Restart required");
        assert_eq!(presentation["microphone_action"], "Allow microphone");
        assert_eq!(presentation["waiting_for_permission"], true);
    }

    #[test]
    fn flow_abi_rejects_invalid_or_stale_requests_without_registering_keys() {
        for (request, succeeds) in [
            (r#"{"operation":"begin","seconds":11}"#, false),
            (
                r#"{"operation":"begin_recording_screenshot","parent_generation":999,"seconds":0}"#,
                false,
            ),
            (r#"{"operation":"poll","generation":999}"#, false),
            (
                r#"{"operation":"start_countdown","generation":999,"seconds":3}"#,
                false,
            ),
            (
                r#"{"operation":"restart_countdown","generation":999,"seconds":3}"#,
                false,
            ),
            (r#"{"operation":"disarm_escape","generation":999}"#, false),
            (r#"{"operation":"finish","generation":999}"#, true),
            ("not json", false),
        ] {
            let input = CString::new(request).unwrap();
            let pointer = unsafe { captures_flow_request_v1(input.as_ptr()) };
            let response: Value =
                serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
            unsafe { captures_settings_free_v1(pointer) };
            assert_eq!(response["ok"], succeeds);
        }
    }

    #[test]
    fn application_abi_envelopes_and_ownership() {
        for (request, succeeds) in [
            (Some(r#"{"operation":"default_history_root"}"#), true),
            (Some(r#"{"operation":"unknown"}"#), false),
            (Some(r#"{"operation":"capture_display"}"#), false),
            (
                Some(r#"{"operation":"open_image","root":"/tmp","path":"/tmp/source.png"}"#),
                false,
            ),
            (
                Some(
                    r#"{"operation":"open_image","root":"/tmp","path":"/tmp/source.png","open_artifact_ids":"wrong"}"#,
                ),
                false,
            ),
            (
                Some(r#"{"operation":"open_media","root":"/tmp","path":"/tmp/source.png"}"#),
                false,
            ),
            (
                Some(
                    r#"{"operation":"open_media","root":"/tmp","path":"/tmp/source.png","open_artifact_ids":"wrong"}"#,
                ),
                false,
            ),
            (Some("not json"), false),
            (None, false),
        ] {
            let input = request.map(|s| CString::new(s).unwrap());
            let ptr = unsafe {
                captures_app_request_v1(input.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()))
            };
            assert!(!ptr.is_null());
            let result: Value =
                serde_json::from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes()).unwrap();
            unsafe { captures_settings_free_v1(ptr) };
            assert_eq!(result["ok"], succeeds);
            if succeeds {
                assert_eq!(result["result"]["kind"], "history_root");
                assert!(!result["result"]["path"].as_str().unwrap().is_empty());
            } else {
                assert!(result["result"].is_null());
                assert!(!result["error"].as_str().unwrap().is_empty());
            }
        }
    }

    #[test]
    fn application_abi_opens_image_with_owned_json_and_reports_already_open() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("capture-history");
        let source = data.path().join("source.png");
        image::RgbaImage::from_fn(9, 5, |x, y| {
            image::Rgba([x as u8 * 23, y as u8 * 37, 3, 255])
        })
        .save(&source)
        .unwrap();
        let request =
            json!({"operation":"open_image","root":root,"path":source,"open_artifact_ids":[]});
        let input = CString::new(request.to_string()).unwrap();
        let ptr = unsafe { captures_app_request_v1(input.as_ptr()) };
        let first: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes()).unwrap();
        unsafe { captures_settings_free_v1(ptr) };
        assert_eq!(first["ok"], true);
        assert_eq!(first["result"]["kind"], "opened_image");
        assert_eq!(first["result"]["already_open"], false);
        assert_eq!(first["result"]["artifact"]["entry"]["width"], 9);
        let id = first["result"]["artifact"]["entry"]["id"].as_str().unwrap();
        let input = CString::new(
            json!({"operation":"open_image","root":root,"path":source,"open_artifact_ids":[id]})
                .to_string(),
        )
        .unwrap();
        let ptr = unsafe { captures_app_request_v1(input.as_ptr()) };
        let second: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes()).unwrap();
        unsafe { captures_settings_free_v1(ptr) };
        assert_eq!(second["ok"], true);
        assert_eq!(second["result"]["already_open"], true);
        assert_eq!(second["result"]["artifact"]["entry"]["id"], id);
    }

    #[test]
    fn application_abi_opens_still_media_without_tools_and_preserves_old_envelope() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("capture-history");
        let source = data.path().join("source.webp");
        image::RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([x as u8 * 28, y as u8 * 67, 9, 255])
        })
        .save(&source)
        .unwrap();
        for (operation, kind) in [
            ("open_media", "opened_media"),
            ("open_image", "opened_image"),
        ] {
            let mut request =
                json!({"operation":operation,"root":root,"path":source,"open_artifact_ids":[]});
            if operation == "open_media" {
                request["ffmpeg"] = json!("/missing-ffmpeg");
                request["ffprobe"] = json!("/missing-ffprobe");
            }
            let input = CString::new(request.to_string()).unwrap();
            let ptr = unsafe { captures_app_request_v1(input.as_ptr()) };
            let response: Value =
                serde_json::from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes()).unwrap();
            unsafe { captures_settings_free_v1(ptr) };
            assert_eq!(response["ok"], true);
            assert_eq!(response["result"]["kind"], kind);
            assert_eq!(response["result"]["artifact"]["entry"]["width"], 7);
        }
    }

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
        let motion = call(r#"{"operation":"motion"}"#);
        assert_eq!(motion["ok"], true);
        assert_eq!(
            motion["motion"]["keyframes"]["update_notice_in"]["easing"]["token"],
            "ease-out"
        );
        assert!(motion["motion"]["transitions"]["segmented_indicator"].is_object());
        assert_eq!(call("not json")["ok"], false);
        let ptr = unsafe { captures_settings_request_v1(std::ptr::null()) };
        assert!(!ptr.is_null());
        unsafe { captures_settings_free_v1(ptr) };
        unsafe { captures_settings_free_v1(std::ptr::null_mut()) };
    }
}
