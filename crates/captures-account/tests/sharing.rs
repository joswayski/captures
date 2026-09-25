use captures_account::{
    AccountClient, Error as AccountError, Vault, VaultError,
    sharing::{AssociationStore, Error, Opened, Patch, Progress, SharePatch, SharingCoordinator},
};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

struct Fake;
impl Vault for Fake {
    fn load(&self) -> Result<Option<String>, VaultError> {
        Ok(Some("opaqueTOKEN".into()))
    }
    fn save(&self, _: &str) -> Result<(), VaultError> {
        unreachable!()
    }
    fn delete(&self) -> Result<(), VaultError> {
        Ok(())
    }
}

#[derive(Default)]
struct State {
    requests: Vec<(String, String, Vec<u8>)>,
    create: usize,
    parts: Vec<(String, Vec<u8>)>,
    completed: Vec<serde_json::Value>,
    shares: Vec<serde_json::Value>,
    expire_first: bool,
    fail_share_once: bool,
    fail_complete_once: bool,
    reject_signed_headers: bool,
    redirect_object_once: bool,
    deny_share: bool,
    other_account: bool,
    ambiguous_create: bool,
    deleted: bool,
    share_id: usize,
}
struct Server {
    url: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, done, base) = (state.clone(), stop.clone(), url.clone());
        let handle = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let Ok((mut socket, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut socket);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.is_empty() {
                        break;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if line.to_ascii_lowercase().starts_with("content-length:") {
                        length = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                    }
                    headers.push_str(&line.to_ascii_lowercase());
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                drop(reader);
                let mut state = shared.lock().unwrap();
                let line = first.trim().to_owned();
                state
                    .requests
                    .push((line.clone(), headers.clone(), body.clone()));
                let (status, output, extra) = if line.starts_with("GET /api/account/me ") {
                    (
                        "200 OK",
                        if state.other_account {
                            r#"{"user":{"id":"owner2","email":"b@example.com"}}"#
                        } else {
                            r#"{"user":{"id":"owner1","email":"a@example.com"}}"#
                        }
                        .to_owned(),
                        "",
                    )
                } else if line.starts_with("PUT /api/asset-uploads/") {
                    state.create += 1;
                    if state.ambiguous_create {
                        continue;
                    } // Server may have committed; no response.
                    (
                        "201 Created",
                        r#"{"id":"asset1","partSize":3,"partCount":2}"#.to_owned(),
                        "",
                    )
                } else if line.starts_with("POST /api/assets/asset1/parts ") {
                    let part =
                        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["partNumber"]
                            .as_u64()
                            .unwrap();
                    let key = if state.reject_signed_headers {
                        "authorization"
                    } else {
                        "x-part"
                    };
                    (
                        "200 OK",
                        format!(
                            r#"{{"url":"{base}/object?part={part}","headers":{{"{key}":"{part}"}}}}"#
                        ),
                        "",
                    )
                } else if line.starts_with("PUT /object?") {
                    state.parts.push((line.clone(), body));
                    if state.redirect_object_once {
                        state.redirect_object_once = false;
                        (
                            "302 Found",
                            String::new(),
                            "Location: http://127.0.0.1:9/untrusted\r\n",
                        )
                    } else if state.expire_first {
                        state.expire_first = false;
                        ("403 Forbidden", "expired".into(), "")
                    } else {
                        (
                            "200 OK",
                            String::new(),
                            if line.contains("part=1") {
                                "ETag: \"etag-one\"\r\n"
                            } else {
                                "ETag: \"etag-two\"\r\n"
                            },
                        )
                    }
                } else if line.starts_with("POST /api/assets/asset1/complete ") {
                    state.completed.push(serde_json::from_slice(&body).unwrap());
                    if state.fail_complete_once {
                        state.fail_complete_once = false;
                        ("503 Service Unavailable", "{}".into(), "")
                    } else {
                        ("200 OK", asset(false, state.share_id), "")
                    }
                } else if line.starts_with("PUT /api/assets/asset1/share ") {
                    state.shares.push(serde_json::from_slice(&body).unwrap());
                    if state.deny_share {
                        ("401 Unauthorized", "{}".into(), "")
                    } else if state.fail_share_once {
                        state.fail_share_once = false;
                        ("503 Service Unavailable", "{}".into(), "")
                    } else {
                        if state.shares.last().unwrap()["enabled"] == false {
                            state.share_id = 0;
                        } else if state.share_id == 0 {
                            state.share_id = 1 + state
                                .shares
                                .iter()
                                .filter(|v| v["enabled"] == false)
                                .count();
                        }
                        let output = if state.share_id == 0 {
                            r#"{"share":null}"#.into()
                        } else {
                            format!(
                                r#"{{"share":{{"id":"share{}","passwordProtected":false,"expiresAt":null,"sharedAt":"2026-09-25T00:00:00Z"}}}}"#,
                                state.share_id
                            )
                        };
                        ("200 OK", output, "")
                    }
                } else if line.starts_with("GET /api/assets?deleted=true ") {
                    (
                        "200 OK",
                        format!(r#"{{"assets":[{}]}}"#, asset(true, 0)),
                        "",
                    )
                } else if line.starts_with("GET /api/assets ") {
                    (
                        "200 OK",
                        if state.deleted {
                            r#"{"assets":[]}"#.into()
                        } else {
                            format!(r#"{{"assets":[{}]}}"#, asset(false, state.share_id))
                        },
                        "",
                    )
                } else if line.starts_with("DELETE /api/assets/asset1 ") {
                    state.deleted = true;
                    state.share_id = 0;
                    ("204 No Content", String::new(), "")
                } else if line.starts_with("POST /api/assets/asset1/restore ") {
                    state.deleted = false;
                    ("200 OK", asset(false, 0), "")
                } else {
                    panic!("unexpected request path: {line}");
                };
                write!(socket, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{output}", output.len()).unwrap();
            }
        });
        Self {
            url,
            state,
            stop,
            handle: Some(handle),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
    }
}
fn asset(deleted: bool, share_id: usize) -> String {
    let share = if share_id == 0 {
        "null".into()
    } else {
        format!(
            r#"{{"id":"share{share_id}","passwordProtected":false,"expiresAt":null,"sharedAt":"2026-09-25T00:00:00Z"}}"#
        )
    };
    format!(
        r#"{{"id":"asset1","name":"original.png","contentType":"image/png","byteSize":5,"createdAt":"2026-09-25T00:00:00Z","deletedAt":{},"share":{share}}}"#,
        if deleted {
            r#""2026-09-25T01:00:00Z""#
        } else {
            "null"
        }
    )
}
fn client(url: &str) -> AccountClient<Fake> {
    let mut client = AccountClient::new(url, Fake).unwrap();
    assert!(client.load().unwrap());
    client
}
fn progress() -> Progress {
    Arc::new(|_, _| ())
}
fn cancelled() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[test]
fn upload_retries_expired_part_without_credentials_then_configures_without_reupload() {
    let server = Server::new();
    server.state.lock().unwrap().expire_first = true;
    server.state.lock().unwrap().fail_share_once = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    fs::write(&path, b"ABCDE").unwrap();
    let mut account = client(&server.url);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert!(matches!(
        flow.open("artifact1").unwrap(),
        Opened::Unassociated
    ));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let samples = Arc::clone(&observed);
    let progress: Progress =
        Arc::new(move |sent, total| samples.lock().unwrap().push((sent, total)));
    let asset = flow
        .upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress,
        )
        .unwrap();
    assert_eq!(asset.id, "asset1");
    assert!(asset.share.is_none());
    let observed = observed.lock().unwrap();
    assert!(!observed.is_empty());
    assert!(
        observed
            .iter()
            .all(|(sent, total)| *total == 5 && *sent <= 5)
    );
    assert_eq!(observed.last(), Some(&(5, 5)));
    let association =
        fs::read_to_string(dir.path().join("native-share-associations.json")).unwrap();
    assert!(association.contains("asset1"));
    assert!(!association.contains("opaqueTOKEN"));
    assert!(!association.contains("abcdefgh"));
    let patch = SharePatch {
        password: Patch::Set("abcdefgh".into()),
        expires_at: Patch::Clear,
    };
    assert_eq!(
        flow.configure_share("artifact1", true, patch).err(),
        Some(Error::Unavailable)
    );
    let share = flow
        .configure_share("artifact1", true, SharePatch::default())
        .unwrap()
        .unwrap();
    assert_eq!(share.id, "share1");
    drop(flow);
    let mut reopened =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert!(
        matches!(reopened.open("artifact1").unwrap(), Opened::Asset(asset) if asset.share.as_ref().unwrap().id == "share1")
    );
    reopened
        .configure_share(
            "artifact1",
            true,
            SharePatch {
                password: Patch::Clear,
                expires_at: Patch::Set("2099-02-01T00:00:00Z".into()),
            },
        )
        .unwrap();
    reopened
        .configure_share("artifact1", false, SharePatch::default())
        .unwrap();
    assert_eq!(
        reopened
            .configure_share("artifact1", true, SharePatch::default())
            .unwrap()
            .unwrap()
            .id,
        "share2"
    );
    reopened.trash("artifact1").unwrap();
    assert!(
        matches!(reopened.open("artifact1").unwrap(), Opened::Asset(asset) if asset.deleted_at.is_some() && asset.share.is_none())
    );
    assert!(reopened.restore("artifact1").unwrap().share.is_none());
    let state = server.state.lock().unwrap();
    assert_eq!(state.create, 1);
    assert_eq!(state.parts.len(), 3);
    assert_eq!(
        state
            .parts
            .iter()
            .map(|(_, data)| data.as_slice())
            .collect::<Vec<_>>(),
        vec![b"ABC".as_slice(), b"ABC".as_slice(), b"DE".as_slice()]
    );
    assert_eq!(
        state.completed[0],
        serde_json::json!({"parts":[{"partNumber":1,"etag":"\"etag-one\""},{"partNumber":2,"etag":"\"etag-two\""}]})
    );
    assert_eq!(
        state.shares[0],
        serde_json::json!({"enabled":true,"password":"abcdefgh","expiresAt":null})
    );
    assert_eq!(state.shares[1], serde_json::json!({"enabled":true}));
    assert_eq!(
        state.shares[2],
        serde_json::json!({"enabled":true,"password":null,"expiresAt":"2099-02-01T00:00:00Z"})
    );
    for (line, headers, _) in &state.requests {
        assert!(!headers.contains("cookie:"));
        assert!(!headers.contains("origin:"));
        if line.starts_with("PUT /object?") {
            assert!(!headers.contains("authorization:"));
            assert!(headers.contains("x-part:"));
        } else {
            assert!(headers.contains("authorization: bearer opaquetoken"));
        }
        assert!(!line.contains("opaqueTOKEN"));
    }
}

