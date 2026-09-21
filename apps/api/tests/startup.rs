use std::process::{Command, Output};

fn api() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_captures-api"));
    command
        .env_remove("DATABASE_URL")
        .env_remove("MIGRATION_DATABASE_URL")
        .env("AUTH_ENABLED", "false")
        .env("SHARING_ENABLED", "false")
        .env("CAPTURES_API_BIND", "127.0.0.1:0");
    command
}

fn failure(output: Output, expected: &str) {
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains(expected), "unexpected error: {error}");
    assert!(!error.contains("private-password"));
}

#[test]
fn migration_command_never_falls_back_to_runtime_credentials() {
    failure(
        api()
            .arg("migrate")
            .env(
                "DATABASE_URL",
                "postgres://app:private-password@127.0.0.1:1/captures",
            )
            .output()
            .unwrap(),
        "MIGRATION_DATABASE_URL is required",
    );
}

#[test]
fn startup_requires_the_migration_secret_and_rejects_the_pooler() {
    let runtime = "postgres://app:private-password@127.0.0.1:1/captures";
    failure(
        api().env("DATABASE_URL", runtime).output().unwrap(),
        "MIGRATION_DATABASE_URL is required",
    );
    failure(
        api()
            .env("DATABASE_URL", runtime)
            .env(
                "MIGRATION_DATABASE_URL",
                "postgres://owner:private-password@127.0.0.1:6432/captures",
            )
            .output()
            .unwrap(),
        "migrations require the direct PostgreSQL endpoint",
    );
}

#[test]
fn startup_does_not_serve_after_migration_connection_failure() {
    let unavailable = "postgres://app:private-password@127.0.0.1:1/captures";
    failure(
        api()
            .env("DATABASE_URL", unavailable)
            .env("MIGRATION_DATABASE_URL", unavailable)
            .output()
            .unwrap(),
        "database migration connection failed",
    );
}

#[test]
fn enabled_accounts_fail_startup_on_missing_or_invalid_settings() {
    failure(
        api()
            .env("AUTH_ENABLED", "true")
            .env_remove("AUTH_SECRET")
            .output()
            .unwrap(),
        "AUTH_SECRET is required",
    );
    failure(
        api()
            .env("AUTH_ENABLED", "true")
            .env("AUTH_SECRET", "short")
            .output()
            .unwrap(),
        "AUTH_SECRET must contain at least 32 bytes",
    );
    failure(
        api().env("SHARING_ENABLED", "true").output().unwrap(),
        "SHARING_ENABLED requires AUTH_ENABLED=true",
    );
    for missing in [
        "AUTH_ALLOWED_ORIGIN",
        "AWS_REGION",
        "SES_FROM_ADDRESS",
        "SES_CONFIGURATION_SET",
        "R2_ACCOUNT_ID",
        "R2_BUCKET",
        "R2_ACCESS_KEY_ID",
        "R2_SECRET_ACCESS_KEY",
        "MEDIA_WORKER_SECRET",
    ] {
        let mut command = api();
        command
            .env("AUTH_ENABLED", "true")
            .env("SHARING_ENABLED", "true")
            .env("AUTH_SECRET", "test-fixture-not-a-secret-32-bytes")
            .env("AUTH_ALLOWED_ORIGIN", "https://captur.es")
            .env("AUTH_INSECURE_LOOPBACK_COOKIE", "false")
            .env("AWS_REGION", "us-east-1")
            .env("SES_FROM_ADDRESS", "Captures <noreply@example.com>")
            .env("SES_CONFIGURATION_SET", "test-transactional")
            .env("R2_ACCOUNT_ID", "0123456789abcdef0123456789abcdef")
            .env("R2_BUCKET", "test-captures")
            .env("R2_ACCESS_KEY_ID", "test-key")
            .env("R2_SECRET_ACCESS_KEY", "test-key")
            .env("MEDIA_WORKER_SECRET", "test-fixture-not-a-secret-32-bytes")
            .env_remove(missing);
        failure(command.output().unwrap(), &format!("{missing} is required"));
    }
}
