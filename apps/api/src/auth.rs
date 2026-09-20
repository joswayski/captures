use crate::{
    config::AuthConfig,
    email::{Mailer, SesMailer},
};
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;
const COOKIE: &str = "captures_session";
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

#[derive(Clone)]
pub struct AuthState {
    pub pool: PgPool,
    inner: Option<Arc<Enabled>>,
}
struct Enabled {
    secret: Vec<u8>,
    mailer: Arc<dyn Mailer>,
    origin: String,
    trust_cf: bool,
    insecure_cookie: bool,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct User {
    #[serde(serialize_with = "serialize_id")]
    pub id: i64,
    pub email: String,
}

fn serialize_id<S: serde::Serializer>(id: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&id.to_string())
}

#[derive(Debug)]
pub struct AuthError {
    status: StatusCode,
    message: &'static str,
    attempts: Option<i16>,
}
impl AuthError {
    fn new(status: StatusCode, message: &'static str) -> Self {
        Self {
            status,
            message,
            attempts: None,
        }
    }
}
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let mut body = json!({"error": self.message});
        if let Some(n) = self.attempts {
            body["attemptsRemaining"] = json!(n);
        }
        let mut response = (self.status, Json(body)).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

impl AuthState {
    pub fn disabled(pool: PgPool) -> Self {
        Self { pool, inner: None }
    }

    pub fn check_mutation(&self, headers: &HeaderMap) -> Result<(), AuthError> {
        mutation_allowed(self, headers)
    }

    pub fn secure_cookie(&self) -> bool {
        !self.inner.as_ref().is_some_and(|a| a.insecure_cookie)
    }

    pub fn source_hash(&self, headers: &HeaderMap, peer: IpAddr) -> Result<Vec<u8>, AuthError> {
        let auth = self.enabled()?;
        Ok(keyed_hash(
            &auth.secret,
            b"ip",
            source_ip(auth, headers, peer).to_string().as_bytes(),
        ))
    }

    #[cfg(test)]
    pub fn for_test(pool: PgPool, mailer: Arc<dyn Mailer>, origin: &str) -> Self {
        Self {
            pool,
            inner: Some(Arc::new(Enabled {
                secret: vec![42; 32],
                mailer,
                origin: origin.into(),
                trust_cf: false,
                insecure_cookie: false,
            })),
        }
    }

    pub async fn new(pool: PgPool, config: AuthConfig) -> Self {
        let inner = if config.enabled {
            let mailer = SesMailer::new(config.ses_from, config.ses_configuration_set).await;
            Some(Arc::new(Enabled {
                secret: config.secret.into_bytes(),
                mailer: Arc::new(mailer),
                origin: config.allowed_origin,
                trust_cf: config.trust_cf_connecting_ip,
                insecure_cookie: config.insecure_loopback_cookie,
            }))
        } else {
            None
        };
        Self { pool, inner }
    }

    fn enabled(&self) -> Result<&Enabled, AuthError> {
        self.inner
            .as_deref()
            .ok_or_else(|| AuthError::new(StatusCode::SERVICE_UNAVAILABLE, "accounts unavailable"))
    }

    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<User, AuthError> {
        self.enabled()?;
        let token = bearer(headers)
            .or_else(|| cookie(headers))
            .ok_or_else(unauthorized)?;
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        sqlx::query_as("SELECT u.id, u.email FROM account_sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now() AND u.disabled_at IS NULL AND u.deleted_at IS NULL AND u.email IS NOT NULL")
            .bind(hash).fetch_optional(&self.pool).await.map_err(db_error)?.ok_or_else(unauthorized)
    }
}

pub fn router(state: AuthState) -> Router {
    Router::new()
        .route("/api/auth/email/request", post(request_code))
        .route("/api/auth/email/verify", post(verify_code))
        .route("/api/account/me", get(me))
        .route("/api/auth/logout", post(logout))
        .with_state(state)
        .layer(middleware::map_response(no_store))
}

