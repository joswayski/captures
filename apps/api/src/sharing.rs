use crate::{
    auth::{AuthError, AuthState},
    storage::{ObjectStore, multipart_shape},
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct SharingState {
    pub auth: AuthState,
    pub store: Option<Arc<dyn ObjectStore>>,
    pub media_worker_secret: Option<String>,
    work: Arc<Semaphore>,
    pub(crate) new_id: fn() -> String,
}
impl SharingState {
    pub fn new(auth: AuthState, store: Option<Arc<dyn ObjectStore>>) -> Self {
        Self {
            auth,
            store,
            media_worker_secret: None,
            work: Arc::new(Semaphore::new(2)),
            new_id: || nanoid::nanoid!(12),
        }
    }
    fn store(&self) -> Result<&dyn ObjectStore, ShareError> {
        self.store.as_deref().ok_or_else(unavailable)
    }
    pub async fn cleanup(&self) -> Result<(), ShareError> {
        let store = self.store()?;
        sqlx::query("UPDATE assets SET state='cancelled',deleted_at=now() WHERE state='pending' AND created_at<now()-interval '7 days'")
            .execute(&self.auth.pool).await?;
        let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
            "SELECT id,storage_key,multipart_upload_id FROM assets WHERE state='cancelled' AND multipart_upload_id IS NOT NULL LIMIT 100",
        )
        .fetch_all(&self.auth.pool)
        .await?;
        for (id, storage_key, upload) in rows {
            if let Some(u) = upload
                && store.abort_multipart(&storage_key, &u).await.is_err()
            {
                continue;
            }
            sqlx::query(
                "UPDATE assets SET multipart_upload_id=NULL WHERE id=$1 AND state='cancelled'",
            )
            .bind(id)
            .execute(&self.auth.pool)
            .await?;
        }
        Ok(())
    }
}
pub fn router(s: SharingState) -> Router {
    Router::new()
        .route("/api/assets", get(list).post(create))
        .route("/api/assets/{id}", delete(remove))
        .route("/api/assets/{id}/restore", post(restore))
        .route("/api/assets/{id}/parts", post(part))
        .route("/api/assets/{id}/complete", post(complete))
        .route("/api/media/assets/{id}", get(owner_media))
        .route("/api/assets/{id}/share", put(update_share))
        .route("/api/shares/{id}", get(metadata))
        .route("/api/media/shares/{id}", get(shared_media))
        .route("/api/shares/{id}/unlock", post(unlock))
        .with_state(s)
        .layer(middleware::map_response(headers))
}
async fn headers(mut r: Response) -> Response {
    for (k, v) in [
        (header::CACHE_CONTROL, "private, no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        r.headers_mut().insert(k, HeaderValue::from_static(v));
    }
    r.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    for name in ["cdn-cache-control", "cloudflare-cdn-cache-control"] {
        r.headers_mut()
            .insert(name, HeaderValue::from_static("no-store"));
    }
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    r
}
#[derive(Debug)]
pub struct ShareError(StatusCode, &'static str);
impl IntoResponse for ShareError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<sqlx::Error> for ShareError {
    fn from(_: sqlx::Error) -> Self {
        tracing::error!("sharing database operation failed");
        unavailable()
    }
}
impl From<AuthError> for ShareError {
    fn from(e: AuthError) -> Self {
        Self(e.into_response().status(), "Request not allowed")
    }
}
fn unavailable() -> ShareError {
    ShareError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Sharing is temporarily unavailable",
    )
}
fn missing() -> ShareError {
    ShareError(StatusCode::NOT_FOUND, "Asset or share not available")
}
fn bad(s: &'static str) -> ShareError {
    ShareError(StatusCode::BAD_REQUEST, s)
}
fn key(user: &str, asset: &str) -> String {
    format!("assets/{user}/{asset}")
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Share {
    id: String,
    password_protected: bool,
    expires_at: Option<DateTime<Utc>>,
    shared_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Asset {
    #[serde(skip)]
    internal_id: i64,
    #[serde(rename = "id")]
    external_id: String,
    name: String,
    content_type: String,
    byte_size: i64,
    created_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
    #[sqlx(skip)]
    share: Option<Share>,
}
async fn asset(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<Asset, ShareError> {
    let mut a:Asset=sqlx::query_as("SELECT id internal_id,external_id,name,content_type,byte_size,created_at,deleted_at FROM assets WHERE external_id=$1 AND state='ready'").bind(id).fetch_one(&mut **tx).await?;
    a.share=sqlx::query_as("SELECT id,password_hash IS NOT NULL password_protected,expires_at,shared_at FROM shares WHERE asset_id=$1 AND active").bind(a.internal_id).fetch_optional(&mut **tx).await?;
    Ok(a)
}
async fn list(
    State(s): State<SharingState>,
    h: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, ShareError> {
    let u = s.auth.authenticate(&h).await?;
    s.store()?;
    let deleted = q.get("deleted").is_some_and(|v| v == "true");
    let mut rows:Vec<Asset>=sqlx::query_as("SELECT id internal_id,external_id,name,content_type,byte_size,created_at,deleted_at FROM assets WHERE user_id=$1 AND state='ready' AND (($2 AND deleted_at IS NOT NULL) OR (NOT $2 AND deleted_at IS NULL)) ORDER BY created_at DESC").bind(u.internal_id).bind(deleted).fetch_all(&s.auth.pool).await?;
    for a in &mut rows {
        a.share=sqlx::query_as("SELECT id,password_hash IS NOT NULL password_protected,expires_at,shared_at FROM shares WHERE asset_id=$1 AND active").bind(a.internal_id).fetch_optional(&s.auth.pool).await?;
    }
    Ok(Json(json!({"assets":rows})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewAsset {
    name: String,
    content_type: String,
    byte_size: i64,
}
async fn create(
    State(s): State<SharingState>,
    h: HeaderMap,
    Json(b): Json<NewAsset>,
) -> Result<impl IntoResponse, ShareError> {
    s.auth.check_mutation(&h)?;
    let u = s.auth.authenticate(&h).await?;
    if b.name.trim().is_empty() || b.name.len() > 1024 || b.content_type.len() > 255 {
        return Err(bad("Invalid asset metadata"));
    }
    let (size, count) =
        multipart_shape(b.byte_size).map_err(|_| bad("Object exceeds storage protocol bounds"))?;
    let store = s.store()?;
    for _ in 0..8 {
        let id = (s.new_id)();
        let storage_key = key(&u.external_id, &id);
        let mut tx = s.auth.pool.begin().await?;
        let result=sqlx::query("INSERT INTO assets(external_id,user_id,storage_key,name,content_type,byte_size,state) VALUES($1,$2,$3,$4,$5,$6,'pending') ON CONFLICT(external_id) DO NOTHING").bind(&id).bind(u.internal_id).bind(&storage_key).bind(&b.name).bind(&b.content_type).bind(b.byte_size).execute(&mut *tx).await?;
        if result.rows_affected() == 0 {
            tracing::warn!(kind = "asset_id", "public id collision; regenerating");
            continue;
        }
        // Reserve in PostgreSQL before touching storage. A failed transaction's
        // multipart upload is collected by the bucket's incomplete-upload policy.
        let upload = store
            .create_multipart(&storage_key, &b.content_type)
            .await
            .map_err(|_| unavailable())?;
        sqlx::query("UPDATE assets SET multipart_upload_id=$2 WHERE external_id=$1")
            .bind(&id)
            .bind(upload)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok((
            StatusCode::CREATED,
            Json(json!({"id":id,"partSize":size,"partCount":count})),
        ));
    }
    Err(unavailable())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartReq {
    part_number: i32,
}
async fn part(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(b): Json<PartReq>,
) -> Result<Json<Value>, ShareError> {
    s.auth.check_mutation(&h)?;
    let u = s.auth.authenticate(&h).await?;
    let row:Option<(String,i64,String)>=sqlx::query_as("SELECT multipart_upload_id,byte_size,storage_key FROM assets WHERE external_id=$1 AND user_id=$2 AND state='pending' AND deleted_at IS NULL").bind(&id).bind(u.internal_id).fetch_optional(&s.auth.pool).await?;
    let (upload, bytes, storage_key) = row.ok_or_else(missing)?;
    let (_, count) = multipart_shape(bytes).map_err(|_| bad("Invalid upload"))?;
    if b.part_number < 1 || b.part_number > count {
        return Err(bad("Invalid part number"));
    }
    let (url, headers) = s
        .store()?
        .sign_part(&storage_key, &upload, b.part_number)
        .await
        .map_err(|_| unavailable())?;
    Ok(Json(json!({"url":url,"headers":headers})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteReq {
    parts: Vec<Completed>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Completed {
    part_number: i32,
    etag: String,
}
async fn complete(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(b): Json<CompleteReq>,
) -> Result<Json<Asset>, ShareError> {
    s.auth.check_mutation(&h)?;
    let u = s.auth.authenticate(&h).await?;
    let mut tx = s.auth.pool.begin().await?;
    let row:Option<(String,String,i64,String)>=sqlx::query_as("SELECT state,COALESCE(multipart_upload_id,''),byte_size,storage_key FROM assets WHERE external_id=$1 AND user_id=$2 AND deleted_at IS NULL FOR UPDATE").bind(&id).bind(u.internal_id).fetch_optional(&mut *tx).await?;
    let (state, upload, bytes, storage_key) = row.ok_or_else(missing)?;
    if state == "ready" {
        return Ok(Json(asset(&mut tx, &id).await?));
    }
    if state != "pending" {
        return Err(missing());
    }
    let (_, count) = multipart_shape(bytes).map_err(|_| bad("Invalid upload"))?;
    if b.parts.len() != count as usize
        || b.parts
            .iter()
            .enumerate()
            .any(|(i, p)| p.part_number != i as i32 + 1 || p.etag.is_empty())
    {
        return Err(bad("All upload parts are required in order"));
    }
    let store = s.store()?;
    if store.head(&storage_key).await.ok() != Some(bytes) {
        store
            .complete_multipart(
                &storage_key,
                &upload,
                b.parts
                    .into_iter()
                    .map(|p| (p.part_number, p.etag))
                    .collect(),
            )
            .await
            .map_err(|_| unavailable())?;
    }
    if store.head(&storage_key).await.map_err(|_| unavailable())? != bytes {
        return Err(bad("Finalized object size does not match"));
    }
    sqlx::query("UPDATE assets SET state='ready',multipart_upload_id=NULL WHERE external_id=$1")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    let a = asset(&mut tx, &id).await?;
    tx.commit().await?;
    Ok(Json(a))
}
async fn remove(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<StatusCode, ShareError> {
    s.auth.check_mutation(&h)?;
    let u = s.auth.authenticate(&h).await?;
    let mut tx = s.auth.pool.begin().await?;
    s.store()?;
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT id,state FROM assets WHERE external_id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(&id)
    .bind(u.internal_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (internal_id, state) = row.ok_or_else(missing)?;
    if state == "ready" {
        sqlx::query("UPDATE assets SET deleted_at=COALESCE(deleted_at,now()) WHERE id=$1")
            .bind(internal_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE shares SET active=false WHERE asset_id=$1 AND active")
            .bind(internal_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM share_viewer_grants WHERE share_id IN (SELECT id FROM shares WHERE asset_id=$1)").bind(internal_id).execute(&mut *tx).await?;
    } else {
        sqlx::query(
            "UPDATE assets SET state='cancelled',deleted_at=COALESCE(deleted_at,now()) WHERE id=$1",
        )
        .bind(internal_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Asset>, ShareError> {
    s.auth.check_mutation(&h)?;
    s.store()?;
    let u = s.auth.authenticate(&h).await?;
    let mut tx = s.auth.pool.begin().await?;
    let found: Option<i64> = sqlx::query_scalar("SELECT id FROM assets WHERE external_id=$1 AND user_id=$2 AND state='ready' AND deleted_at IS NOT NULL FOR UPDATE")
        .bind(&id).bind(u.internal_id).fetch_optional(&mut *tx).await?;
    let internal_id = found.ok_or_else(missing)?;
    sqlx::query("UPDATE assets SET deleted_at=NULL WHERE id=$1")
        .bind(internal_id)
        .execute(&mut *tx)
        .await?;
    let restored = asset(&mut tx, &id).await?;
    tx.commit().await?;
    Ok(Json(restored))
}

fn require_worker(s: &SharingState, h: &HeaderMap) -> Result<(), ShareError> {
    s.store()?;
    let secret = s.media_worker_secret.as_deref().ok_or_else(unavailable)?;
    let supplied = h
        .get("x-captures-media-key")
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    // HMAC verification compares fixed-length tags in constant time.
    let mut expected =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| unavailable())?;
    expected.update(b"captures-media-worker");
    let mut received = Hmac::<Sha256>::new_from_slice(supplied).map_err(|_| unavailable())?;
    received.update(b"captures-media-worker");
    expected
        .verify_slice(&received.finalize().into_bytes())
        .map_err(|_| ShareError(StatusCode::UNAUTHORIZED, "Worker authentication required"))
}
async fn owner_media(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ShareError> {
    require_worker(&s, &h)?;
    let u = s.auth.authenticate(&h).await?;
    let row: Option<(String, String, i64, String)> = sqlx::query_as(
        "SELECT content_type,name,byte_size,storage_key FROM assets WHERE external_id=$1 AND user_id=$2 AND state='ready' AND deleted_at IS NULL",
    )
    .bind(&id)
    .bind(u.internal_id)
    .fetch_optional(&s.auth.pool)
    .await?;
    let (t, n, size, storage_key) = row.ok_or_else(missing)?;
    Ok(Json(
        json!({"key":storage_key,"contentType":t,"name":n,"byteSize":size}),
    ))
}

#[derive(Deserialize)]
struct ShareUpdate {
    enabled: bool,
    #[serde(default, deserialize_with = "patch_value")]
    password: Option<Value>,
    #[serde(rename = "expiresAt", default, deserialize_with = "patch_value")]
    expires_at: Option<Value>,
}
fn patch_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}
async fn update_share(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(b): Json<ShareUpdate>,
) -> Result<Json<Value>, ShareError> {
    s.auth.check_mutation(&h)?;
    let u = s.auth.authenticate(&h).await?;
    s.store()?;
    let hash_permit = s
        .work
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    let mut tx = s.auth.pool.begin().await?;
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM assets WHERE external_id=$1 AND user_id=$2 AND state='ready' AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(&id)
    .bind(u.internal_id)
    .fetch_optional(&mut *tx)
    .await?;
    let asset_internal_id = found.ok_or_else(missing)?;
    let current: Option<(String, Option<String>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id,password_hash,expires_at FROM shares WHERE asset_id=$1 AND active FOR UPDATE",
    )
    .bind(asset_internal_id)
    .fetch_optional(&mut *tx)
    .await?;
    if !b.enabled {
        if let Some((sid, _, _)) = current {
            sqlx::query("DELETE FROM share_viewer_grants WHERE share_id=$1")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE shares SET active=false WHERE id=$1")
                .bind(sid)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        return Ok(Json(json!({"share":null})));
    }
    let previous = current.as_ref().and_then(|x| x.1.clone());
    let password = tokio::task::spawn_blocking(move || {
        let _permit = hash_permit;
        patch_password(b.password, previous)
    })
    .await
    .map_err(|_| unavailable())??;
    let expiry = patch_expiry(b.expires_at, current.as_ref().and_then(|x| x.2))?;
    let sid = if let Some((sid, old, _)) = current {
        if old != password {
            sqlx::query("DELETE FROM share_viewer_grants WHERE share_id=$1")
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE shares SET password_hash=$2,expires_at=$3 WHERE id=$1")
            .bind(&sid)
            .bind(&password)
            .bind(expiry)
            .execute(&mut *tx)
            .await?;
        sid
    } else {
        insert_share(&mut tx, asset_internal_id, &password, expiry, s.new_id).await?
    };
    let row:Share=sqlx::query_as("SELECT id,password_hash IS NOT NULL password_protected,expires_at,shared_at FROM shares WHERE id=$1").bind(sid).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"share":row})))
}
async fn insert_share(
    tx: &mut Transaction<'_, Postgres>,
    asset: i64,
    password: &Option<String>,
    expiry: Option<DateTime<Utc>>,
    next_id: fn() -> String,
) -> Result<String, ShareError> {
    for _ in 0..8 {
        let id = next_id();
        let result = sqlx::query(
            "INSERT INTO shares(id,asset_id,password_hash,expires_at) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO NOTHING",
        )
        .bind(&id)
        .bind(asset)
        .bind(password)
        .bind(expiry)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() == 1 {
            return Ok(id);
        }
        tracing::warn!(kind = "share_id", "public id collision; regenerating");
    }
    Err(unavailable())
}
fn patch_password(v: Option<Value>, old: Option<String>) -> Result<Option<String>, ShareError> {
    match v {
        None => Ok(old),
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if (8..=128).contains(&s.chars().count()) => {
            let salt =
                SaltString::encode_b64(&rand::random::<[u8; 16]>()).map_err(|_| unavailable())?;
            Ok(Some(
                Argon2::default()
                    .hash_password(s.as_bytes(), &salt)
                    .map_err(|_| unavailable())?
                    .to_string(),
            ))
        }
        _ => Err(bad("Invalid password")),
    }
}
fn patch_expiry(
    v: Option<Value>,
    old: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, ShareError> {
    match v {
        None => Ok(old),
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let date: DateTime<Utc> = s.parse().map_err(|_| bad("Invalid expiry"))?;
            if date <= Utc::now() {
                return Err(bad("Expiry must be in the future"));
            }
            Ok(Some(date))
        }
        _ => Err(bad("Invalid expiry")),
    }
}

#[derive(FromRow)]
struct Access {
    id: String,
    storage_key: String,
    user_id: i64,
    password_hash: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    name: String,
    content_type: String,
    byte_size: i64,
}
async fn access(s: &SharingState, id: &str) -> Result<Access, ShareError> {
    s.store()?;
    sqlx::query_as("SELECT s.id,a.storage_key,a.user_id,s.password_hash,s.expires_at,a.name,a.content_type,a.byte_size FROM shares s JOIN assets a ON a.id=s.asset_id JOIN users u ON u.id=a.user_id WHERE s.id=$1 AND s.active AND (s.expires_at IS NULL OR s.expires_at>now()) AND a.state='ready' AND a.deleted_at IS NULL AND u.disabled_at IS NULL AND u.deleted_at IS NULL").bind(id).fetch_optional(&s.auth.pool).await?.ok_or_else(missing)
}
async fn permitted(s: &SharingState, a: &Access, h: &HeaderMap) -> Result<bool, ShareError> {
    if let Ok(u) = s.auth.authenticate(h).await
        && u.internal_id == a.user_id
    {
        return Ok(true);
    }
    if a.password_hash.is_none() {
        return Ok(true);
    }
    let prefix = format!("captures_view_{}=", a.id);
    let token = h
        .get(header::COOKIE)
        .and_then(|x| x.to_str().ok())
        .and_then(|x| {
            x.split(';')
                .map(str::trim)
                .find_map(|x| x.strip_prefix(&prefix))
        });
    let Some(token) = token else { return Ok(false) };
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM share_viewer_grants WHERE token_hash=$1 AND share_id=$2)",
    )
    .bind(Sha256::digest(token).to_vec())
    .bind(&a.id)
    .fetch_one(&s.auth.pool)
    .await?)
}
async fn metadata(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ShareError> {
    let a = access(&s, &id).await?;
    let can = permitted(&s, &a, &h).await?;
    Ok(Json(
        json!({"id":a.id,"name":a.name,"contentType":a.content_type,"byteSize":a.byte_size,"passwordRequired":a.password_hash.is_some(),"expiresAt":a.expires_at,"mediaUrl":can.then(||format!("/media/shares/{id}"))}),
    ))
}
async fn shared_media(
    State(s): State<SharingState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ShareError> {
    require_worker(&s, &h)?;
    let a = access(&s, &id).await?;
    if !permitted(&s, &a, &h).await? {
        return Err(ShareError(StatusCode::UNAUTHORIZED, "Password required"));
    }
    Ok(Json(
        json!({"key":a.storage_key,"contentType":a.content_type,"name":a.name,"byteSize":a.byte_size}),
    ))
}
#[derive(Deserialize)]
struct Unlock {
    password: String,
}
async fn unlock(
    State(s): State<SharingState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(b): Json<Unlock>,
) -> Result<Response, ShareError> {
    s.auth.check_mutation(&h)?;
    let a = access(&s, &id).await?;
    let hash = a.password_hash.ok_or_else(missing)?;
    if b.password.len() > 512 {
        return Err(bad("Invalid password"));
    }
    let permit = s
        .work
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    let source_ip = s.auth.source_ip(&h, peer.ip())?;
    let ip = s.auth.source_hash(&h, peer.ip())?;
    let user_agent = h
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(512).collect::<String>());
    let mut tx = s.auth.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(734894239112::bigint)")
        .execute(&mut *tx)
        .await?;
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM share_unlock_attempts WHERE ip_hash=$1 AND created_at>now()-interval '15 minutes'),(SELECT count(*) FROM share_unlock_attempts WHERE share_id=$2 AND created_at>now()-interval '15 minutes'),(SELECT count(*) FROM share_unlock_attempts WHERE created_at>now()-interval '1 hour')").bind(&ip).bind(&id).fetch_one(&mut *tx).await?;
    if counts.0 >= 20 || counts.1 >= 50 || counts.2 >= 1000 {
        return Err(ShareError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests",
        ));
    }
    let attempt_id: i64 = sqlx::query_scalar("INSERT INTO share_unlock_attempts(share_id,ip_hash,source_ip,user_agent) VALUES($1,$2,$3::inet,$4) RETURNING id")
        .bind(&id)
        .bind(ip)
        .bind(source_ip.to_string())
        .bind(user_agent)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let expected = hash.clone();
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(&expected).is_ok_and(|x| {
            Argon2::default()
                .verify_password(b.password.as_bytes(), &x)
                .is_ok()
        })
    })
    .await
    .map_err(|_| unavailable())?;
    if !valid {
        sqlx::query("UPDATE share_unlock_attempts SET outcome='denied' WHERE id=$1")
            .bind(attempt_id)
            .execute(&s.auth.pool)
            .await?;
        return Err(ShareError(StatusCode::UNAUTHORIZED, "Incorrect password"));
    }
    // Serialize the grant insertion against password edits and disabling. An
    // old password verified while an edit was running must not mint a new grant.
    let mut tx = s.auth.pool.begin().await?;
    let current:Option<Option<String>>=sqlx::query_scalar("SELECT password_hash FROM shares WHERE id=$1 AND active AND (expires_at IS NULL OR expires_at>now()) FOR UPDATE").bind(&id).fetch_optional(&mut *tx).await?;
    if current != Some(Some(hash)) {
        sqlx::query("UPDATE share_unlock_attempts SET outcome='denied' WHERE id=$1")
            .bind(attempt_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(ShareError(
            StatusCode::UNAUTHORIZED,
            "Share access changed; try again",
        ));
    }
    let token = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    sqlx::query("INSERT INTO share_viewer_grants(token_hash,share_id) VALUES($1,$2)")
        .bind(Sha256::digest(&token).to_vec())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE share_unlock_attempts SET outcome='granted' WHERE id=$1")
        .bind(attempt_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut r = StatusCode::NO_CONTENT.into_response();
    let secure = if s.auth.secure_cookie() {
        "; Secure"
    } else {
        ""
    };
    r.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "captures_view_{id}={token}; Path=/; HttpOnly; SameSite=Lax{secure}"
        ))
        .map_err(|_| unavailable())?,
    );
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_parts() {
        assert_eq!(multipart_shape(1).unwrap(), (64 * 1024 * 1024, 1));
        assert_eq!(multipart_shape(64 * 1024 * 1024 + 1).unwrap().1, 2);
    }
    #[test]
    fn ids_are_url_safe_and_twelve() {
        for _ in 0..100 {
            let x = nanoid::nanoid!(12);
            assert_eq!(x.len(), 12);
            assert!(
                x.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            );
        }
    }
}
