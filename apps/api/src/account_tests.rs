//! Real PostgreSQL tests; only delivery and object storage are substituted.
use crate::{auth::AuthState, email::Mailer, sharing::SharingState, storage::ObjectStore};
use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
    response::Response,
};
use image::{ImageBuffer, Rgb};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    collections::HashMap,
    io::Cursor,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
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
    data: Mutex<HashMap<String, Vec<u8>>>,
    fail_delete: AtomicBool,
    during_get: Mutex<Option<(PgPool, Uuid)>>,
}
#[async_trait]
impl ObjectStore for Objects {
    async fn put(&self, key: &str, _: &str, bytes: Vec<u8>) -> Result<(), ()> {
        self.data.lock().unwrap().insert(key.into(), bytes);
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, ()> {
        let hook = self.during_get.lock().unwrap().take();
        if let Some((pool, id)) = hook {
            sqlx::query("UPDATE shares SET revoked_at=now() WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        self.data.lock().unwrap().get(key).cloned().ok_or(())
    }
    async fn delete(&self, key: &str) -> Result<(), ()> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(());
        }
        self.data.lock().unwrap().remove(key);
        Ok(())
    }
}

async fn database() -> (PgPool, PgPool, String) {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("Use a disposable local PostgreSQL TEST_DATABASE_URL");
    let mut parsed = reqwest::Url::parse(&url).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        "Tests require a local disposable server"
    );
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
        .max_connections(8)
        .connect(parsed.as_str())
        .await
        .unwrap();
    crate::MIGRATOR.run(&pool).await.unwrap();
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
fn app(pool: &PgPool, mail: Arc<Mail>, store: Arc<Objects>) -> (Router, SharingState) {
    let auth = AuthState::for_test(pool.clone(), mail, "https://captur.es");
    let sharing = SharingState::new(auth.clone(), Some(store));
    (crate::app_router(None, auth, sharing.clone()), sharing)
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
) -> Response {
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "https://captur.es");
    if let Some(token) = token {
        r = r.header("authorization", format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        r = r.header("cookie", cookie);
    }
    let body = if let Some(body) = body {
        r = r.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let mut r = r.body(body).unwrap();
    r.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:4567".parse::<SocketAddr>().unwrap()));
    app.clone().oneshot(r).await.unwrap()
}
async fn json_body(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn request_code(app: &Router, email: &str) -> String {
    let response = call(
        app,
        "POST",
        "/api/auth/email/request",
        None,
        None,
        Some(json!({"email":email})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    json_body(response).await["challengeId"]
        .as_str()
        .unwrap()
        .into()
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
    let response = verify(app, &id, &code, "bearer").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(body["user"]["id"].is_string());
    body["token"].as_str().unwrap().into()
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_otp_concurrency_limits_and_sessions() {
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let (app, _) = app(&pool, mail.clone(), Arc::new(Objects::default()));
    let id = request_code(&app, " Alice@Example.com ").await;
    let code = mail.sent.lock().unwrap()[0].1.clone();
    assert_eq!(mail.sent.lock().unwrap()[0].0, "alice@example.com");
    let wrong = if code == "AAAAAA" { "BBBBBB" } else { "AAAAAA" };
    let (first, second) = tokio::join!(
        verify(&app, &id, wrong, "bearer"),
        verify(&app, &id, wrong, "bearer")
    );
    assert_eq!(first.status(), StatusCode::BAD_REQUEST);
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let mut attempts = [
        json_body(first).await["attemptsRemaining"]
            .as_i64()
            .unwrap(),
        json_body(second).await["attemptsRemaining"]
            .as_i64()
            .unwrap(),
    ];
    attempts.sort();
    assert_eq!(attempts, [1, 2]);
    let last = verify(&app, &id, wrong, "bearer").await;
    assert_eq!(json_body(last).await["attemptsRemaining"], 0);
    assert_eq!(
        verify(&app, &id, &code, "bearer").await.status(),
        StatusCode::BAD_REQUEST
    );

    let replacement = request_code(&app, "alice@example.com").await;
    let new_code = mail.sent.lock().unwrap().last().unwrap().1.clone();
    let (a, b) = tokio::join!(
        verify(&app, &replacement, &new_code, "bearer"),
        verify(&app, &replacement, &new_code, "bearer")
    );
    let (success, failure) = if a.status() == StatusCode::OK {
        (a, b)
    } else {
        (b, a)
    };
    assert_eq!(failure.status(), StatusCode::BAD_REQUEST);
    let token = json_body(success).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
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
    assert_eq!(
        call(&app, "POST", "/api/auth/logout", Some(&token), None, None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", "/api/account/me", Some(&token), None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // One remaining issuance slot, two concurrent requests: exactly one email.
    let before = mail.sent.lock().unwrap().len();
    let (_a, _b) = tokio::join!(
        request_code(&app, "alice@example.com"),
        request_code(&app, "ALICE@example.com")
    );
    assert_eq!(mail.sent.lock().unwrap().len(), before + 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_email_challenges WHERE email='alice@example.com'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 3);

    let id = request_code(&app, "cookie@example.com").await;
    let code = mail.sent.lock().unwrap().last().unwrap().1.clone();
    let response = verify(&app, &id, &code, "cookie").await;
    let set_cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set_cookie.contains("HttpOnly")
            && set_cookie.contains("Secure")
            && set_cookie.contains("SameSite=Lax")
    );
    assert!(json_body(response).await.get("token").is_none());
    let cookie = set_cookie.split(';').next().unwrap();
    assert_eq!(
        call(&app, "GET", "/api/account/me", None, Some(cookie), None)
            .await
            .status(),
        StatusCode::OK
    );
    for origin in [None, Some("https://evil.example")] {
        let mut r = Request::post("/api/auth/logout").header("cookie", cookie);
        if let Some(origin) = origin {
            r = r.header("origin", origin);
        }
        assert_eq!(
            app.clone()
                .oneshot(r.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    sqlx::query("UPDATE users SET disabled_at=now() WHERE email='cookie@example.com'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", "/api/account/me", None, Some(cookie), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    mail.fail.store(true, Ordering::SeqCst);
    let failure = call(
        &app,
        "POST",
        "/api/auth/email/request",
        None,
        None,
        Some(json!({"email":"failure@example.com"})),
    )
    .await;
    assert_eq!(failure.status(), StatusCode::SERVICE_UNAVAILABLE);
    let consumed: bool = sqlx::query_scalar("SELECT consumed_at IS NOT NULL FROM auth_email_challenges WHERE email='failure@example.com'").fetch_one(&pool).await.unwrap();
    assert!(consumed);
    finish(admin, pool, &name).await;
}

fn image_bytes() -> Vec<u8> {
    let image = ImageBuffer::from_fn(7, 3, |x, y| Rgb([(x * 29) as u8, (y * 80) as u8, 123]));
    let mut data = Cursor::new(Vec::new());
    image.write_to(&mut data, image::ImageFormat::Png).unwrap();
    // Must be stripped rather than copied to the object store.
    data.get_mut()
        .extend_from_slice(b"private trailing metadata");
    data.into_inner()
}
async fn upload_image(app: &Router, token: &str) -> String {
    let r = Request::post("/api/uploads")
        .header("origin", "https://captur.es")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "image/png")
        .body(Body::from(image_bytes()))
        .unwrap();
    let response = app.clone().oneshot(r).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    json_body(response).await["id"].as_str().unwrap().into()
}
async fn share(
    app: &Router,
    token: &str,
    upload: &str,
    visibility: &str,
    password: Option<&str>,
) -> String {
    let response = call(
        app,
        "POST",
        &format!("/api/uploads/{upload}/shares"),
        Some(token),
        None,
        Some(json!({"visibility":visibility,"password":password})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    json_body(response).await["id"].as_str().unwrap().into()
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_upload_authorization_and_immediate_revocation() {
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let store = Arc::new(Objects::default());
    let (app, sharing) = app(&pool, mail.clone(), store.clone());
    let owner = login(&app, &mail, "owner@example.com").await;
    let other = login(&app, &mail, "other@example.com").await;
    let upload = upload_image(&app, &owner).await;
    let data = store.data.lock().unwrap().values().next().unwrap().clone();
    assert!(!data.windows(8).any(|v| v == b"private "));
    let decoded = image::load_from_memory(&data).unwrap().into_rgb8();
    assert_eq!(decoded.dimensions(), (7, 3));
    assert_eq!(decoded.get_pixel(5, 2).0, [145, 160, 123]);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/uploads/{upload}/media"),
            Some(&other),
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
            "DELETE",
            &format!("/api/uploads/{upload}"),
            Some(&other),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let private = share(&app, &owner, &upload, "private", None).await;
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/shares/{private}"),
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
            &format!("/api/shares/{private}/media"),
            Some(&other),
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
            &format!("/api/shares/{private}/media"),
            Some(&owner),
            None,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let unlisted = share(&app, &owner, &upload, "unlisted", None).await;
    let public = share(&app, &owner, &upload, "public", None).await;
    for (id, indexed) in [(&unlisted, false), (&public, true)] {
        let r = call(
            &app,
            "GET",
            &format!("/api/shares/{id}/media"),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["cache-control"], "private, no-store");
        assert_eq!(
            r.headers()["x-robots-tag"]
                .to_str()
                .unwrap()
                .starts_with("index"),
            indexed
        );
        assert!(!r.headers().contains_key("location"));
    }
    let protected = share(&app, &owner, &upload, "public", Some("a real password")).await;
    let path = format!("/api/shares/{protected}");
    let meta = call(&app, "GET", &path, None, None, None).await;
    assert!(
        meta.headers()["x-robots-tag"]
            .to_str()
            .unwrap()
            .contains("noindex")
    );
    assert!(json_body(meta).await["mediaUrl"].is_null());
    assert_eq!(
        call(&app, "GET", &format!("{path}/media"), None, None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/unlock"),
            None,
            None,
            Some(json!({"password":"wrong"}))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let unlock = call(
        &app,
        "POST",
        &format!("{path}/unlock"),
        None,
        None,
        Some(json!({"password":"a real password"})),
    )
    .await;
    assert_eq!(unlock.status(), StatusCode::NO_CONTENT);
    let cookie = unlock.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}/media"),
            None,
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let other_protected = share(&app, &owner, &upload, "unlisted", Some("a real password")).await;
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/shares/{other_protected}/media"),
            None,
            Some(&cookie),
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
            &format!("{path}/revoke"),
            Some(&other),
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
            "POST",
            &format!("{path}/revoke"),
            Some(&owner),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}/media"),
            None,
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    // A revocation while R2 is reading must not produce a newly authorized body.
    *store.during_get.lock().unwrap() = Some((pool.clone(), public.parse().unwrap()));
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/shares/{public}/media"),
            None,
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE shares SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(unlisted.parse::<Uuid>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/shares/{unlisted}/media"),
            None,
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    store.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/uploads/{upload}"),
            Some(&owner),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/shares/{private}/media"),
            Some(&owner),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert!(!store.data.lock().unwrap().is_empty());
    store.fail_delete.store(false, Ordering::SeqCst);
    sharing.cleanup().await.unwrap();
    assert!(store.data.lock().unwrap().is_empty());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM shares")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    finish(admin, pool, &name).await;
}

#[tokio::test]
#[ignore = "requires disposable local TEST_DATABASE_URL"]
async fn postgres_upload_limits_and_expired_credentials() {
    let (admin, pool, name) = database().await;
    let mail = Arc::new(Mail::default());
    let store = Arc::new(Objects::default());
    let (app, _) = app(&pool, mail.clone(), store.clone());
    let token = login(&app, &mail, "limit@example.com").await;
    let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email='limit@example.com'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let request = |data: Vec<u8>, content_type: &str| {
        Request::post("/api/uploads")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", content_type)
            .body(Body::from(data))
            .unwrap()
    };
    for (data, content_type, status) in [
        (image_bytes(), "image/jpeg", StatusCode::BAD_REQUEST),
        (b"<svg/>".to_vec(), "image/svg+xml", StatusCode::BAD_REQUEST),
        (
            vec![0; crate::storage::MAX_BYTES + 1],
            "image/png",
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(data, content_type))
                .await
                .unwrap()
                .status(),
            status
        );
    }
    assert!(store.data.lock().unwrap().is_empty());
    for _ in 0..99 {
        sqlx::query("INSERT INTO uploads(id,user_id,content_type,byte_size,state) VALUES($1,$2,'image/png',10,'pending')")
            .bind(Uuid::new_v4()).bind(user).execute(&pool).await.unwrap();
    }
    let (a, b) = tokio::join!(
        app.clone().oneshot(request(image_bytes(), "image/png")),
        app.clone().oneshot(request(image_bytes(), "image/png"))
    );
    let mut statuses = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM uploads WHERE user_id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 100);
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
    let id = request_code(&app, "expired@example.com").await;
    let code = mail.sent.lock().unwrap().last().unwrap().1.clone();
    sqlx::query(
        "UPDATE auth_email_challenges SET expires_at=now()-interval '1 second' WHERE id=$1",
    )
    .bind(id.parse::<Uuid>().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        verify(&app, &id, &code, "bearer").await.status(),
        StatusCode::BAD_REQUEST
    );
    finish(admin, pool, &name).await;
}