async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Deserialize)]
struct RequestBody {
    email: String,
}
#[derive(Deserialize)]
struct VerifyBody {
    #[serde(rename = "challengeId")]
    challenge_id: Uuid,
    code: String,
    transport: Transport,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Transport {
    Cookie,
    Bearer,
}

async fn request_code(
    State(state): State<AuthState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<RequestBody>,
) -> Result<impl IntoResponse, AuthError> {
    mutation_allowed(&state, &headers)?;
    let auth = state.enabled()?;
    let email = normalize_email(&body.email)?;
    let ip = source_ip(auth, &headers, peer.ip());
    let ip_hash = keyed_hash(&auth.secret, b"ip", ip.to_string().as_bytes());
    let mut locks = [
        lock(&auth.secret, b"email-lock", email.as_bytes()),
        lock(&auth.secret, b"ip-lock", &ip_hash),
        lock(&auth.secret, b"global-lock", b"captures"),
    ];
    locks.sort_unstable();
    let mut tx = state.pool.begin().await.map_err(db_error)?;
    for value in locks {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(value)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
    }
    sqlx::query("DELETE FROM auth_email_challenges WHERE expires_at < now()-interval '7 days'")
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    sqlx::query("DELETE FROM account_sessions WHERE expires_at < now()-interval '7 days'")
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    let counts: (i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM auth_email_challenges WHERE email=$1 AND created_at>now()-interval '15 minutes'),(SELECT count(*) FROM auth_email_challenges WHERE email=$1 AND created_at>now()-interval '24 hours'),(SELECT count(*) FROM auth_email_challenges WHERE request_ip_hash=$2 AND created_at>now()-interval '1 hour'),(SELECT count(*) FROM auth_email_challenges WHERE created_at>now()-interval '1 hour')").bind(&email).bind(&ip_hash).fetch_one(&mut *tx).await.map_err(db_error)?;
    let id = Uuid::new_v4();
    if counts.0 >= 3 || counts.1 >= 5 || counts.2 >= 10 || counts.3 >= 500 {
        tx.commit().await.map_err(db_error)?;
        return Ok((StatusCode::ACCEPTED, Json(json!({"challengeId":id}))));
    }
    let code = random_code();
    let hash = code_hash(&auth.secret, id, &email, &code);
    sqlx::query(
        "UPDATE auth_email_challenges SET consumed_at=now() WHERE email=$1 AND consumed_at IS NULL",
    )
    .bind(&email)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    sqlx::query("INSERT INTO auth_email_challenges(id,email,code_hash,request_ip_hash,attempts_remaining,expires_at) VALUES($1,$2,$3,$4,3,$5)").bind(id).bind(&email).bind(hash).bind(ip_hash).bind(Utc::now()+Duration::minutes(10)).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    if auth.mailer.send_code(&email, &code).await.is_err() {
        let _ = sqlx::query("UPDATE auth_email_challenges SET consumed_at=now() WHERE id=$1")
            .bind(id)
            .execute(&state.pool)
            .await;
        return Err(AuthError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "accounts unavailable",
        ));
    }
    Ok((StatusCode::ACCEPTED, Json(json!({"challengeId":id}))))
}

#[derive(FromRow)]
struct Challenge {
    email: String,
    code_hash: Vec<u8>,
    attempts_remaining: i16,
    expires_at: chrono::DateTime<Utc>,
    consumed_at: Option<chrono::DateTime<Utc>>,
}
async fn verify_code(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(body): Json<VerifyBody>,
) -> Result<Response, AuthError> {
    mutation_allowed(&state, &headers)?;
    let auth = state.enabled()?;
    if body.code.len() != 6
        || !body
            .code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(invalid(None));
    }
    let mut tx = state.pool.begin().await.map_err(db_error)?;
    let challenge:Challenge=sqlx::query_as("SELECT email,code_hash,attempts_remaining,expires_at,consumed_at FROM auth_email_challenges WHERE id=$1 FOR UPDATE").bind(body.challenge_id).fetch_optional(&mut *tx).await.map_err(db_error)?.ok_or_else(||invalid(None))?;
    if challenge.consumed_at.is_some()
        || challenge.expires_at <= Utc::now()
        || challenge.attempts_remaining <= 0
    {
        return Err(invalid(Some(0)));
    }
    let expected = code_hash(
        &auth.secret,
        body.challenge_id,
        &challenge.email,
        &body.code,
    );
    if !bool::from(challenge.code_hash.ct_eq(&expected)) {
        let left = challenge.attempts_remaining - 1;
        sqlx::query("UPDATE auth_email_challenges SET attempts_remaining=$2,consumed_at=CASE WHEN $2=0 THEN now() ELSE NULL END WHERE id=$1").bind(body.challenge_id).bind(left).execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        return Err(invalid(Some(left)));
    }
    sqlx::query("UPDATE auth_email_challenges SET consumed_at=now() WHERE id=$1")
        .bind(body.challenge_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    let user:User=sqlx::query_as("INSERT INTO users(email,email_verified) VALUES($1,true) ON CONFLICT(email) WHERE email IS NOT NULL DO UPDATE SET email_verified=true,updated_at=now() WHERE users.disabled_at IS NULL AND users.deleted_at IS NULL RETURNING id,email").bind(&challenge.email).fetch_optional(&mut *tx).await.map_err(db_error)?.ok_or_else(unauthorized)?;
    let bytes: [u8; 32] = rand::random();
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = Sha256::digest(token.as_bytes()).to_vec();
    sqlx::query("INSERT INTO account_sessions(token_hash,user_id,expires_at) VALUES($1,$2,$3)")
        .bind(hash)
        .bind(user.id)
        .bind(Utc::now() + Duration::days(30))
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    let mut value = json!({"user":user});
    let mut response = match body.transport {
        Transport::Bearer => {
            value["token"] = json!(token);
            Json(value).into_response()
        }
        Transport::Cookie => {
            let secure = if auth.insecure_cookie { "" } else { "; Secure" };
            let mut r = Json(value).into_response();
            r.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!(
                    "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{secure}"
                ))
                .expect("cookie"),
            );
            r
        }
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn me(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AuthError> {
    let user = state.authenticate(&headers).await?;
    Ok(Json(json!({"user":user})))
}
async fn logout(State(state): State<AuthState>, headers: HeaderMap) -> Result<Response, AuthError> {
    mutation_allowed(&state, &headers)?;
    state.enabled()?;
    let token = bearer(&headers)
        .or_else(|| cookie(&headers))
        .ok_or_else(unauthorized)?;
    sqlx::query("UPDATE account_sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(Sha256::digest(token.as_bytes()).to_vec())
        .execute(&state.pool)
        .await
        .map_err(db_error)?;
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "captures_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; Secure",
        ),
    );
    Ok(r)
}

