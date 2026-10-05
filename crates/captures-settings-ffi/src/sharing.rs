//! Opaque native worker bridge. Only commands/events cross the ABI, never the
//! bearer, OTP challenge, vault contents, or temporary snapshot path.
use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::Arc,
};

use captures_account::{
    native::{Auth, Command, Event, Selection, State, Worker, error_text},
    sharing::{Opened, SharePatch},
};
use serde::Deserialize;
use serde_json::{Value, json};

pub struct CapturesSharing {
    worker: Worker,
    busy: bool,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Open {
        artifact_id: String,
        path: PathBuf,
        name: String,
        content_type: String,
    },
    Refresh {},
    #[serde(rename = "request_code")]
    SendCode {
        email: String,
    },
    Verify {
        code: String,
    },
    RetrySave {},
    Logout {},
    Upload {
        #[serde(default)]
        patch: SharePatch,
    },
    Configure {
        enabled: bool,
        #[serde(default)]
        patch: SharePatch,
    },
    Trash {},
    Restore {},
    Cancel {},
    Poll {},
}

impl CapturesSharing {
    fn request(&mut self, request: Request) -> Result<Value, &'static str> {
        let command = match request {
            Request::Poll {} => {
                let mut events = Vec::new();
                while let Some(event) = self.worker.try_recv() {
                    events.push(match event {
                        Event::Progress { read, total } => {
                            json!({"event":"progress", "read":read, "total":total})
                        }
                        Event::Finished(state) => {
                            self.busy = false;
                            finished(state)
                        }
                    });
                }
                return Ok(json!({"events":events}));
            }
            Request::Cancel {} => {
                self.worker.cancel();
                return Ok(json!({"accepted":true}));
            }
            Request::Open {
                artifact_id,
                path,
                name,
                content_type,
            } => Command::Open(Selection {
                artifact_id,
                path,
                name,
                content_type,
            }),
            Request::Refresh {} => Command::Refresh,
            Request::SendCode { email } => Command::RequestCode(email),
            Request::Verify { code } => Command::Verify(code),
            Request::RetrySave {} => Command::RetrySave,
            Request::Logout {} => Command::Logout,
            Request::Upload { patch } => Command::Upload(patch),
            Request::Configure { enabled, patch } => Command::Configure { enabled, patch },
            Request::Trash {} => Command::Trash,
            Request::Restore {} => Command::Restore,
        };
        if self.busy {
            return Err("Wait for the accepted sharing operation to finish.");
        }
        if !self.worker.send(command) {
            return Err("Sharing worker is unavailable.");
        }
        self.busy = true;
        Ok(json!({"accepted":true}))
    }
}

fn finished(state: State) -> Value {
    let auth = match &state.auth {
        Auth::SignedOut => json!({"status":"signed_out"}),
        Auth::CodeSent => json!({"status":"code_sent"}),
        Auth::SaveRequired => json!({"status":"save_required"}),
        Auth::Unavailable => json!({"status":"unavailable"}),
        Auth::SignedIn(user) => json!({"status":"signed_in", "email":user.email}),
    };
    let opened = match &state.opened {
        Opened::Unassociated => json!({"status":"unassociated"}),
        Opened::Pending => json!({"status":"pending"}),
        Opened::Asset(asset) => json!({"status":"asset", "asset":asset}),
    };
    let link = if state.error.is_none() && matches!(state.auth, Auth::SignedIn(_)) {
        match &state.opened {
            Opened::Asset(asset) if asset.deleted_at.is_none() => asset
                .share
                .as_ref()
                .map(|s| format!("{}/s/{}", captures_account::DEFAULT_API, s.id)),
            _ => None,
        }
    } else {
        None
    };
    json!({"event":"finished", "auth":auth, "opened":opened,
        "error":state.error.as_ref().map(error_text), "link":link})
}

