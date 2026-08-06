#![forbid(unsafe_code)]

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
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
/// Discord embed description limit.
const DISCORD_DESCRIPTION_MAX: usize = 4_000;
/// One accepted submission per client IP per minute (in-memory; fine for a single pod).
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const DISCORD_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct AppState {
    discord_webhook_url: String,
    http: reqwest::Client,
    rate_limiter: Arc<parking_lot::Mutex<RateLimiter>>,
}

struct RateLimiter {
    /// Last accepted submission time per client key (IP / X-Forwarded-For).
    last_accepted: std::collections::HashMap<String, std::time::Instant>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            last_accepted: std::collections::HashMap::new(),
        }
    }

    fn can_accept(&mut self, key: &str) -> bool {
        let now = std::time::Instant::now();
        self.prune(now);
        self.last_accepted
            .get(key)
            .is_none_or(|last| now.duration_since(*last) >= RATE_LIMIT_COOLDOWN)
    }

    /// Atomically check and reserve a cooldown slot to close concurrent races.
    fn try_accept(&mut self, key: &str) -> bool {
        if !self.can_accept(key) {
            return false;
        }
        self.record(key);
        true
    }

    fn record(&mut self, key: &str) {
        let now = std::time::Instant::now();
        self.prune(now);
        self.last_accepted.insert(key.to_owned(), now);
    }

    fn prune(&mut self, now: std::time::Instant) {
        self.last_accepted
            .retain(|_, at| now.duration_since(*at) < RATE_LIMIT_COOLDOWN.saturating_mul(2));
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
    ok: bool,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug, Serialize)]
struct DiscordWebhookPayload {
    embeds: Vec<DiscordEmbed>,
}

#[derive(Debug, Serialize)]
struct DiscordEmbed {
    title: String,
    description: String,
    color: u32,
    fields: Vec<DiscordField>,
}

#[derive(Debug, Serialize)]
struct DiscordField {
    name: String,
    value: String,
    inline: bool,
}

#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("please wait a minute before sending more feedback")]
    RateLimited,
    #[error("failed to deliver feedback")]
    Delivery,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Delivery => StatusCode::BAD_GATEWAY,
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

    let discord_webhook_url = std::env::var("DISCORD_WEBHOOK_URL").map_err(
        |_| "DISCORD_WEBHOOK_URL is required (Discord channel webhook for feedback posts)",
    )?;
    if !discord_webhook_url.starts_with("https://discord.com/api/webhooks/")
        && !discord_webhook_url.starts_with("https://discordapp.com/api/webhooks/")
    {
        return Err("DISCORD_WEBHOOK_URL must be a Discord webhook URL".into());
    }

    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080);
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| format!("0.0.0.0:{port}"));

    let http = reqwest::Client::builder()
        .timeout(DISCORD_TIMEOUT)
        .user_agent(format!("captures-api/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let state = AppState {
        discord_webhook_url,
        http,
        rate_limiter: Arc::new(parking_lot::Mutex::new(RateLimiter::new())),
    };

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any)
        .allow_origin(Any);

    let app = Router::new()
        .route("/health", get(health))
        // Host is already the API (`api.captur.es`); no redundant `/api` prefix.
        .route("/feedback", post(create_feedback))
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
    // Validate first so empty/invalid payloads do not consume the cooldown.
    let message = normalize_required(&payload.message, "message", 1, MAX_MESSAGE_LEN)?;
    let contact = normalize_optional(payload.contact.as_deref(), MAX_CONTACT_LEN)?;
    let category = normalize_category(&payload.category)?;
    let app_version = normalize_optional(payload.app_version.as_deref(), MAX_META_LEN)?;
    let os = normalize_optional(payload.os.as_deref(), MAX_META_LEN)?;
    let os_version = normalize_optional(payload.os_version.as_deref(), MAX_META_LEN)?;
    let arch = normalize_optional(payload.arch.as_deref(), MAX_META_LEN)?;
    let source = normalize_source(&payload.source)?;

    let rate_key = client_key(&headers, addr);
    if !state.rate_limiter.lock().try_accept(&rate_key) {
        return Err(ApiError::RateLimited);
    }

    let discord_payload = build_discord_payload(&DeliveredFeedback {
        category: &category,
        message: &message,
        contact: contact.as_deref(),
        app_version: app_version.as_deref(),
        os: os.as_deref(),
        os_version: os_version.as_deref(),
        arch: arch.as_deref(),
        source: &source,
        client_key: &rate_key,
    });

    let response = state
        .http
        .post(&state.discord_webhook_url)
        .json(&discord_payload)
        .send()
        .await
        .map_err(|error| {
            warn!(%error, "discord webhook request failed");
            ApiError::Delivery
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        warn!(%status, body = %body.chars().take(300).collect::<String>(), "discord webhook rejected feedback");
        return Err(ApiError::Delivery);
    }

    info!(category = %category, source = %source, "feedback delivered to discord");
    Ok((StatusCode::CREATED, Json(FeedbackResponse { ok: true })))
}

struct DeliveredFeedback<'a> {
    category: &'a str,
    message: &'a str,
    contact: Option<&'a str>,
    app_version: Option<&'a str>,
    os: Option<&'a str>,
    os_version: Option<&'a str>,
    arch: Option<&'a str>,
    source: &'a str,
    client_key: &'a str,
}

