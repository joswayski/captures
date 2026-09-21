use std::{env, net::SocketAddr};

#[derive(Clone)]
pub struct AuthConfig {
    pub enabled: bool,
    pub secret: String,
    pub ses_from: String,
    pub ses_configuration_set: Option<String>,
    pub allowed_origin: String,
    pub trust_cf_connecting_ip: bool,
    pub insecure_loopback_cookie: bool,
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind: SocketAddr,
    pub discord_webhook_url: Option<String>,
    pub auth: AuthConfig,
    pub storage: Option<StorageConfig>,
    pub media_worker_secret: Option<String>,
}

#[derive(Clone)]
pub struct StorageConfig {
    pub account_id: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let bind: SocketAddr = env::var("CAPTURES_API_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3001".into())
            .parse()
            .map_err(|_| "CAPTURES_API_BIND is invalid".to_string())?;
        let enabled = parse_bool("AUTH_ENABLED", false)?;
        let auth = if enabled {
            let secret = required("AUTH_SECRET")?;
            if secret.len() < 32 {
                return Err("AUTH_SECRET must contain at least 32 bytes".into());
            }
            if env::var("AWS_REGION")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .is_none()
            {
                return Err("AWS_REGION is required when AUTH_ENABLED=true".into());
            }
            let allowed_origin = required("AUTH_ALLOWED_ORIGIN")?;
            let insecure_loopback_cookie = parse_bool("AUTH_INSECURE_LOOPBACK_COOKIE", false)?;
            validate_origin(&allowed_origin, insecure_loopback_cookie)?;
            if insecure_loopback_cookie && !bind.ip().is_loopback() {
                return Err(
                    "AUTH_INSECURE_LOOPBACK_COOKIE requires a loopback CAPTURES_API_BIND".into(),
                );
            }
            AuthConfig {
                enabled,
                secret,
                ses_from: required("SES_FROM_ADDRESS")?,
                ses_configuration_set: Some(required("SES_CONFIGURATION_SET")?),
                allowed_origin,
                trust_cf_connecting_ip: parse_bool("AUTH_TRUST_CF_CONNECTING_IP", false)?,
                insecure_loopback_cookie,
            }
        } else {
            AuthConfig {
                enabled,
                secret: String::new(),
                ses_from: String::new(),
                ses_configuration_set: None,
                allowed_origin: String::new(),
                trust_cf_connecting_ip: false,
                insecure_loopback_cookie: false,
            }
        };
        let storage = if parse_bool("SHARING_ENABLED", false)? {
            if !enabled {
                return Err("SHARING_ENABLED requires AUTH_ENABLED=true".into());
            }
            let account_id = required("R2_ACCOUNT_ID")?;
            if account_id.len() != 32 || !account_id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("R2_ACCOUNT_ID must be a 32-character account ID".into());
            }
            let bucket = required("R2_BUCKET")?;
            if !(3..=63).contains(&bucket.len())
                || !bucket
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err("R2_BUCKET must be a lowercase bucket name".into());
            }
            Some(StorageConfig {
                account_id,
                bucket,
                access_key: required("R2_ACCESS_KEY_ID")?,
                secret_key: required("R2_SECRET_ACCESS_KEY")?,
            })
        } else {
            None
        };
        let media_worker_secret = if storage.is_some() {
            let secret = required("MEDIA_WORKER_SECRET")?;
            if secret.len() < 32 {
                return Err("MEDIA_WORKER_SECRET must contain at least 32 bytes".into());
            }
            Some(secret)
        } else {
            None
        };
        Ok(Self {
            database_url: database_url("DATABASE_URL", false)?,
            bind,
            discord_webhook_url: discord_webhook_url()?,
            auth,
            storage,
            media_worker_secret,
        })
    }
}

fn parse_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name).ok().as_deref() {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(_) => Err(format!("{name} must be true or false")),
    }
}

fn validate_origin(value: &str, insecure_loopback: bool) -> Result<(), String> {
    let url =
        reqwest::Url::parse(value).map_err(|_| "AUTH_ALLOWED_ORIGIN must be an origin URL")?;
    if insecure_loopback && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err("AUTH_INSECURE_LOOPBACK_COOKIE requires a loopback AUTH_ALLOWED_ORIGIN".into());
    }
    let local_http = insecure_loopback
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !local_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || value != url.origin().ascii_serialization()
    {
        return Err("AUTH_ALLOWED_ORIGIN must be an HTTPS origin without a path".into());
    }
    Ok(())
}

fn discord_webhook_url() -> Result<Option<String>, String> {
    let Some(value) = env::var("DISCORD_WEBHOOK_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let url = reqwest::Url::parse(&value)
        .map_err(|_| "DISCORD_WEBHOOK_URL must be a valid Discord HTTPS webhook URL".to_string())?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("discord.com" | "discordapp.com"))
        || !url.path().starts_with("/api/webhooks/")
    {
        return Err("DISCORD_WEBHOOK_URL must be a valid Discord HTTPS webhook URL".into());
    }
    Ok(Some(value))
}