/// Construct lazily after an explicit Share action, without vault/network I/O.
///
/// # Safety
/// `root` must be readable NUL-terminated UTF-8 for this call. The returned
/// handle must be exclusively owned and freed once with `captures_sharing_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_sharing_create_v1(root: *const c_char) -> *mut CapturesSharing {
    catch_unwind(AssertUnwindSafe(|| {
        if root.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: Caller upholds the documented string lifetime.
        let Ok(root) = unsafe { CStr::from_ptr(root) }.to_str() else {
            return std::ptr::null_mut();
        };
        if root.is_empty() || root.len() > 64 * 1024 {
            return std::ptr::null_mut();
        }
        Box::into_raw(Box::new(CapturesSharing {
            worker: Worker::production(root.into(), Arc::new(|| {})),
            busy: false,
        }))
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Nonblocking command submission/event draining. Serialize calls for a handle.
/// Password/code-bearing parse errors deliberately never echo input.
///
/// # Safety
/// `handle` is live and exclusively accessed during this call; `request_json`
/// is readable NUL-terminated UTF-8. Free the returned owned JSON once with
/// `captures_settings_free_v1`. Neither pointer may alias the returned buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_sharing_request_v1(
    handle: *mut CapturesSharing,
    request_json: *const c_char,
) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        let result = (|| {
            if handle.is_null() || request_json.is_null() {
                return Err("Sharing pointer is null.");
            }
            // SAFETY: Caller upholds the exclusive handle/string contracts.
            let bytes = unsafe { CStr::from_ptr(request_json) }.to_bytes();
            if bytes.len() > 64 * 1024 {
                return Err("Sharing request exceeds 64 KiB.");
            }
            let request = serde_json::from_slice(bytes).map_err(|_| "Invalid sharing command.")?;
            unsafe { &mut *handle }.request(request)
        })();
        match result {
            Ok(result) => json!({"ok":true, "result":result}),
            Err(error) => json!({"ok":false, "error":error}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false, "error":"Sharing bridge failed."}));
    CString::new(value.to_string())
        .expect("JSON contains no NUL")
        .into_raw()
}