#[test]
fn ambiguous_create_reuses_durable_key_after_restart_and_missing_file_never_creates() {
    let server = Server::new();
    server.state.lock().unwrap().ambiguous_create = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    let mut account = client(&server.url);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::MissingFile)
    );
    fs::write(&path, b"ABCDE").unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::CreateUncertain)
    );
    drop(flow);
    let association = dir.path().join("native-share-associations.json");
    let creating = fs::read(&association).unwrap();
    server.state.lock().unwrap().ambiguous_create = false;
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert!(matches!(flow.open("artifact1").unwrap(), Opened::Pending));
    assert_eq!(server.state.lock().unwrap().create, 1); // Opening never retries.
    flow.upload(
        "artifact1",
        &path,
        "original.png",
        "image/png",
        cancelled(),
        progress(),
    )
    .unwrap();
    drop(flow);
    // The same durable state remains if a process dies after receiving create
    // but before saving the asset ID. Explicit retry must use that same key.
    fs::write(&association, &creating).unwrap();
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    flow.upload(
        "artifact1",
        &path,
        "original.png",
        "image/png",
        cancelled(),
        progress(),
    )
    .unwrap();
    drop(flow);
    let state = server.state.lock().unwrap();
    let creates: Vec<_> = state
        .requests
        .iter()
        .filter(|(line, _, _)| line.starts_with("PUT /api/asset-uploads/"))
        .collect();
    assert_eq!(creates.len(), 3);
    assert!(
        creates
            .iter()
            .all(|(line, _, body)| line == &creates[0].0 && body == &creates[0].2)
    );
    drop(state);
    // Pre-key checkpoint records cannot safely infer an identity or retry.
    let mut legacy: serde_json::Value = serde_json::from_slice(&creating).unwrap();
    legacy["accounts"]["owner1"]["artifact1"]
        .as_object_mut()
        .unwrap()
        .remove("create_key");
    fs::write(&association, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::CreateUncertain)
    );
    assert_eq!(server.state.lock().unwrap().create, 3);
}

