//! Pure update-notice copy, fixture statuses and tray placement for AppKit.
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use captures_app::{
    tray_notice::LogicalRect,
    update_notice::{self, StubEvent, UpdateStatus, ViewState},
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Present {
        status: Option<UpdateStatus>,
        #[serde(default)]
        view: ViewState,
    },
    Fixture {
        name: String,
    },
    Stub {
        status: UpdateStatus,
        event: StubEvent,
    },
    Placement {
        monitor: LogicalRect,
        work_area: LogicalRect,
        tray: Option<LogicalRect>,
        menu_bar_at_top: bool,
        card_width: f64,
        card_height: f64,
    },
}

fn finite(rect: &LogicalRect) -> bool {
    [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|value| value.is_finite())
}

fn respond(bytes: &[u8]) -> Result<Value, String> {
    match serde_json::from_slice::<Request>(bytes).map_err(|error| error.to_string())? {
        Request::Present { status, view } => {
            Ok(json!(update_notice::present(status.as_ref(), &view)))
        }
        Request::Fixture { name } => update_notice::fixture(&name)
            .map(|status| json!({"status": status}))
            .ok_or_else(|| format!("Unknown update notice fixture {name}")),
        Request::Stub { status, event } => {
            let next = update_notice::stub_next(&status, event);
            let interval = next.as_ref().and_then(update_notice::stub_tick_interval_ms);
            Ok(json!({"status": next, "tick_ms": interval}))
        }
        Request::Placement {
            monitor,
            work_area,
            tray,
            menu_bar_at_top,
            card_width,
            card_height,
        } => {
            if !finite(&monitor)
                || !finite(&work_area)
                || tray.as_ref().is_some_and(|tray| !finite(tray))
                || !card_width.is_finite()
                || !card_height.is_finite()
                || card_width <= 0.
                || card_height <= 0.
            {
                return Err("Notice placement needs finite, positive geometry".into());
            }
            let placement = captures_app::update_notice::placement(
                monitor,
                work_area,
                tray,
                menu_bar_at_top,
                card_width,
                card_height,
            );
            Ok(json!({"placement": placement, "card": placement.card_rect()}))
        }
    }
}

/// UI-thread-safe pure update notice requests. See the header for operations.
///
/// # Safety
/// `request_json` is readable NUL-terminated UTF-8 for this call; free the
/// returned owned JSON exactly once with `captures_settings_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_update_notice_request_v1(
    request_json: *const c_char,
) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        if request_json.is_null() {
            return json!({"ok":false,"error":"request pointer is null"});
        }
        // SAFETY: Caller upholds the same contract as the other JSON ABIs.
        let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
        if bytes.len() > 1024 * 1024 {
            return json!({"ok":false,"error":"update notice request exceeds 1 MiB"});
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
        let pointer = unsafe { captures_update_notice_request_v1(input.as_ptr()) };
        let value = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(pointer) };
        value
    }

    #[test]
    fn presents_fixtures_steps_the_stub_and_places_the_card() {
        let fixture = call(r#"{"operation":"fixture","name":"available"}"#);
        assert_eq!(fixture["ok"], true);
        let status = &fixture["result"]["status"];
        let present = call(
            &json!({"operation":"present","status":status,"view":{"show_changelog":false}})
                .to_string(),
        );
        assert_eq!(present["result"]["title"], "Update available");
        assert_eq!(present["result"]["reveal_notes"]["label"], "What’s new");
        assert_eq!(present["result"]["card_height"], 168.0);
        let loading = call(r#"{"operation":"present","status":null}"#);
        assert_eq!(loading["result"]["title"], "Loading update details");

        let step = call(&json!({"operation":"stub","status":status,"event":"install"}).to_string());
        assert_eq!(step["result"]["status"]["state"], "downloading");
        assert_eq!(step["result"]["tick_ms"], 400);

        let placed = call(
            r#"{"operation":"placement","monitor":{"x":0,"y":0,"width":1440,"height":900},
            "work_area":{"x":0,"y":25,"width":1440,"height":875},
            "tray":{"x":1200,"y":0,"width":24,"height":24},"menu_bar_at_top":true,
            "card_width":400,"card_height":168}"#,
        );
        assert_eq!(placed["result"]["placement"]["caret"], "top");
        assert_eq!(placed["result"]["card"]["x"], 28.0);

        for request in [
            r#"{"operation":"fixture","name":"nope"}"#,
            r#"{"operation":"stub","status":{"state":"idle"},"event":"tick"}"#,
            r#"{"operation":"placement","monitor":{"x":0,"y":0,"width":1,"height":1},
            "work_area":{"x":0,"y":0,"width":1,"height":1},"tray":null,
            "menu_bar_at_top":false,"card_width":0,"card_height":1}"#,
            "not json",
        ] {
            assert_eq!(call(request)["ok"], false, "{request}");
        }
        let pointer = unsafe { captures_update_notice_request_v1(std::ptr::null()) };
        assert!(!pointer.is_null());
        unsafe { crate::captures_settings_free_v1(pointer) };
    }
}