fn mutation_allowed(state: &AuthState, h: &HeaderMap) -> Result<(), AuthError> {
    let auth = state.enabled()?;
    if let Some(origin) = h.get(header::ORIGIN) {
        if origin.as_bytes() != auth.origin.as_bytes() {
            return Err(AuthError::new(StatusCode::FORBIDDEN, "origin not allowed"));
        }
    } else if h.contains_key(header::COOKIE) || h.contains_key("sec-fetch-site") {
        // Native bearer/JSON clients omit browser headers. Cookie-bearing
        // mutations always require an exact trusted Origin, even on old clients.
        return Err(AuthError::new(StatusCode::FORBIDDEN, "origin required"));
    }
    Ok(())
}
fn source_ip(auth: &Enabled, h: &HeaderMap, fallback: IpAddr) -> IpAddr {
    if auth.trust_cf {
        h.get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(fallback)
    } else {
        fallback
    }
}
fn bearer(h: &HeaderMap) -> Option<&str> {
    h.get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|v| !v.is_empty())
}
fn cookie(h: &HeaderMap) -> Option<&str> {
    h.get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|v| v.strip_prefix(&format!("{COOKIE}=")))
        .filter(|v| !v.is_empty())
}
fn normalize_email(v: &str) -> Result<String, AuthError> {
    let e = v.trim().to_ascii_lowercase();
    if e.len() > 254 || !email_address::EmailAddress::is_valid(&e) {
        Err(AuthError::new(StatusCode::BAD_REQUEST, "invalid email"))
    } else {
        Ok(e)
    }
}
fn keyed_hash(secret: &[u8], domain: &[u8], value: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC key");
    mac.update(domain);
    mac.update(&[0]);
    mac.update(value);
    mac.finalize().into_bytes().to_vec()
}
fn lock(s: &[u8], d: &[u8], v: &[u8]) -> i64 {
    i64::from_be_bytes(keyed_hash(s, d, v)[..8].try_into().expect("hash"))
}
fn code_hash(s: &[u8], id: Uuid, email: &str, code: &str) -> Vec<u8> {
    let mut v = id.as_bytes().to_vec();
    v.push(0);
    v.extend(email.as_bytes());
    v.push(0);
    v.extend(code.as_bytes());
    keyed_hash(s, b"login-code", &v)
}
fn random_code() -> String {
    let mut r = rand::rng();
    (0..6)
        .map(|_| ALPHABET[r.random_range(0..ALPHABET.len())] as char)
        .collect()
}
fn unauthorized() -> AuthError {
    AuthError::new(StatusCode::UNAUTHORIZED, "unauthorized")
}
fn invalid(attempts: Option<i16>) -> AuthError {
    AuthError {
        status: StatusCode::BAD_REQUEST,
        message: "invalid or expired code",
        attempts,
    }
}
fn db_error(_: sqlx::Error) -> AuthError {
    tracing::error!("account database operation failed");
    AuthError::new(StatusCode::SERVICE_UNAVAILABLE, "accounts unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_and_hash_binding() {
        for _ in 0..100 {
            let c = random_code();
            assert_eq!(c.len(), 6);
            assert!(
                c.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            );
        }
        let id = Uuid::new_v4();
        assert_ne!(
            code_hash(b"secret", id, "a@b.com", "AAAAAA"),
            code_hash(b"secret", Uuid::new_v4(), "a@b.com", "AAAAAA")
        );
    }
}
