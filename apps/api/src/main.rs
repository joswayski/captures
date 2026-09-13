mod config;
mod public_api;

#[cfg(test)]
mod regression_tests;

use std::{str::FromStr, time::Duration};

use axum::{
    Json, Router,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use config::Config;
use serde_json::json;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    if std::env::args().len() == 2 && std::env::args().nth(1).as_deref() == Some("migrate") {
        let url = config::database_url("MIGRATION_DATABASE_URL", true).unwrap_or_else(|e| {
            eprintln!("configuration error: {e}");
            std::process::exit(2)
        });
        migrate(&url).await.unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        });
        return;
    }
    if std::env::args().len() != 1 {
        eprintln!("usage: captures-api [migrate]");
        std::process::exit(2);
    }
    let config = Config::from_env().unwrap_or_else(|e| {
        eprintln!("configuration error: {e}");
        std::process::exit(2)
    });
    {
        let migration_url =
            config::database_url("MIGRATION_DATABASE_URL", true).unwrap_or_else(|e| {
                eprintln!("configuration error: {e}");
                std::process::exit(2)
            });
        config::validate_database_pair(&config.database_url, &migration_url).unwrap_or_else(|e| {
            eprintln!("configuration error: {e}");
            std::process::exit(2)
        });
        // Startup cannot listen or open its runtime pool until migration succeeds.
        migrate(&migration_url).await.unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        });
    }
    let pool = connect(&config.database_url).await.unwrap_or_else(|_| {
        eprintln!("database connection failed");
        std::process::exit(2)
    });
    let bind = config.bind;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|_| {
            eprintln!("server bind failed");
            std::process::exit(2)
        });
    tracing::info!(%bind, "captures API listening");
    axum::serve(listener, router(config.discord_webhook_url))
        .with_graceful_shutdown(shutdown())
        .await
        .unwrap_or_else(|_| eprintln!("server stopped unexpectedly"));
    pool.close().await;
}

fn router(discord_webhook_url: Option<String>) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route(
            "/api/health",
            get(|| async { Json(json!({"status":"ok"})) }),
        )
        .route("/api/account/me", get(account_unavailable))
        .route("/api/updates/preview", get(public_api::preview))
        .route(
            "/api/feedback",
            post(public_api::feedback).options(public_api::feedback_options),
        )
        .with_state(public_api::ApiState::new(discord_webhook_url))
}

async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    // Keep the default public search path; do not send pooler startup overrides.
    let options = PgConnectOptions::from_str(url)?;
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
}

async fn connect_for_migration(url: &str) -> Result<PgPool, sqlx::Error> {
    // Pin the direct connection: defaults can prefer a username or custom schema.
    // Runtime pooler connections must not receive this startup option.
    let options = PgConnectOptions::from_str(url)?.options([("search_path", "public")]);
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
}

async fn migrate(url: &str) -> Result<(), &'static str> {
    let pool = tokio::time::timeout(Duration::from_secs(300), connect_for_migration(url))
        .await
        .map_err(|_| "database migration connection timed out")?
        .map_err(|_| "database migration connection failed")?;
    let result = tokio::time::timeout(Duration::from_secs(300), MIGRATOR.run(&pool)).await;
    // Close the DDL connection even on failure; runtime uses its own credentials.
    pool.close().await;
    result
        .map_err(|_| "database migration timed out")?
        .map_err(|_| "database migration failed")
}

async fn account_unavailable() -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "Accounts are not available"})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