fn build_discord_payload(feedback: &DeliveredFeedback<'_>) -> DiscordWebhookPayload {
    let title = match feedback.category {
        "idea" => "Idea",
        "other" => "Other feedback",
        _ => "Bug report",
    }
    .to_owned();
    let color = match feedback.category {
        "idea" => 0x58_a6_ff,  // blue
        "other" => 0x8b_94_9e, // gray
        _ => 0xef_46_50,       // red-ish
    };

    let mut fields = Vec::new();
    fields.push(DiscordField {
        name: "Category".to_owned(),
        value: feedback.category.to_owned(),
        inline: true,
    });
    fields.push(DiscordField {
        name: "Source".to_owned(),
        value: feedback.source.to_owned(),
        inline: true,
    });
    if let Some(version) = feedback.app_version {
        fields.push(DiscordField {
            name: "App".to_owned(),
            value: version.to_owned(),
            inline: true,
        });
    }
    let system = [feedback.os, feedback.os_version, feedback.arch]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    if !system.is_empty() {
        fields.push(DiscordField {
            name: "System".to_owned(),
            value: system,
            inline: true,
        });
    }
    if let Some(contact) = feedback.contact {
        fields.push(DiscordField {
            name: "Contact".to_owned(),
            value: contact.to_owned(),
            inline: true,
        });
    }
    fields.push(DiscordField {
        name: "Client".to_owned(),
        value: truncate(feedback.client_key, 64),
        inline: true,
    });

    DiscordWebhookPayload {
        embeds: vec![DiscordEmbed {
            title,
            description: truncate(feedback.message, DISCORD_DESCRIPTION_MAX),
            color,
            fields,
        }],
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = value.chars().take(keep).collect();
    out.push('…');
    out
}

/// Pick a rate-limit key for this request.
///
/// Do **not** trust raw `X-Forwarded-For`: clients can send anything there.
/// Behind Cloudflare → Railway, prefer `CF-Connecting-IP` (set by Cloudflare).
/// Fall back to the TCP peer address (usually Cloudflare or Railway, not the end user).
fn client_key(headers: &HeaderMap, addr: SocketAddr) -> String {
    if let Some(ip) = trusted_client_ip(headers) {
        return ip;
    }
    addr.ip().to_string()
}

fn trusted_client_ip(headers: &HeaderMap) -> Option<String> {
    // Cloudflare overwrites/sets this at the edge. Safe when origin traffic is
    // forced through Cloudflare (orange-cloud / authenticated origin pulls).
    // Spoofable only if an attacker can hit Railway *directly* while sending
    // this header — lock the origin down if that matters for you.
    single_ip_header(headers, "cf-connecting-ip")
        // Railway / some proxies set this from the trusted hop only.
        .or_else(|| single_ip_header(headers, "x-real-ip"))
}

fn single_ip_header(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    if value.is_empty() || value.contains(',') {
        return None;
    }
    // Basic shape check: reject obvious garbage / header injection attempts.
    if value.split('.').count() == 4 && value.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Some(value.to_owned());
    }
    if value.contains(':')
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
    {
        return Some(value.to_owned());
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn rate_limiter_allows_one_per_minute_per_key() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.try_accept("1.2.3.4"));
        assert!(!limiter.try_accept("1.2.3.4"));
        // Different IP is independent.
        assert!(limiter.try_accept("5.6.7.8"));
    }

    #[test]
    fn rate_limiter_recovers_after_cooldown() {
        let mut limiter = RateLimiter::new();
        limiter.record("client");
        assert!(!limiter.can_accept("client"));
        if let Some(at) = limiter.last_accepted.get_mut("client") {
            *at = std::time::Instant::now()
                .checked_sub(RATE_LIMIT_COOLDOWN + Duration::from_millis(5))
                .expect("instant should support subtraction");
        }
        thread::sleep(Duration::from_millis(2));
        assert!(limiter.can_accept("client"));
    }

    #[test]
    fn truncate_preserves_short_strings() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("abcdefghij", 5).chars().count(), 5);
        assert!(truncate("abcdefghij", 5).ends_with('…'));
    }

    #[test]
    fn trusted_client_ip_prefers_cloudflare_over_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        headers.insert("cf-connecting-ip", "203.0.113.9".parse().unwrap());
        assert_eq!(trusted_client_ip(&headers).as_deref(), Some("203.0.113.9"));
        // Spoofed XFF alone is ignored.
        headers.remove("cf-connecting-ip");
        assert_eq!(trusted_client_ip(&headers), None);
    }

    #[test]
    fn discord_payload_includes_message_and_meta() {
        let payload = build_discord_payload(&DeliveredFeedback {
            category: "bug",
            message: "Recording freezes",
            contact: Some("@jose"),
            app_version: Some("0.1.0"),
            os: Some("macos"),
            os_version: Some("15.5"),
            arch: Some("aarch64"),
            source: "desktop",
            client_key: "203.0.113.10",
        });
        assert_eq!(payload.embeds.len(), 1);
        assert_eq!(payload.embeds[0].title, "Bug report");
        assert_eq!(payload.embeds[0].description, "Recording freezes");
        assert!(
            payload.embeds[0]
                .fields
                .iter()
                .any(|field| field.name == "Contact" && field.value == "@jose")
        );
    }
}