#[test]
fn cancellation_before_create_leaves_no_association() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    fs::write(&path, b"ABCDE").unwrap();
    let mut account = client(&server.url);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    let cancel = cancelled();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancel,
            progress()
        )
        .err(),
        Some(Error::Cancelled)
    );
    assert!(matches!(
        flow.open("artifact1").unwrap(),
        Opened::Unassociated
    ));
    assert_eq!(server.state.lock().unwrap().create, 0);
}

#[test]
fn interrupted_part_and_uncertain_completion_resume_without_create_or_reupload() {
    let server = Server::new();
    server.state.lock().unwrap().fail_complete_once = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    fs::write(&path, b"ABCDE").unwrap();
    let mut account = client(&server.url);
    let cancel = cancelled();
    let trigger = cancel.clone();
    let callback: Progress = Arc::new(move |sent, _| {
        if sent == 3 {
            trigger.store(true, Ordering::Relaxed);
        }
    });
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancel,
            callback
        )
        .err(),
        Some(Error::Cancelled)
    );
    drop(flow);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::Unavailable)
    );
    drop(flow);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .unwrap()
        .id,
        "asset1"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.create, 1);
    assert_eq!(state.parts.len(), 3); // Part 1 was sent, then cancelled before its ETag was saved.
    assert_eq!(state.completed.len(), 2);
}

