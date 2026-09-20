use crate::{
    auth::{AuthError, AuthState},
    storage::{MAX_BYTES, ObjectStore},
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use image::{ImageDecoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::{io::Cursor, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use uuid::Uuid;

#[derive(Clone)]
pub struct SharingState {
    pub auth: AuthState,
    pub store: Option<Arc<dyn ObjectStore>>,
    work: Arc<Semaphore>,
}
impl SharingState {
    pub fn new(auth: AuthState, store: Option<Arc<dyn ObjectStore>>) -> Self {
        Self {
            auth,
            store,
            work: Arc::new(Semaphore::new(2)),
        }
    }
    fn store(&self) -> Result<&dyn ObjectStore, ShareError> {
        self.store.as_deref().ok_or_else(unavailable)
    }

    // Pending writes have no viewer access. Persisted deleting state immediately
    // denies reads, then retries object deletion after failures or process death.
    pub async fn cleanup(&self) -> Result<(), ShareError> {
        let store = self.store()?;
        let ids: Vec<Uuid> = sqlx::query_scalar("UPDATE uploads SET state='deleting' WHERE state='pending' AND created_at<now()-interval '1 hour' RETURNING id")
            .fetch_all(&self.auth.pool).await?;
        drop(ids);
        let ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM uploads WHERE state='deleting' LIMIT 100")
                .fetch_all(&self.auth.pool)
                .await?;
        for id in ids {
            if store.delete(&object_key(id)).await.is_ok() {
                sqlx::query("DELETE FROM uploads WHERE id=$1 AND state='deleting'")
                    .bind(id)
                    .execute(&self.auth.pool)
                    .await?;
            }
        }
        sqlx::query("DELETE FROM share_viewer_grants WHERE expires_at<now()")
            .execute(&self.auth.pool)
            .await?;
        sqlx::query("DELETE FROM share_unlock_attempts WHERE created_at<now()-interval '1 day'")
            .execute(&self.auth.pool)
            .await?;
        Ok(())
    }
}

pub fn router(state: SharingState) -> Router {
    Router::new()
        .route("/api/uploads", get(list_uploads).post(upload))
        .route("/api/uploads/{id}", delete(delete_upload))
        .route("/api/uploads/{id}/media", get(owner_media))
        .route("/api/uploads/{id}/shares", post(create_share))
        .route("/api/shares/{id}", get(metadata))
        .route("/api/shares/{id}/media", get(shared_media))
        .route("/api/shares/{id}/unlock", post(unlock))
        .route("/api/shares/{id}/revoke", post(revoke))
        .with_state(state)
        .layer(middleware::map_response(private_response))
}

async fn private_response(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    h.insert("cdn-cache-control", HeaderValue::from_static("no-store"));
    h.insert(
        "cloudflare-cdn-cache-control",
        HeaderValue::from_static("no-store"),
    );
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.entry("x-robots-tag")
        .or_insert(HeaderValue::from_static("noindex, nofollow, noarchive"));
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
    fn from(value: AuthError) -> Self {
        let status = value.into_response().status();
        Self(
            status,
            if status == StatusCode::UNAUTHORIZED {
                "Sign in to continue"
            } else {
                "Request not allowed"
            },
        )
    }
}
fn unavailable() -> ShareError {
    ShareError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Sharing is temporarily unavailable",
    )
}
fn missing() -> ShareError {
    ShareError(StatusCode::NOT_FOUND, "Share or image not available")
}
fn bad(message: &'static str) -> ShareError {
    ShareError(StatusCode::BAD_REQUEST, message)
}
fn busy() -> ShareError {
    ShareError(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many requests. Try again later.",
    )
}
fn object_key(id: Uuid) -> String {
    format!("images/{id}")
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Upload {
    id: Uuid,
    content_type: String,
    byte_size: i64,
    created_at: DateTime<Utc>,
    #[sqlx(skip)]
    shares: Vec<Share>,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Share {
    id: Uuid,
    visibility: String,
    password_protected: bool,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

async fn list_uploads(
    State(s): State<SharingState>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, ShareError> {
    let user = s.auth.authenticate(&h).await?;
    s.store()?;
    let mut rows: Vec<Upload> = sqlx::query_as("SELECT id,content_type,byte_size,created_at FROM uploads WHERE user_id=$1 AND state='ready' ORDER BY created_at DESC")
        .bind(user.id).fetch_all(&s.auth.pool).await?;
    for row in &mut rows {
        row.shares = sqlx::query_as("SELECT id,visibility,password_hash IS NOT NULL AS password_protected,expires_at,revoked_at FROM shares WHERE upload_id=$1 ORDER BY created_at DESC")
            .bind(row.id).fetch_all(&s.auth.pool).await?;
    }
    Ok(Json(json!({"uploads":rows})))
}

async fn upload(
    State(s): State<SharingState>,
    request: Request,
) -> Result<impl IntoResponse, ShareError> {
    s.auth.check_mutation(request.headers())?;
    let user = s.auth.authenticate(request.headers()).await?;
    let store = s.store()?;
    let permit = s.work.clone().try_acquire_owned().map_err(|_| busy())?;
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let format = match content_type.as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::WebP,
        _ => return Err(bad("Choose a static PNG, JPEG, or WebP image")),
    };
    let bytes = tokio::time::timeout(
        Duration::from_secs(60),
        to_bytes(request.into_body(), MAX_BYTES),
    )
    .await
    .map_err(|_| ShareError(StatusCode::REQUEST_TIMEOUT, "Upload timed out"))?
    .map_err(|_| {
        ShareError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Images must be 20 MiB or smaller",
        )
    })?;
    let (bytes, _permit) =
        tokio::task::spawn_blocking(move || (sanitize_image(&bytes, format), permit))
            .await
            .map_err(|_| unavailable())?;
    let bytes = bytes?;
    let id = Uuid::new_v4();
    let mut tx = s.auth.pool.begin().await?;
    // Lock the account row so simultaneous uploads cannot overrun its quota.
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let (count, total): (i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(byte_size),0)::bigint FROM uploads WHERE user_id=$1",
    )
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if count >= 100 || total + bytes.len() as i64 > 1024 * 1024 * 1024 {
        return Err(ShareError(
            StatusCode::CONFLICT,
            "Account limit reached: 100 images or 1 GiB",
        ));
    }
    let row: Upload = sqlx::query_as("INSERT INTO uploads(id,user_id,content_type,byte_size,state) VALUES($1,$2,$3,$4,'pending') RETURNING id,content_type,byte_size,created_at")
        .bind(id).bind(user.id).bind(&content_type).bind(bytes.len() as i64).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    // On an uncertain write failure retain the reservation for durable cleanup.
    store
        .put(&object_key(id), &content_type, bytes)
        .await
        .map_err(|_| unavailable())?;
    let result = sqlx::query("UPDATE uploads SET state='ready' WHERE id=$1 AND state='pending'")
        .bind(id)
        .execute(&s.auth.pool)
        .await?;
    if result.rows_affected() != 1 {
        return Err(unavailable());
    }
    Ok((StatusCode::CREATED, Json(row)))
}

fn sanitize_image(bytes: &[u8], format: ImageFormat) -> Result<Vec<u8>, ShareError> {
    if image::guess_format(bytes).ok() != Some(format) {
        return Err(bad("Image contents do not match its file type"));
    }
    let cursor = Cursor::new(bytes);
    let animated = match format {
        ImageFormat::Png => image::codecs::png::PngDecoder::new(cursor)
            .map_err(|_| bad("Invalid image"))?
            .is_apng()
            .map_err(|_| bad("Invalid image"))?,
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(cursor)
            .map_err(|_| bad("Invalid image"))?
            .has_animation(),
        _ => false,
    };
    if animated {
        return Err(bad("Animated images are not supported yet"));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12000);
    limits.max_image_height = Some(12000);
    limits.max_alloc = Some(160 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| bad("Invalid or oversized image"))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 32_000_000 {
        return Err(bad("Images must contain at most 32 megapixels"));
    }
    let orientation = decoder
        .orientation()
        .map_err(|_| bad("Invalid image orientation"))?;
    let mut decoded =
        image::DynamicImage::from_decoder(decoder).map_err(|_| bad("Invalid image"))?;
    // Normalize metadata and remove EXIF/GPS, trailing data and non-image payloads.
    // Orient before stripping EXIF so portrait photographs keep their appearance.
    decoded.apply_orientation(orientation);
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, format)
        .map_err(|_| bad("Image could not be encoded"))?;
    if output.get_ref().len() > MAX_BYTES {
        return Err(bad("Normalized image exceeds 20 MiB"));
    }
    Ok(output.into_inner())
}

async fn delete_upload(
    State(s): State<SharingState>,
    Path(id): Path<Uuid>,
    h: HeaderMap,
) -> Result<StatusCode, ShareError> {
    s.auth.check_mutation(&h)?;
    let user = s.auth.authenticate(&h).await?;
    let store = s.store()?;
    let result = sqlx::query("UPDATE uploads SET state='deleting' WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.id)
        .execute(&s.auth.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(missing());
    }
    // The persisted state denies access even if the provider is unavailable.
    if store.delete(&object_key(id)).await.is_ok() {
        sqlx::query("DELETE FROM uploads WHERE id=$1 AND state='deleting'")
            .bind(id)
            .execute(&s.auth.pool)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewShare {
    visibility: String,
    password: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}
async fn create_share(
    State(s): State<SharingState>,
    Path(id): Path<Uuid>,
    h: HeaderMap,
    Json(body): Json<NewShare>,
) -> Result<impl IntoResponse, ShareError> {
    s.auth.check_mutation(&h)?;
    let user = s.auth.authenticate(&h).await?;
    s.store()?;
    if !matches!(body.visibility.as_str(), "private" | "unlisted" | "public") {
        return Err(bad("Invalid visibility"));
    }
    if body.expires_at.is_some_and(|e| e <= Utc::now()) {
        return Err(bad("Expiry must be in the future"));
    }
    let password = body.password.filter(|p| !p.is_empty());
    if password
        .as_ref()
        .is_some_and(|p| !(8..=128).contains(&p.chars().count()))
    {
        return Err(bad("Passwords must be between 8 and 128 characters"));
    }
    let password_hash = if let Some(password) = password {
        let permit = s.work.clone().try_acquire_owned().map_err(|_| busy())?;
        Some(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let salt = SaltString::encode_b64(&rand::random::<[u8; 16]>())
                    .map_err(|_| unavailable())?;
                Argon2::default()
                    .hash_password(password.as_bytes(), &salt)
                    .map(|h| h.to_string())
                    .map_err(|_| unavailable())
            })
            .await
            .map_err(|_| unavailable())??,
        )
    } else {
        None
    };
    let mut tx = s.auth.pool.begin().await?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM uploads WHERE id=$1 AND user_id=$2 AND state='ready' FOR UPDATE",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;
    if found.is_none() {
        return Err(missing());
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM shares WHERE upload_id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if count >= 20 {
        return Err(bad("An image can have at most 20 share links"));
    }
    let row: Share = sqlx::query_as("INSERT INTO shares(id,upload_id,visibility,password_hash,expires_at) VALUES($1,$2,$3,$4,$5) RETURNING id,visibility,password_hash IS NOT NULL AS password_protected,expires_at,revoked_at")
        .bind(Uuid::new_v4()).bind(id).bind(body.visibility).bind(password_hash).bind(body.expires_at).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(FromRow)]
struct Access {
    id: Uuid,
    upload_id: Uuid,
    user_id: i64,
    visibility: String,
    password_hash: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    content_type: String,
}
async fn active_share(s: &SharingState, id: Uuid) -> Result<Access, ShareError> {
    s.store()?;
    sqlx::query_as("SELECT s.id,s.upload_id,u.user_id,s.visibility,s.password_hash,s.expires_at,u.content_type FROM shares s JOIN uploads u ON u.id=s.upload_id JOIN users a ON a.id=u.user_id WHERE s.id=$1 AND s.revoked_at IS NULL AND (s.expires_at IS NULL OR s.expires_at>now()) AND u.state='ready' AND a.disabled_at IS NULL AND a.deleted_at IS NULL")
        .bind(id).fetch_optional(&s.auth.pool).await?.ok_or_else(missing)
}
async fn permitted(s: &SharingState, a: &Access, h: &HeaderMap) -> Result<bool, ShareError> {
    let owner = if h.contains_key(header::AUTHORIZATION) || h.contains_key(header::COOKIE) {
        match s.auth.authenticate(h).await {
            Ok(user) => user.id == a.user_id,
            Err(e) => {
                if e.into_response().status() != StatusCode::UNAUTHORIZED {
                    return Err(unavailable());
                }
                false
            }
        }
    } else {
        false
    };
    if a.visibility == "private" && !owner {
        return Err(missing());
    }
    if owner || a.password_hash.is_none() {
        return Ok(true);
    }
    let name = format!("captures_view_{}=", a.id.simple());
    let token = h
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .map(str::trim)
                .find_map(|v| v.strip_prefix(&name))
        });
    let Some(token) = token else {
        return Ok(false);
    };
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM share_viewer_grants WHERE token_hash=$1 AND share_id=$2 AND expires_at>now())")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(a.id).fetch_one(&s.auth.pool).await?;
    Ok(valid)
}
fn indexing(response: &mut Response, access: &Access) {
    if access.visibility == "public" && access.password_hash.is_none() {
        response
            .headers_mut()
            .insert("x-robots-tag", HeaderValue::from_static("index, follow"));
    }
}
async fn metadata(
    State(s): State<SharingState>,
    Path(id): Path<Uuid>,
    h: HeaderMap,
) -> Result<Response, ShareError> {
    let a = active_share(&s, id).await?;
    let can_read = permitted(&s, &a, &h).await?;
    let mut response = Json(json!({"id":id,"visibility":a.visibility,"passwordRequired":a.password_hash.is_some(),"expiresAt":a.expires_at,"mediaUrl":can_read.then(||format!("/api/shares/{id}/media"))})).into_response();
    indexing(&mut response, &a);
    Ok(response)
}
fn image_response(bytes: Vec<u8>, content_type: &str) -> Result<Response, ShareError> {
    let mut r = bytes.into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).map_err(|_| unavailable())?,
    );
    r.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    r.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(r)
}
async fn shared_media(
    State(s): State<SharingState>,
    Path(id): Path<Uuid>,
    h: HeaderMap,
) -> Result<Response, ShareError> {
    let a = active_share(&s, id).await?;
    if !permitted(&s, &a, &h).await? {
        return Err(ShareError(StatusCode::UNAUTHORIZED, "Password required"));
    }
    let bytes = s
        .store()?
        .get(&object_key(a.upload_id))
        .await
        .map_err(|_| unavailable())?;
    // Check again after provider latency: revocation during the object fetch must
    // not hand out a fresh response based on stale authorization.
    let current = active_share(&s, id).await?;
    if !permitted(&s, &current, &h).await? {
        return Err(missing());
    }
    let mut response = image_response(bytes, &a.content_type)?;
    indexing(&mut response, &current);
    Ok(response)
}
async fn owner_media(
    State(s): State<SharingState>,
    Path(id): Path<Uuid>,
    h: HeaderMap,
) -> Result<Response, ShareError> {
    let user = s.auth.authenticate(&h).await?;
    let content_type: String = sqlx::query_scalar(
        "SELECT content_type FROM uploads WHERE id=$1 AND user_id=$2 AND state='ready'",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.auth.pool)
    .await?
    .ok_or_else(missing)?;
    let bytes = s
        .store()?
        .get(&object_key(id))
        .await
        .map_err(|_| unavailable())?;
    s.auth.authenticate(&h).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM uploads WHERE id=$1 AND user_id=$2 AND state='ready')",
    )
    .bind(id)
    .bind(user.id)
    .fetch_one(&s.auth.pool)
    .await?;
    if !exists {
        return Err(missing());
    }
    image_response(bytes, &content_type)
}
async fn revoke(
    State(s): State<SharingState>,
    Path(id): Path<Uuid>,
    h: HeaderMap,
) -> Result<StatusCode, ShareError> {
    s.auth.check_mutation(&h)?;
    let user = s.auth.authenticate(&h).await?;
    let result = sqlx::query("UPDATE shares SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1 AND upload_id IN (SELECT id FROM uploads WHERE user_id=$2)")
        .bind(id).bind(user.id).execute(&s.auth.pool).await?;
    if result.rows_affected() == 0 {
        return Err(missing());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Unlock {
    password: String,
}
async fn unlock(
    State(s): State<SharingState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<Uuid>,
    h: HeaderMap,
    Json(body): Json<Unlock>,
) -> Result<Response, ShareError> {
    s.auth.check_mutation(&h)?;
    let a = active_share(&s, id).await?;
    if a.visibility == "private" {
        return Err(missing());
    }
    let password_hash = a.password_hash.ok_or_else(missing)?;
    if body.password.len() > 512 {
        return Err(bad("Invalid password"));
    }
    let permit = s.work.clone().try_acquire_owned().map_err(|_| busy())?;
    let ip_hash = s.auth.source_hash(&h, peer.ip())?;
    let mut tx = s.auth.pool.begin().await?;
    // One short transaction serializes attempts across replicas, not hash work.
    sqlx::query("SELECT pg_advisory_xact_lock(734894239112::bigint)")
        .execute(&mut *tx)
        .await?;
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM share_unlock_attempts WHERE ip_hash=$1 AND created_at>now()-interval '15 minutes'),(SELECT count(*) FROM share_unlock_attempts WHERE share_id=$2 AND created_at>now()-interval '15 minutes'),(SELECT count(*) FROM share_unlock_attempts WHERE created_at>now()-interval '1 hour')")
        .bind(&ip_hash).bind(id).fetch_one(&mut *tx).await?;
    if counts.0 >= 20 || counts.1 >= 50 || counts.2 >= 1000 {
        return Err(busy());
    }
    sqlx::query("INSERT INTO share_unlock_attempts(share_id,ip_hash) VALUES($1,$2)")
        .bind(id)
        .bind(ip_hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(&password_hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(body.password.as_bytes(), &hash)
                .is_ok()
        })
    })
    .await
    .map_err(|_| unavailable())?;
    if !valid {
        return Err(ShareError(StatusCode::UNAUTHORIZED, "Incorrect password"));
    }
    active_share(&s, id).await?;
    let token = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    sqlx::query("INSERT INTO share_viewer_grants(token_hash,share_id,expires_at) VALUES($1,$2,now()+interval '15 minutes')")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(&s.auth.pool).await?;
    let secure = if s.auth.secure_cookie() {
        "; Secure"
    } else {
        ""
    };
    let mut response = StatusCode::NO_CONTENT.into_response();
    // Path=/s cannot authorize /api media. Use a per-share cookie name at /
    // so both SSR and media requests see it; unrelated grants cannot authorize it.
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "captures_view_{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=900{secure}",
            id.simple()
        ))
        .map_err(|_| unavailable())?,
    );
    Ok(response)
}
