#![forbid(unsafe_code)]

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower_http::{
    cors::{Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    trace::TraceLayer,
};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

const MAX_MESSAGE_LEN: usize = 8_000;
const MAX_CONTACT_LEN: usize = 200;
const MAX_META_LEN: usize = 128;
const MAX_BODY_BYTES: usize = 32 * 1024;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const RATE_LIMIT_MAX: u32 = 8;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    rate_limiter: Arc<parking_lot::Mutex<RateLimiter>>,
}

struct RateLimiter {
    entries: std::collections::HashMap<String, RateBucket>,
}

struct RateBucket {
    window_start: std::time::Instant,
    count: u32,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
        }
    }

    fn allow(&mut self, key: &str) -> bool {
        let now = std::time::Instant::now();
        self.entries.retain(|_, bucket| {
            now.duration_since(bucket.window_start) < RATE_LIMIT_WINDOW.saturating_mul(2)
        });
        let bucket = self.entries.entry(key.to_owned()).or_insert(RateBucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= RATE_LIMIT_WINDOW {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= RATE_LIMIT_MAX {
            return false;
        }
        bucket.count += 1;
        true
    }
}

#[derive(Debug, Deserialize)]
struct FeedbackRequest {
    message: String,
    #[serde(default)]
    contact: Option<String>,
    #[serde(default = "default_category")]
    category: String,
    #[serde(default)]
    app_version: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    os_version: Option<String>,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default = "default_source")]
    source: String,
}

fn default_category() -> String {
    "bug".to_owned()
}

fn default_source() -> String {
    "desktop".to_owned()
}

#[derive(Debug, Serialize)]
struct FeedbackResponse {
    id: i64,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("too many feedback submissions; try again shortly")]
    RateLimited,
    #[error("failed to store feedback")]
    Database,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Database => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorBody {
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL is required (Postgres connection string)")?;
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080);
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| format!("0.0.0.0:{port}"));

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&database_url)
        .await?;

    run_migrations(&pool).await?;

    let state = AppState {
        pool,
        rate_limiter: Arc::new(parking_lot::Mutex::new(RateLimiter::new())),
    };

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any)
        .allow_origin(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/feedback", post(create_feedback))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let addr: SocketAddr = bind.parse()?;
    info!("captures-api listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn create_feedback(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(payload): Json<FeedbackRequest>,
) -> Result<(StatusCode, Json<FeedbackResponse>), ApiError> {
    let rate_key = client_key(&headers, addr);
    if !state.rate_limiter.lock().allow(&rate_key) {
        return Err(ApiError::RateLimited);
    }

    let message = normalize_required(&payload.message, "message", 1, MAX_MESSAGE_LEN)?;
    let contact = normalize_optional(payload.contact.as_deref(), MAX_CONTACT_LEN)?;
    let category = normalize_category(&payload.category)?;
    let app_version = normalize_optional(payload.app_version.as_deref(), MAX_META_LEN)?;
    let os = normalize_optional(payload.os.as_deref(), MAX_META_LEN)?;
    let os_version = normalize_optional(payload.os_version.as_deref(), MAX_META_LEN)?;
    let arch = normalize_optional(payload.arch.as_deref(), MAX_META_LEN)?;
    let source = normalize_source(&payload.source)?;

    let row = sqlx::query_as::<_, (i64, DateTime<Utc>)>(
        r#"
        INSERT INTO feedback (
            message, contact, category, app_version, os, os_version, arch, source
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, created_at
        "#,
    )
    .bind(&message)
    .bind(contact.as_deref())
    .bind(&category)
    .bind(app_version.as_deref())
    .bind(os.as_deref())
    .bind(os_version.as_deref())
    .bind(arch.as_deref())
    .bind(&source)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| {
        warn!(%error, "failed to insert feedback");
        ApiError::Database
    })?;

    info!(id = row.0, category = %category, source = %source, "feedback stored");
    Ok((
        StatusCode::CREATED,
        Json(FeedbackResponse {
            id: row.0,
            created_at: row.1,
        }),
    ))
}

fn client_key(headers: &HeaderMap, addr: SocketAddr) -> String {
    forwarded_for(headers).unwrap_or_else(|| addr.ip().to_string())
}

fn forwarded_for(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())?;
    let first = value.split(',').next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_owned())
    }
}

fn normalize_required(
    value: &str,
    field: &str,
    min_len: usize,
    max_len: usize,
) -> Result<String, ApiError> {
    let trimmed = value.trim();
    if trimmed.chars().count() < min_len {
        return Err(ApiError::BadRequest(format!("{field} is required")));
    }
    if trimmed.chars().count() > max_len {
        return Err(ApiError::BadRequest(format!(
            "{field} must be at most {max_len} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

fn normalize_optional(value: Option<&str>, max_len: usize) -> Result<Option<String>, ApiError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max_len {
        return Err(ApiError::BadRequest(format!(
            "value must be at most {max_len} characters"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

fn normalize_category(value: &str) -> Result<String, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "bug" | "idea" | "other" => Ok(value.trim().to_ascii_lowercase()),
        _ => Err(ApiError::BadRequest(
            "category must be one of: bug, idea, other".to_owned(),
        )),
    }
}

fn normalize_source(value: &str) -> Result<String, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "desktop" | "web" => Ok(value.trim().to_ascii_lowercase()),
        _ => Err(ApiError::BadRequest(
            "source must be one of: desktop, web".to_owned(),
        )),
    }
}

async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    // Keep bootstrap simple: apply the checked-in SQL without a migration tool.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS feedback (
            id BIGSERIAL PRIMARY KEY,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            message TEXT NOT NULL,
            contact TEXT,
            category TEXT NOT NULL DEFAULT 'bug',
            app_version TEXT,
            os TEXT,
            os_version TEXT,
            arch TEXT,
            source TEXT NOT NULL DEFAULT 'desktop'
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS feedback_created_at_idx ON feedback (created_at DESC)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS feedback_category_idx ON feedback (category)")
        .execute(pool)
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                warn!(%error, "failed to install SIGTERM handler");
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    info!("shutdown signal received");
}