/// Cancel/join the retained worker before releasing the native profile lock.
/// May wait for bounded HTTP work or an OS credential prompt. Call off the UI
/// thread; ordinary popup/preview closure must not free the handle.
///
/// # Safety
/// `handle` is null or the live exclusively owned handle returned by create.
/// No requests may overlap this call or use the handle afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_sharing_free_v1(handle: *mut CapturesSharing) {
    if !handle.is_null() {
        // SAFETY: Caller transfers exclusive ownership exactly once.
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_account::{
        AccountClient, Vault, VaultError,
        sharing::{AssetInfo, Patch, ShareInfo},
    };
    use std::{sync::mpsc, time::Duration};

    struct SignedOut;
    impl Vault for SignedOut {
        fn load(&self) -> Result<Option<String>, VaultError> {
            Ok(None)
        }
        fn save(&self, _: &str) -> Result<(), VaultError> {
            unreachable!()
        }
        fn delete(&self) -> Result<(), VaultError> {
            unreachable!()
        }
    }

    fn call(handle: *mut CapturesSharing, request: &str) -> Value {
        let input = CString::new(request).unwrap();
        let pointer = unsafe { captures_sharing_request_v1(handle, input.as_ptr()) };
        let result = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(pointer) };
        result
    }

    #[test]
    fn patch_wire_distinguishes_omitted_clear_and_set_without_debugging_secrets() {
        let p: SharePatch = serde_json::from_str("{}").unwrap();
        assert!(matches!(p.password, Patch::Keep));
        assert!(matches!(p.expires_at, Patch::Keep));
        let p: SharePatch =
            serde_json::from_str(r#"{"password":null,"expiresAt":"2028-04-02T13:20:00Z"}"#)
                .unwrap();
        assert!(matches!(p.password, Patch::Clear));
        assert!(matches!(p.expires_at, Patch::Set(v) if v == "2028-04-02T13:20:00Z"));
        let p: SharePatch =
            serde_json::from_str(r#"{"password":"private-password","expiresAt":null}"#).unwrap();
        assert!(matches!(p.password, Patch::Set(v) if v == "private-password"));
        assert!(matches!(p.expires_at, Patch::Clear));
    }

    #[test]
    fn malformed_secret_commands_and_endpoint_overrides_fail_without_echo() {
        let root = tempfile::tempdir().unwrap();
        let handle = Box::into_raw(Box::new(CapturesSharing {
            worker: Worker::with_client(
                root.path().into(),
                AccountClient::new("http://127.0.0.1:9", SignedOut).unwrap(),
                Arc::new(|| {}),
            ),
            busy: false,
        }));
        for request in [
            r#"{"operation":"verify","code":{"private-secret":true}}"#,
            r#"{"operation":"upload","patch":{"password":42,"private-secret":true}}"#,
            r#"{"operation":"refresh","origin":"https://private-secret.invalid"}"#,
            r#"{"operation":"upload","patch":{"endpoint":"private-secret"}}"#,
        ] {
            let response = call(handle, request);
            assert_eq!(
                response,
                json!({"ok":false,"error":"Invalid sharing command."})
            );
            assert!(!response.to_string().contains("private-secret"));
        }
        assert_eq!(
            call(handle, r#"{"operation":"poll"}"#)["result"]["events"],
            json!([])
        );
        unsafe { captures_sharing_free_v1(handle) };
    }

    #[test]
    fn open_is_nonblocking_busy_selection_is_pinned_and_poll_drains_the_reply() {
        struct Gated {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Vault for Gated {
            fn load(&self) -> Result<Option<String>, VaultError> {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(None)
            }
            fn save(&self, _: &str) -> Result<(), VaultError> {
                unreachable!()
            }
            fn delete(&self) -> Result<(), VaultError> {
                unreachable!()
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original.png");
        std::fs::write(&path, b"original bytes").unwrap();
        let (entered, wait) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let handle = Box::into_raw(Box::new(CapturesSharing {
            worker: Worker::with_client(
                root.path().into(),
                AccountClient::new(
                    "http://127.0.0.1:9",
                    Gated {
                        entered,
                        release: gate,
                    },
                )
                .unwrap(),
                Arc::new(|| {}),
            ),
            busy: false,
        }));
        let request = json!({"operation":"open","artifact_id":"selected-original","path":path,"name":"Capture.png","content_type":"image/png"}).to_string();
        assert_eq!(call(handle, &request)["result"]["accepted"], true);
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(call(handle, &request)["ok"], false);
        assert_eq!(
            call(handle, r#"{"operation":"poll"}"#)["result"]["events"],
            json!([])
        );
        assert_eq!(call(handle, r#"{"operation":"cancel"}"#)["ok"], true);
        release.send(()).unwrap();
        let event = (0..200)
            .find_map(|_| {
                let response = call(handle, r#"{"operation":"poll"}"#);
                let found = response["result"]["events"]
                    .as_array()
                    .unwrap()
                    .first()
                    .cloned();
                if found.is_none() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                found
            })
            .unwrap();
        assert_eq!(
            event,
            json!({"event":"finished","auth":{"status":"signed_out"},"opened":{"status":"unassociated"},"error":null,"link":null})
        );
        assert_eq!(
            call(handle, r#"{"operation":"poll"}"#)["result"]["events"],
            json!([])
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original bytes");
        assert!(!root.path().join("native-share-associations.json").exists());
        unsafe { captures_sharing_free_v1(handle) };
    }

    #[test]
    fn finished_link_requires_successful_signed_in_ready_nontrashed_share() {
        fn state() -> State {
            State {
                auth: Auth::SignedIn(captures_account::User {
                    id: "owner".into(),
                    email: "owner@example.com".into(),
                }),
                opened: Opened::Asset(Box::new(AssetInfo {
                    id: "asset".into(),
                    name: "Capture.png".into(),
                    content_type: "image/png".into(),
                    byte_size: 7,
                    created_at: "2026-10-05T10:00:00Z".into(),
                    deleted_at: None,
                    share: Some(ShareInfo {
                        id: "link".into(),
                        password_protected: true,
                        expires_at: None,
                        shared_at: "2026-10-05T10:01:00Z".into(),
                    }),
                })),
                error: None,
            }
        }
        assert_eq!(finished(state())["link"], "https://captur.es/s/link");
        let mut failed = state();
        failed.error = Some(captures_account::sharing::Error::Unavailable);
        assert!(finished(failed)["link"].is_null());
        let mut signed_out = state();
        signed_out.auth = Auth::SignedOut;
        assert!(finished(signed_out)["link"].is_null());
        let mut pending = state();
        pending.opened = Opened::Pending;
        assert!(finished(pending)["link"].is_null());
        let mut trashed = state();
        if let Opened::Asset(a) = &mut trashed.opened {
            a.deleted_at = Some("2026-10-05T11:00:00Z".into());
        }
        assert!(finished(trashed)["link"].is_null());
        let mut private = state();
        if let Opened::Asset(a) = &mut private.opened {
            a.share = None;
        }
        assert!(finished(private)["link"].is_null());
    }
}
