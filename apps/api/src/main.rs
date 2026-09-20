pub mod auth;
mod config;
mod email;
mod public_api;
mod sharing;
mod storage;

#[cfg(test)]
mod account_tests;
#[cfg(test)]
mod regression_tests;

use std::{str::FromStr, time::Duration};

use axum::{
    Json, Router,
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
    let discord_webhook_url = config.discord_webhook_url.clone();
    let auth_state = auth::AuthState::new(pool.clone(), config.auth).await;
    let store = config.storage.map(|config| {
        std::sync::Arc::new(storage::R2Store::new(config))
            as std::sync::Arc<dyn storage::ObjectStore>
    });
    let mut sharing_state = sharing::SharingState::new(auth_state.clone(), store);
    sharing_state.media_worker_secret = config.media_worker_secret;
    let cleanup = sharing_state.clone();
    let cleanup_task = tokio::spawn(async move {
        if cleanup.store.is_none() {
            return;
        }
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            if cleanup.cleanup().await.is_err() {
                tracing::warn!("sharing cleanup will retry");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|_| {
            eprintln!("server bind failed");
            std::process::exit(2)
        });
    tracing::info!(%bind, "captures API listening");
    axum::serve(
        listener,
        app_router(discord_webhook_url, auth_state, sharing_state)
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .unwrap_or_else(|_| eprintln!("server stopped unexpectedly"));
    cleanup_task.abort();
    pool.close().await;
}

#[cfg(test)]
fn router(discord_webhook_url: Option<String>) -> Router {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://test@127.0.0.1/test")
        .unwrap();
    let auth = auth::AuthState::disabled(pool);
    app_router(
        discord_webhook_url,
        auth.clone(),
        sharing::SharingState::new(auth, None),
    )
}

fn public_router(discord_webhook_url: Option<String>) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route(
            "/api/health",
            get(|| async { Json(json!({"status":"ok"})) }),
        )
        .route("/api/updates/preview", get(public_api::preview))
        .route(
            "/api/feedback",
            post(public_api::feedback).options(public_api::feedback_options),
        )
        .with_state(public_api::ApiState::new(discord_webhook_url))
}

fn app_router(
    discord_webhook_url: Option<String>,
    auth_state: auth::AuthState,
    sharing_state: sharing::SharingState,
) -> Router {
    public_router(discord_webhook_url)
        .merge(auth::router(auth_state))
        .merge(sharing::router(sharing_state))
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
    let result = tokio::time::timeout(Duration::from_secs(300), async {
        MIGRATOR.run(&pool).await.map_err(|_| ())?;
        backfill_user_external_ids(&pool).await.map_err(|_| ())
    })
    .await;
    // Close the DDL connection even on failure; runtime uses its own credentials.
    pool.close().await;
    result
        .map_err(|_| "database migration timed out")?
        .map_err(|_| "database migration failed")
}

async fn backfill_user_external_ids(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Migration's advisory lock is released before this application-data step.
    // A separate transaction lock serializes ID generation across API replicas.
    sqlx::query("SELECT pg_advisory_xact_lock(219398741223::bigint)")
        .execute(&mut *tx)
        .await?;
    let required: bool = sqlx::query_scalar("SELECT NOT attnotnull FROM pg_attribute WHERE attrelid='public.users'::regclass AND attname='external_id'")
        .fetch_one(&mut *tx).await?;
    if !required {
        return tx.commit().await;
    }
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE external_id IS NULL ORDER BY id FOR UPDATE")
            .fetch_all(&mut *tx)
            .await?;
    for id in ids {
        let mut assigned = false;
        for _ in 0..8 {
            let external_id = nanoid::nanoid!(12);
            let result = sqlx::query("UPDATE users SET external_id=$2 WHERE id=$1 AND external_id IS NULL AND NOT EXISTS(SELECT 1 FROM users WHERE external_id=$2)")
                .bind(id).bind(external_id).execute(&mut *tx).await?;
            if result.rows_affected() == 1 {
                assigned = true;
                break;
            }
            tracing::warn!(
                kind = "user_id",
                "public id collision during backfill; regenerating"
            );
        }
        if !assigned {
            return Err(sqlx::Error::RowNotFound);
        }
    }
    sqlx::query("ALTER TABLE users ALTER COLUMN external_id SET NOT NULL")
        .execute(&mut *tx)
        .await?;
    tx.commit().await
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