fn required(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

pub fn database_url(name: &str, migration: bool) -> Result<String, String> {
    let value = required(name)?;
    validate_database_url(&value, migration).map_err(|reason| format!("{name}: {reason}"))?;
    Ok(value)
}

/// Check the original URL: SQLx's parsed options contain credentials and must
/// never be included in errors. Loopback-only development may use plaintext connections.
fn validate_database_url(value: &str, migration: bool) -> Result<reqwest::Url, &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid PostgreSQL URL")?;
    if !matches!(url.scheme(), "postgres" | "postgresql") || url.host_str().is_none() {
        return Err("expected a PostgreSQL URL with a host");
    }
    let database = url.path().strip_prefix('/').unwrap_or_default();
    if database.is_empty()
        || !database
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(
            "an explicit named database is required (letters, digits, underscores or hyphens)",
        );
    }
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !local && matches!(database, "postgres" | "template0" | "template1") {
        return Err("use the dedicated captures database, not a shared/default database");
    }
    if migration && (url.port() == Some(6432) || (!local && url.port().unwrap_or(5432) != 5432)) {
        return Err(
            "migrations require the direct PostgreSQL endpoint (5432), not PgBouncer (6432)",
        );
    }
    let mut sslmode = None;
    let mut sslrootcert = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "sslmode" if sslmode.is_none() => sslmode = Some(value.into_owned()),
            "sslrootcert" if sslrootcert.is_none() => sslrootcert = Some(value.into_owned()),
            "application_name" => {}
            // Reject aliases/overrides that could disagree with the checked host,
            // port, database or TLS settings rather than silently accepting them.
            _ => return Err("unsupported or duplicate connection URL parameter"),
        }
    }
    if !local && sslmode.as_deref() != Some("verify-full") {
        return Err("remote database connections require sslmode=verify-full");
    }
    if sslrootcert
        .as_deref()
        .is_some_and(|cert| cert.is_empty() || cert == "system")
    {
        return Err(
            "SQLx requires a CA PEM file for sslrootcert; omit it to use bundled public roots",
        );
    }
    Ok(url)
}

pub fn validate_database_pair(runtime: &str, migration: &str) -> Result<(), String> {
    let runtime =
        validate_database_url(runtime, false).map_err(|e| format!("DATABASE_URL: {e}"))?;
    let migration = validate_database_url(migration, true)
        .map_err(|e| format!("MIGRATION_DATABASE_URL: {e}"))?;
    if runtime.host_str() != migration.host_str() || runtime.path() != migration.path() {
        return Err(
            "DATABASE_URL and MIGRATION_DATABASE_URL must target the same host and named database"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_origins_are_exact_and_insecure_cookies_stay_local() {
        assert!(validate_origin("https://captur.es", false).is_ok());
        assert!(validate_origin("http://localhost:5174", true).is_ok());
        for (value, insecure) in [
            ("https://captur.es/", false),
            ("https://user@captur.es", false),
            ("https://captur.es/path", false),
            ("http://captur.es", false),
            ("http://localhost:5174", false),
            ("https://captur.es", true),
        ] {
            assert!(validate_origin(value, insecure).is_err(), "{value}");
        }
    }

    #[test]
    fn database_roles_share_a_database_but_not_a_pooler_endpoint() {
        let runtime = "postgres://app:password@db.example:6432/captures?sslmode=verify-full";
        let migration = "postgres://owner:password@db.example:5432/captures?sslmode=verify-full";
        assert!(validate_database_pair(runtime, migration).is_ok());
        assert!(validate_database_pair(runtime, runtime).is_err());
        assert!(
            validate_database_pair(runtime, &migration.replace("/captures?", "/caperchat?"))
                .is_err()
        );
        assert!(
            validate_database_pair(runtime, &migration.replace("db.example", "other.example"))
                .is_err()
        );
    }

    #[test]
    fn unsafe_or_ambiguous_remote_urls_are_rejected_without_secrets() {
        for value in [
            "not-a-url",
            "https://app:password@db.example/captures?sslmode=verify-full",
            "postgres://app:password@db.example/?sslmode=verify-full",
            "postgres://app:password@db.example/postgres?sslmode=verify-full",
            "postgres://app:password@db.example/captures?sslmode=require",
            "postgres://app:password@db.example/captures?sslmode=verify-full&sslmode=disable",
            "postgres://app:password@db.example/captures?sslmode=verify-full&sslrootcert=system",
            "postgres://app:password@localhost/captures?host=db.example",
        ] {
            let error = validate_database_url(value, true).unwrap_err();
            assert!(!error.contains("password"));
        }
        assert!(validate_database_url("postgres://app:password@db.example/captures?sslmode=verify-full&sslrootcert=/etc/ssl/certs/ca-certificates.crt", true).is_ok());
        assert!(
            validate_database_url("postgres://test@127.0.0.1:55432/captures_test", true).is_ok()
        );
        assert!(
            validate_database_url("postgres://test@127.0.0.1:6432/captures_test", true).is_err()
        );
    }
}