#[test]
fn untrusted_signed_headers_and_changed_source_fail_closed() {
    let server = Server::new();
    server.state.lock().unwrap().reject_signed_headers = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    fs::write(&path, b"ABCDE").unwrap();
    let mut account = client(&server.url);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::Protocol)
    );
    fs::write(&path, b"ABCDF").unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::ChangedFile)
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.create, 1);
    assert!(state.parts.is_empty());
    assert!(state.completed.is_empty());
}

#[test]
fn redirect_is_not_followed_and_resume_uses_same_asset() {
    let server = Server::new();
    server.state.lock().unwrap().redirect_object_once = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    fs::write(&path, b"ABCDE").unwrap();
    let mut account = client(&server.url);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::Protocol)
    );
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .unwrap()
        .id,
        "asset1"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.create, 1);
    assert_eq!(state.parts.len(), 3);
    assert_eq!(state.completed.len(), 1);
}

#[test]
fn association_is_scoped_to_account_and_profile_and_corruption_does_not_create() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    fs::write(&path, b"ABCDE").unwrap();
    let mut account = client(&server.url);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    flow.upload(
        "artifact1",
        &path,
        "original.png",
        "image/png",
        cancelled(),
        progress(),
    )
    .unwrap();
    drop(flow);
    assert!(matches!(
        SharingCoordinator::new(&mut account, AssociationStore::new(second.path()))
            .unwrap()
            .open("artifact1")
            .unwrap(),
        Opened::Unassociated
    ));
    server.state.lock().unwrap().other_account = true;
    assert!(matches!(
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path()))
            .unwrap()
            .open("artifact1")
            .unwrap(),
        Opened::Unassociated
    ));
    server.state.lock().unwrap().other_account = false;
    fs::write(
        dir.path().join("native-share-associations.json"),
        b"not-json",
    )
    .unwrap();
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    assert_eq!(
        flow.upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress()
        )
        .err(),
        Some(Error::Storage)
    );
    assert_eq!(server.state.lock().unwrap().create, 1);
}

#[test]
fn unauthorized_share_invalidates_account_without_exposing_a_link() {
    let server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.png");
    fs::write(&path, b"ABCDE").unwrap();
    let mut account = client(&server.url);
    let mut flow =
        SharingCoordinator::new(&mut account, AssociationStore::new(dir.path())).unwrap();
    let asset = flow
        .upload(
            "artifact1",
            &path,
            "original.png",
            "image/png",
            cancelled(),
            progress(),
        )
        .unwrap();
    assert!(asset.share.is_none());
    server.state.lock().unwrap().deny_share = true;
    assert_eq!(
        flow.configure_share("artifact1", true, SharePatch::default())
            .err(),
        Some(Error::Account(AccountError::InvalidSession))
    );
    drop(flow);
    assert_eq!(account.me(), Err(AccountError::InvalidSession));
}
