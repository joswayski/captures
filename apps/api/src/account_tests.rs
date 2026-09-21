//! End-to-end account and sharing contracts against real PostgreSQL.
use crate::{auth::AuthState, email::Mailer, sharing::SharingState, storage::ObjectStore};
use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
    response::Response,
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Default)]
struct Mail {
    sent: Mutex<Vec<(String, String)>>,
    fail: AtomicBool,
}
#[async_trait]
impl Mailer for Mail {
    async fn send_code(&self, email: &str, code: &str) -> Result<(), ()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(());
        }
        self.sent.lock().unwrap().push((email.into(), code.into()));
        Ok(())
    }
}

#[derive(Default)]
struct Objects {
    complete: Mutex<HashMap<String, Vec<u8>>>,
    pending: Mutex<HashMap<String, (String, Vec<u8>)>>,
    created_keys: Mutex<Vec<String>>,
    complete_calls: AtomicUsize,
    fail_abort: AtomicBool,
    fail_head_once: AtomicBool,
}
impl Objects {
    fn stage(&self, asset: &str, bytes: &[u8]) {
        let key = self
            .created_keys
            .lock()
            .unwrap()
            .iter()
            .find(|key| key.ends_with(&format!("/{asset}")))
            .unwrap()
            .clone();
        self.pending
            .lock()
            .unwrap()
            .values_mut()
            .find(|x| x.0 == key)
            .unwrap()
            .1 = bytes.to_vec();
    }

    fn key_for(&self, asset: &str) -> String {
        self.created_keys
            .lock()
            .unwrap()
            .iter()
            .find(|key| key.ends_with(&format!("/{asset}")))
            .unwrap()
            .clone()
    }
}
#[async_trait]
impl ObjectStore for Objects {
    async fn create_multipart(&self, key: &str, _: &str) -> Result<String, ()> {
        self.created_keys.lock().unwrap().push(key.into());
        let id = format!("upload-{}", self.created_keys.lock().unwrap().len());
        self.pending
            .lock()
            .unwrap()
            .insert(id.clone(), (key.into(), Vec::new()));
        Ok(id)
    }
    async fn sign_part(
        &self,
        key: &str,
        upload: &str,
        part: i32,
    ) -> Result<(String, HashMap<String, String>), ()> {
        if self
            .pending
            .lock()
            .unwrap()
            .get(upload)
            .map(|x| x.0.as_str())
            != Some(key)
        {
            return Err(());
        }
        Ok((
            format!("https://storage.invalid/{upload}/{part}"),
            HashMap::new(),
        ))
    }
    async fn complete_multipart(
        &self,
        key: &str,
        upload: &str,
        _: Vec<(i32, String)>,
    ) -> Result<(), ()> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        let (pending_key, bytes) = self.pending.lock().unwrap().remove(upload).ok_or(())?;
        if pending_key != key {
            return Err(());
        }
        self.complete.lock().unwrap().insert(key.into(), bytes);
        Ok(())
    }
    async fn abort_multipart(&self, key: &str, upload: &str) -> Result<(), ()> {
        if self.fail_abort.load(Ordering::SeqCst) {
            return Err(());
        }
        let mut pending = self.pending.lock().unwrap();
        if pending.get(upload).is_some_and(|x| x.0 == key) {
            pending.remove(upload);
        }
        Ok(())
    }
    async fn head(&self, key: &str) -> Result<i64, ()> {
        if self.fail_head_once.swap(false, Ordering::SeqCst) {
            return Err(());
        }
        self.complete
            .lock()
            .unwrap()
            .get(key)
            .map(|x| x.len() as i64)
            .ok_or(())
    }
}

