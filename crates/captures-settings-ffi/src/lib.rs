mod capture_menu;
mod editor;
mod editor_export;
mod feedback;
mod icons;
mod instance;
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
        assert_eq!(call("not json")["ok"], false);
        let ptr = unsafe { captures_settings_request_v1(std::ptr::null()) };
        assert!(!ptr.is_null());
        unsafe { captures_settings_free_v1(ptr) };
        unsafe { captures_settings_free_v1(std::ptr::null_mut()) };
    }
}
