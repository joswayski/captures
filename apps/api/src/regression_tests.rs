use std::str::FromStr;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::Utc;
use sqlx::{ConnectOptions, postgres::PgPoolOptions};
use tower::ServiceExt;

#[tokio::test]
async fn account_requests_fail_closed_even_with_credentials() {
    for token in [None, Some("Bearer obsolete-token")] {
        let mut request = Request::get("/api/account/me");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, token);
        }
        let response = crate::router(None)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
}

#[tokio::test]
async fn health_remains_available() {
    for path in ["/health", "/api/health"] {
        let response = crate::router(None)
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn feedback_contract_validates_before_configuration() {
    let response = crate::router(None)
        .oneshot(
            Request::post("/api/feedback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"message":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = crate::router(None)
        .oneshot(
            Request::post("/api/feedback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"message":"Recording freezes"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn feedback_cors_contract_is_preserved() {
    let response = crate::router(None)
        .oneshot(
            Request::options("/api/feedback")
                .header(header::ORIGIN, "https://captur.es")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "https://captur.es"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL configured by TEST_DATABASE_URL"]
async fn postgres_users_and_startup_regressions() {
    let admin_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL server");
    let name = format!(
        "captures_test_{}_{}",
        std::process::id(),
        Utc::now().timestamp_micros().unsigned_abs()
    );
    let options = sqlx::postgres::PgConnectOptions::from_str(&admin_url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let test_url = options.database(&name).to_url_lossy().to_string();
    let result = async {
        let setup = crate::connect(&test_url).await?;
        let user_schema: String = sqlx::query_scalar("SELECT quote_ident(current_user)")
            .fetch_one(&setup)
            .await?;
        sqlx::query(&format!("CREATE SCHEMA {user_schema}"))
            .execute(&setup)
            .await?;
        sqlx::query("CREATE SCHEMA alternate")
            .execute(&setup)
            .await?;
        // The default "$user", public path now prefers the username schema.
        crate::migrate(&test_url)
            .await
            .expect("username schema migration");
        sqlx::query(&format!(
            "ALTER DATABASE {name} SET search_path = alternate, public"
        ))
        .execute(&setup)
        .await?;
        // New direct connections must override a database-level path as well.
        let (first, second) = tokio::join!(crate::migrate(&test_url), crate::migrate(&test_url));
        first.expect("first concurrent startup migration");
        second.expect("second concurrent startup migration");
        let migration_pool = crate::connect_for_migration(&test_url).await?;
        let migration_path: String = sqlx::query_scalar("SHOW search_path")
            .fetch_one(&migration_pool)
            .await?;
        assert_eq!(migration_path, "public");
        migration_pool.close().await;
        let misplaced_tables: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables
             WHERE table_name IN ('users', '_sqlx_migrations') AND table_schema <> 'public'",
        )
        .fetch_one(&setup)
        .await?;
        assert_eq!(misplaced_tables, 0);
        // Restore ordinary lookup for the runtime/manual-query checks below.
        // No CASCADE: these schemas must be empty after every migration attempt.
        sqlx::query(&format!("DROP SCHEMA {user_schema}, alternate"))
            .execute(&setup)
            .await?;
        sqlx::query(&format!("ALTER DATABASE {name} RESET search_path"))
            .execute(&setup)
            .await?;
        setup.close().await;
        let pool = crate::connect(&test_url).await?;
        let search_path: String = sqlx::query_scalar("SHOW search_path")
            .fetch_one(&pool)
            .await?;
        assert!(!search_path.contains("captures"));
        let schema: String = sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(&pool)
            .await?;
        assert_eq!(schema, "public");
        let custom_schema_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname='captures')",
        )
        .fetch_one(&pool)
        .await?;
        assert!(
            !custom_schema_exists,
            "startup must not create a custom schema"
        );
        let tables: Vec<(String, String)> = sqlx::query_as(
            "SELECT table_schema, table_name FROM information_schema.tables
             WHERE table_schema IN ('public', 'captures') ORDER BY table_schema, table_name",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            tables,
            vec![
                ("public".into(), "_sqlx_migrations".into()),
                ("public".into(), "users".into()),
            ]
        );
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns
             WHERE table_schema='public' AND table_name='users' ORDER BY ordinal_position",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            columns,
            vec![
                "id",
                "email",
                "email_verified",
                "created_at",
                "updated_at",
                "disabled_at",
                "deleted_at"
            ]
        );
        let (id, email, verified): (i64, Option<String>, bool) =
            sqlx::query_as("INSERT INTO users DEFAULT VALUES RETURNING id, email, email_verified")
                .fetch_one(&pool)
                .await?;
        assert!(id > 0);
        assert_eq!(email, None);
        assert!(!verified, "new users must not be implicitly verified");
        sqlx::query("UPDATE users SET email='test@example.com' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
        assert!(
            sqlx::query("UPDATE users SET deleted_at=now() WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query("UPDATE users SET email=NULL, deleted_at=now() WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
        crate::migrate(&test_url).await.expect("restart migration");
        let rows = sqlx::query("SELECT * FROM users").fetch_all(&pool).await?;
        assert_eq!(
            rows.len(),
            1,
            "restart must preserve the existing users table"
        );
        let migrations: i64 =
            sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(&pool)
                .await?;
        assert_eq!(migrations, 1);
        // An owner can also use the unqualified name for DDL, without changing
        // search_path. Roll back so the test leaves the migrated schema intact.
        let mut tx = pool.begin().await?;
        sqlx::query("DROP TABLE users").execute(&mut *tx).await?;
        tx.rollback().await?;
        pool.close().await;
        Ok::<_, sqlx::Error>(())
    }
    .await;
    sqlx::query(&format!("DROP DATABASE {name}"))
        .execute(&admin)
        .await
        .expect("drop owned test database");
    result.expect("database regressions");
}