async fn database() -> (PgPool, PgPool, String) {
    let url = std::env::var("TEST_DATABASE_URL").expect("set disposable local TEST_DATABASE_URL");
    let mut parsed = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]")
    ));
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let name = format!("captures_accounts_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    parsed.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .connect(parsed.as_str())
        .await
        .unwrap();
    crate::MIGRATOR.run(&pool).await.unwrap();
    crate::backfill_user_external_ids(&pool).await.unwrap();
    (admin, pool, name)
}
async fn finish(admin: PgPool, pool: PgPool, name: &str) {
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
const MEDIA_WORKER_SECRET: &str = "stable-account-test-media-worker-secret";
fn app(pool: &PgPool, mail: Arc<Mail>, store: Arc<Objects>) -> (Router, SharingState) {
    let auth = AuthState::for_test(pool.clone(), mail, "https://captur.es");
    let mut sharing = SharingState::new(auth.clone(), Some(store));
    sharing.media_worker_secret = Some(MEDIA_WORKER_SECRET.into());
    (crate::app_router(None, auth, sharing.clone()), sharing)
}
async fn call_with_media_key(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
    media_key: Option<&str>,
) -> Response {
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "https://captur.es");
    if let Some(x) = token {
        r = r.header("authorization", format!("Bearer {x}"));
    }
    if let Some(x) = cookie {
        r = r.header("cookie", x);
    }
    if let Some(x) = media_key {
        r = r.header("x-captures-media-key", x);
    }
    let body = if let Some(x) = body {
        r = r.header("content-type", "application/json");
        Body::from(x.to_string())
    } else {
        Body::empty()
    };
    let mut r = r.body(body).unwrap();
    r.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:4567".parse::<SocketAddr>().unwrap()));
    app.clone().oneshot(r).await.unwrap()
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
) -> Response {
    let media_key = path
        .starts_with("/api/media/")
        .then_some(MEDIA_WORKER_SECRET);
    call_with_media_key(app, method, path, token, cookie, body, media_key).await
}
async fn body(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn request_code(app: &Router, email: &str) -> String {
    let r = call(
        app,
        "POST",
        "/api/auth/email/request",
        None,
        None,
        Some(json!({"email":email})),
    )
    .await;
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    body(r).await["challengeId"].as_str().unwrap().into()
}
async fn verify(app: &Router, id: &str, code: &str, transport: &str) -> Response {
    call(
        app,
        "POST",
        "/api/auth/email/verify",
        None,
        None,
        Some(json!({"challengeId":id,"code":code,"transport":transport})),
    )
    .await
}
async fn login(app: &Router, mail: &Mail, email: &str) -> String {
    let id = request_code(app, email).await;
    let code = mail.sent.lock().unwrap().last().unwrap().1.clone();
    let r = verify(app, &id, &code, "bearer").await;
    assert_eq!(r.status(), StatusCode::OK);
    body(r).await["token"].as_str().unwrap().into()
}
async fn create_asset(
    app: &Router,
    store: &Objects,
    token: &str,
    name: &str,
    kind: &str,
    bytes: &[u8],
) -> String {
    let r = call(
        app,
        "POST",
        "/api/assets",
        Some(token),
        None,
        Some(json!({"name":name,"contentType":kind,"byteSize":bytes.len()})),
    )
    .await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let data = body(r).await;
    let id = data["id"].as_str().unwrap().to_owned();
    assert_eq!(data["partCount"], 1);
    assert!(
        store
            .created_keys
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .ends_with(&format!("/{id}"))
    );
    store.stage(&id, bytes);
    let part = call(
        app,
        "POST",
        &format!("/api/assets/{id}/parts"),
        Some(token),
        None,
        Some(json!({"partNumber":1})),
    )
    .await;
    assert_eq!(part.status(), StatusCode::OK);
    let done = call(
        app,
        "POST",
        &format!("/api/assets/{id}/complete"),
        Some(token),
        None,
        Some(json!({"parts":[{"partNumber":1,"etag":"etag"}]})),
    )
    .await;
    assert_eq!(done.status(), StatusCode::OK);
    id
}
async fn set_share(app: &Router, token: &str, asset: &str, patch: Value) -> Response {
    call(
        app,
        "PUT",
        &format!("/api/assets/{asset}/share"),
        Some(token),
        None,
        Some(patch),
    )
    .await
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_otp_concurrency_expiry_and_sessions() {
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let (app, _) = app(&pool, mail.clone(), Arc::new(Objects::default()));
    let id = request_code(&app, " Alice@Example.com ").await;
    let code = mail.sent.lock().unwrap()[0].1.clone();
    assert_eq!(mail.sent.lock().unwrap()[0].0, "alice@example.com");
    let wrong = if code == "AAAAAA" { "BBBBBB" } else { "AAAAAA" };
    let (a, b) = tokio::join!(
        verify(&app, &id, wrong, "bearer"),
        verify(&app, &id, wrong, "bearer")
    );
    let mut remaining = [
        body(a).await["attemptsRemaining"].as_i64().unwrap(),
        body(b).await["attemptsRemaining"].as_i64().unwrap(),
    ];
    remaining.sort();
    assert_eq!(remaining, [1, 2]);
    assert_eq!(
        body(verify(&app, &id, wrong, "bearer").await).await["attemptsRemaining"],
        0
    );
    assert_eq!(
        verify(&app, &id, &code, "bearer").await.status(),
        StatusCode::BAD_REQUEST
    );

    let id = request_code(&app, "alice@example.com").await;
    let code = mail.sent.lock().unwrap().last().unwrap().1.clone();
    let (a, b) = tokio::join!(
        verify(&app, &id, &code, "bearer"),
        verify(&app, &id, &code, "bearer")
    );
    let (ok, rejected) = if a.status() == StatusCode::OK {
        (a, b)
    } else {
        (b, a)
    };
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let token = body(ok).await["token"].as_str().unwrap().to_owned();
    assert_eq!(
        call(&app, "GET", "/api/account/me", Some(&token), None, None)
            .await
            .status(),
        StatusCode::OK
    );
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM account_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert_ne!(stored, token.as_bytes());
    sqlx::query("UPDATE account_sessions SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", "/api/account/me", Some(&token), None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let expired = request_code(&app, "expired@example.com").await;
    let expired_code = mail.sent.lock().unwrap().last().unwrap().1.clone();
    sqlx::query(
        "UPDATE auth_email_challenges SET expires_at=now()-interval '1 second' WHERE id=$1",
    )
    .bind(&expired)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        verify(&app, &expired, &expired_code, "bearer")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    finish(admin, pool, &name).await;
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_asset_multipart_lifecycle_media_authorization_and_cleanup() {
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let store = Arc::new(Objects::default());
    let (app, sharing) = app(&pool, mail.clone(), store.clone());
    let owner = login(&app, &mail, "owner@example.com").await;
    let other = login(&app, &mail, "other@example.com").await;

    let pending = call(
        &app,
        "POST",
        "/api/assets",
        Some(&owner),
        None,
        Some(json!({"name":"pending.gif","contentType":"image/gif","byteSize":6})),
    )
    .await;
    let pending = body(pending).await["id"].as_str().unwrap().to_owned();
    store.stage(&pending, b"GIF89a");
    for path in [
        format!("/api/media/assets/{pending}"),
        format!("/api/assets/{pending}/share"),
    ] {
        let method = if path.ends_with("share") {
            "PUT"
        } else {
            "GET"
        };
        let payload = (method == "PUT").then(|| json!({"enabled":true}));
        assert_eq!(
            call(&app, method, &path, Some(&owner), None, payload)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/assets/{pending}/parts"),
            Some(&other),
            None,
            Some(json!({"partNumber":1}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/assets/{pending}/complete"),
            Some(&other),
            None,
            Some(json!({"parts":[{"partNumber":1,"etag":"x"}]}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/assets/{pending}/complete"),
            Some(&owner),
            None,
            Some(json!({"parts":[]}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/assets/{pending}/complete"),
            Some(&owner),
            None,
            Some(json!({"parts":[{"partNumber":2,"etag":"x"}]}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    store.stage(&pending, b"wrong");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/assets/{pending}/complete"),
            Some(&owner),
            None,
            Some(json!({"parts":[{"partNumber":1,"etag":"x"}]}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let state: String = sqlx::query_scalar("SELECT state FROM assets WHERE external_id=$1")
        .bind(&pending)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "pending");

    store.fail_head_once.store(true, Ordering::SeqCst);
    let gif = create_asset(
        &app,
        &store,
        &owner,
        "clip.gif",
        "image/gif",
        b"GIF89a-exact",
    )
    .await;
    let video = create_asset(
        &app,
        &store,
        &owner,
        "clip.webm",
        "video/webm",
        b"video-exact",
    )
    .await;
    assert_eq!(
        store.complete.lock().unwrap()[&store.key_for(&gif)],
        b"GIF89a-exact"
    );
    assert_eq!(
        store.complete.lock().unwrap()[&store.key_for(&video)],
        b"video-exact"
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/media/assets/{gif}"),
            Some(&other),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let media = call(
        &app,
        "GET",
        &format!("/api/media/assets/{video}"),
        Some(&owner),
        None,
        None,
    )
    .await;
    assert_eq!(media.status(), StatusCode::OK);
    assert_eq!(
        body(media).await,
        json!({
            "key": store.key_for(&video),
            "contentType": "video/webm",
            "name": "clip.webm",
            "byteSize": 11
        })
    );
    for media_key in [None, Some("wrong-worker-secret")] {
        assert_eq!(
            call_with_media_key(
                &app,
                "GET",
                &format!("/api/media/assets/{video}"),
                Some(&owner),
                None,
                None,
                media_key,
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call_with_media_key(
            &app,
            "GET",
            &format!("/api/media/assets/{video}"),
            None,
            None,
            None,
            Some(MEDIA_WORKER_SECRET),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/assets/{video}/media"),
            Some(&owner),
            None,
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/assets/{gif}"),
            Some(&other),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    // Metadata alone may describe large multipart objects without allocating them.
    let large = call(&app, "POST", "/api/assets", Some(&owner), None, Some(json!({"name":"large.bin","contentType":"application/octet-stream","byteSize":70_i64*1024*1024}))).await;
    assert_eq!(large.status(), StatusCode::CREATED);
    assert_eq!(body(large).await["partCount"], 2);

    let calls = store.complete_calls.load(Ordering::SeqCst);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/assets/{video}/complete"),
            Some(&owner),
            None,
            Some(json!({"parts":[{"partNumber":1,"etag":"ignored"}]}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(store.complete_calls.load(Ordering::SeqCst), calls);

    store.fail_abort.store(true, Ordering::SeqCst);
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/assets/{pending}"),
            Some(&owner),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    sharing.cleanup().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM assets WHERE external_id=$1 AND state='cancelled' AND multipart_upload_id IS NOT NULL")
            .bind(&pending)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    store.fail_abort.store(false, Ordering::SeqCst);
    sharing.cleanup().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM assets WHERE external_id=$1 AND state='cancelled' AND deleted_at IS NOT NULL AND multipart_upload_id IS NULL")
            .bind(&pending)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    sharing.cleanup().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM assets WHERE external_id=$1 AND state='cancelled'"
        )
        .bind(&pending)
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    finish(admin, pool, &name).await;
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_share_updates_passwords_expiry_and_revocation() {
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let store = Arc::new(Objects::default());
    let (app, _) = app(&pool, mail.clone(), store.clone());
    let owner = login(&app, &mail, "share@example.com").await;
    let asset = create_asset(
        &app,
        &store,
        &owner,
        "unsafe.svg",
        "image/svg+xml",
        b"<svg><script>x</script></svg>",
    )
    .await;
    let expiry = "2099-01-01T00:00:00Z";
    let enabled = set_share(
        &app,
        &owner,
        &asset,
        json!({"enabled":true,"password":"password one","expiresAt":expiry}),
    )
    .await;
    assert_eq!(enabled.status(), StatusCode::OK);
    let sid = body(enabled).await["share"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (unchanged, edited) = tokio::join!(
        set_share(&app, &owner, &asset, json!({"enabled":true})),
        set_share(
            &app,
            &owner,
            &asset,
            json!({"enabled":true,"expiresAt":"2099-02-01T00:00:00Z"})
        )
    );
    assert_eq!(body(unchanged).await["share"]["id"], sid);
    assert_eq!(body(edited).await["share"]["id"], sid);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM shares s JOIN assets a ON a.id=s.asset_id WHERE a.external_id=$1 AND s.active")
            .bind(&asset)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let unlock = call(
        &app,
        "POST",
        &format!("/api/shares/{sid}/unlock"),
        None,
        None,
        Some(json!({"password":"password one"})),
    )
    .await;
    assert_eq!(unlock.status(), StatusCode::NO_CONTENT);
    let set_cookie = unlock.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(!set_cookie.to_ascii_lowercase().contains("max-age"));
    let cookie = set_cookie.split(';').next().unwrap();
    let metadata = body(
        call(
            &app,
            "GET",
            &format!("/api/shares/{sid}"),
            None,
            Some(cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(metadata["mediaUrl"], format!("/api/files/shares/{sid}"));
    let media = call(
        &app,
        "GET",
        &format!("/api/media/shares/{sid}"),
        None,
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(media.status(), StatusCode::OK);
    assert_eq!(
        body(media).await,
        json!({
            "key": store.key_for(&asset),
            "contentType": "image/svg+xml",
            "name": "unsafe.svg",
            "byteSize": 29
        })
    );
    for media_key in [None, Some("wrong-worker-secret")] {
        assert_eq!(
            call_with_media_key(
                &app,
                "GET",
                &format!("/api/media/shares/{sid}"),
                None,
                Some(cookie),
                None,
                media_key,
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call_with_media_key(
            &app,
            "GET",
            &format!("/api/media/shares/{sid}"),
            None,
            None,
            None,
            Some(MEDIA_WORKER_SECRET),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/shares/{sid}/media"),
            None,
            Some(cookie),
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        set_share(
            &app,
            &owner,
            &asset,
            json!({"enabled":true,"password":"password two"})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/media/shares/{sid}"),
            None,
            Some(cookie),
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/shares/{sid}/unlock"),
            None,
            None,
            Some(json!({"password":"password one"}))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let unlock = call(
        &app,
        "POST",
        &format!("/api/shares/{sid}/unlock"),
        None,
        None,
        Some(json!({"password":"password two"})),
    )
    .await;
    let cookie2 = unlock.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        set_share(
            &app,
            &owner,
            &asset,
            json!({"enabled":true,"password":null,"expiresAt":null})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/media/shares/{sid}"),
            None,
            None,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );

    sqlx::query("UPDATE shares SET active=false WHERE id=$1")
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/media/shares/{sid}"),
            None,
            Some(&cookie2),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE shares SET active=true WHERE id=$1")
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        set_share(&app, &owner, &asset, json!({"enabled":false}))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", &format!("/api/shares/{sid}"), None, None, None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let again = set_share(&app, &owner, &asset, json!({"enabled":true})).await;
    let sid2 = body(again).await["share"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(sid, sid2);
    sqlx::query("UPDATE shares SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(&sid2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/shares/{sid2}"),
            None,
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/media/shares/{sid2}"),
            None,
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    finish(admin, pool, &name).await;
}

static COLLISION_CALLS: AtomicUsize = AtomicUsize::new(0);
fn collision_ids() -> String {
    match COLLISION_CALLS.fetch_add(1, Ordering::SeqCst) {
        0 => "collision001".into(),
        1 => "freshasset01".into(),
        2 => "sharesame001".into(),
        _ => "freshshare01".into(),
    }
}
#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_asset_and_share_id_collisions_preserve_existing_rows() {
    COLLISION_CALLS.store(0, Ordering::SeqCst);
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let store = Arc::new(Objects::default());
    let (app0, mut sharing) = app(&pool, mail.clone(), store.clone());
    let token = login(&app0, &mail, "collision@example.com").await;
    let user: i64 = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO assets(external_id,user_id,storage_key,name,content_type,byte_size,state) VALUES('collision001',$1,'assets/collision001','existing','image/png',1,'ready')").bind(user).execute(&pool).await.unwrap();
    store
        .complete
        .lock()
        .unwrap()
        .insert("assets/collision001".into(), vec![1]);
    sharing.new_id = collision_ids;
    let app = crate::app_router(None, sharing.auth.clone(), sharing.clone());
    let asset = create_asset(&app, &store, &token, "new", "image/png", b"n").await;
    assert_eq!(asset, "freshasset01");
    let existing: String =
        sqlx::query_scalar("SELECT name FROM assets WHERE external_id='collision001'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(existing, "existing");
    sqlx::query("INSERT INTO shares(id,asset_id) SELECT 'sharesame001',id FROM assets WHERE external_id='collision001'")
        .execute(&pool)
        .await
        .unwrap();
    let share = set_share(&app, &token, &asset, json!({"enabled":true})).await;
    assert_eq!(body(share).await["share"]["id"], "freshshare01");
    let old_asset: String =
        sqlx::query_scalar("SELECT a.external_id FROM shares s JOIN assets a ON a.id=s.asset_id WHERE s.id='sharesame001'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old_asset, "collision001");
    assert_eq!(COLLISION_CALLS.load(Ordering::SeqCst), 4);
    finish(admin, pool, &name).await;
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_trash_retains_bytes_restores_privately_and_audits_password_attempts() {
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let store = Arc::new(Objects::default());
    let (app, sharing) = app(&pool, mail.clone(), store.clone());
    let owner = login(&app, &mail, "trash@example.com").await;
    let other = login(&app, &mail, "other@example.com").await;
    let me = body(call(&app, "GET", "/api/account/me", Some(&owner), None, None).await).await;
    let external_user = me["user"]["id"].as_str().unwrap();
    assert_eq!(external_user.len(), 12);
    assert!(me["user"].get("internal_id").is_none());
    let id = create_asset(
        &app,
        &store,
        &owner,
        "kept.gif",
        "image/gif",
        b"GIF89a-retained",
    )
    .await;
    let expected_key = format!("assets/{external_user}/{id}");
    assert_eq!(store.key_for(&id), expected_key);
    let internal: (i64, i64) = sqlx::query_as("SELECT a.id,u.id FROM assets a JOIN users u ON u.id=a.user_id WHERE a.external_id=$1 AND u.external_id=$2")
        .bind(&id).bind(external_user).fetch_one(&pool).await.unwrap();
    assert!(internal.0 > 0 && internal.1 > 0);
    let share = body(
        set_share(
            &app,
            &owner,
            &id,
            json!({"enabled":true,"password":"right-password"}),
        )
        .await,
    )
    .await;
    let sid = share["share"]["id"].as_str().unwrap();
    let mut viewer_cookie = String::new();
    for (password, status) in [
        ("wrong-password", StatusCode::UNAUTHORIZED),
        ("right-password", StatusCode::NO_CONTENT),
    ] {
        let mut request = Request::post(format!("/api/shares/{sid}/unlock"))
            .header("origin", "https://captur.es")
            .header("content-type", "application/json")
            .header("user-agent", "audit-test-browser")
            .header("cf-connecting-ip", "203.0.113.8")
            .body(Body::from(json!({"password":password}).to_string()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "198.51.100.9:4567".parse::<SocketAddr>().unwrap(),
        ));
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::NO_CONTENT {
            viewer_cookie = response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
        }
    }
    let audit: Vec<(String,String,String,i64)> = sqlx::query_as("SELECT host(source_ip),user_agent,outcome,octet_length(ip_hash)::bigint FROM share_unlock_attempts WHERE share_id=$1 ORDER BY id")
        .bind(sid).fetch_all(&pool).await.unwrap();
    assert_eq!(
        audit,
        vec![
            (
                "198.51.100.9".into(),
                "audit-test-browser".into(),
                "denied".into(),
                32
            ),
            (
                "198.51.100.9".into(),
                "audit-test-browser".into(),
                "granted".into(),
                32
            )
        ]
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/assets/{id}"),
            Some(&owner),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        body(call(&app, "GET", "/api/assets", Some(&owner), None, None).await).await["assets"],
        json!([])
    );
    let trash = body(
        call(
            &app,
            "GET",
            "/api/assets?deleted=true",
            Some(&owner),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(trash["assets"][0]["id"], id);
    assert!(trash["assets"][0]["deletedAt"].is_string());
    assert!(trash["assets"][0]["share"].is_null());
    assert!(trash["assets"][0].get("internalId").is_none());
    assert_eq!(
        body(
            call(
                &app,
                "GET",
                "/api/assets?deleted=true",
                Some(&other),
                None,
                None
            )
            .await
        )
        .await["assets"],
        json!([])
    );
    for path in [
        format!("/api/media/assets/{id}"),
        format!("/api/media/shares/{sid}"),
        format!("/api/shares/{sid}"),
    ] {
        assert_eq!(
            call(&app, "GET", &path, Some(&owner), Some(&viewer_cookie), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        set_share(&app, &owner, &id, json!({"enabled":true}))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/assets/{id}/restore"),
            Some(&other),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    sharing.cleanup().await.unwrap();
    assert_eq!(
        store.complete.lock().unwrap()[&expected_key],
        b"GIF89a-retained"
    );
    let restored = body(
        call(
            &app,
            "POST",
            &format!("/api/assets/{id}/restore"),
            Some(&owner),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(restored["id"], id);
    assert!(restored["deletedAt"].is_null() && restored["share"].is_null());
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/media/assets/{id}"),
            Some(&owner),
            None,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/media/shares/{sid}"),
            None,
            Some(&viewer_cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let replacement = body(set_share(&app, &owner, &id, json!({"enabled":true})).await).await;
    assert_ne!(replacement["share"]["id"], sid);
    sqlx::query("UPDATE share_unlock_attempts SET created_at=now()-interval '30 days'")
        .execute(&pool)
        .await
        .unwrap();
    sharing.cleanup().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM share_unlock_attempts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    finish(admin, pool, &name).await;
}

static USER_COLLISION_CALLS: AtomicUsize = AtomicUsize::new(0);
fn user_collision_ids() -> String {
    if USER_COLLISION_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
        "occupied0001"
    } else {
        "freshuser001"
    }
    .into()
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_user_external_id_collision_and_existing_email() {
    USER_COLLISION_CALLS.store(0, Ordering::SeqCst);
    let (admin, pool, name) = database().await;
    sqlx::query(
        "INSERT INTO users(external_id,email) VALUES('occupied0001','existing@example.com')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mail = Arc::new(Mail::default());
    let (_, mut sharing) = app(&pool, mail.clone(), Arc::new(Objects::default()));
    sharing.auth.new_user_id = user_collision_ids;
    let app = crate::app_router(None, sharing.auth.clone(), sharing);
    let token = login(&app, &mail, "new@example.com").await;
    assert_eq!(USER_COLLISION_CALLS.load(Ordering::SeqCst), 2);
    let user = body(call(&app, "GET", "/api/account/me", Some(&token), None, None).await).await;
    assert_eq!(
        user["user"],
        json!({"id":"freshuser001","email":"new@example.com"})
    );
    let again = login(&app, &mail, "new@example.com").await;
    assert_eq!(
        body(call(&app, "GET", "/api/account/me", Some(&again), None, None).await).await,
        user
    );
    sqlx::query("UPDATE users SET disabled_at=now() WHERE email='new@example.com'")
        .execute(&pool)
        .await
        .unwrap();
    let challenge = request_code(&app, "new@example.com").await;
    let code = mail.sent.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(
        verify(&app, &challenge, &code, "bearer").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    finish(admin, pool, &name).await;
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_forward_migration_preserves_legacy_assets_and_backfills_users() {
    let (admin, pool, name) = database().await;
    // This is the unique disposable database created above, never a shared schema.
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(&pool)
        .await
        .unwrap();
    for sql in [
        include_str!("../migrations/0001_accounts.sql"),
        include_str!("../migrations/0002_email_auth.sql"),
        include_str!("../migrations/0003_sharing.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("INSERT INTO users(email) VALUES('legacy@example.com'); INSERT INTO assets(id,user_id,name,content_type,byte_size,state) SELECT 'legacyfile01',id,'old.gif','image/gif',7,'ready' FROM users; INSERT INTO shares(id,asset_id) VALUES('legacyshare1','legacyfile01');").execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/0004_external_ids_soft_delete_audit.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let (a, b) = tokio::join!(
        crate::backfill_user_external_ids(&pool),
        crate::backfill_user_external_ids(&pool)
    );
    a.unwrap();
    b.unwrap();
    let row:(i64,String,String,String)=sqlx::query_as("SELECT a.id,a.external_id,a.storage_key,u.external_id FROM shares s JOIN assets a ON a.id=s.asset_id JOIN users u ON u.id=a.user_id WHERE s.id='legacyshare1'").fetch_one(&pool).await.unwrap();
    assert!(row.0 > 0);
    assert_eq!(row.1, "legacyfile01");
    assert_eq!(row.2, "assets/legacyfile01");
    assert_eq!(row.3.len(), 12);
    crate::backfill_user_external_ids(&pool).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT external_id FROM users")
            .fetch_one(&pool)
            .await
            .unwrap(),
        row.3
    );
    assert!(
        sqlx::query("INSERT INTO users(email) VALUES('missing-id@example.com')")
            .execute(&pool)
            .await
            .is_err()
    );
    finish(admin, pool, &name).await;
}
