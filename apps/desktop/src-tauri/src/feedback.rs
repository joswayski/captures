use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, webview::PageLoadEvent, window::Color,
};

use crate::CommandResult;

/// Production feedback endpoint. Override at runtime with `CAPTURES_FEEDBACK_URL`
/// (useful for pointing a local desktop build at `npm run dev:web`).
const DEFAULT_FEEDBACK_URL: &str = "https://captur.es/api/feedback";
const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(20);
const LOCAL_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const MAX_MESSAGE_LEN: usize = 8_000;
const MAX_CONTACT_LEN: usize = 200;

static LAST_SUCCESSFUL_SUBMIT: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FeedbackContext {
    pub app_version: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FeedbackDraft {
    pub message: String,
    #[serde(default)]
    pub contact: Option<String>,
    #[serde(default = "default_category")]
    pub category: String,
}

fn default_category() -> String {
    "bug".to_owned()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct FeedbackPayload {
    message: String,
    contact: Option<String>,
    category: String,
    app_version: String,
    os: String,
    os_version: String,
    arch: String,
    source: &'static str,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FeedbackSubmitResult {
    pub ok: bool,
}

#[tauri::command]
pub fn get_feedback_context(app: AppHandle) -> FeedbackContext {
    collect_feedback_context(&app)
}

#[tauri::command(async)]
pub fn open_feedback(app: AppHandle) -> CommandResult<()> {
    show_feedback(&app);
    Ok(())
}

#[tauri::command(async)]
pub async fn submit_feedback(
    app: AppHandle,
    draft: FeedbackDraft,
) -> CommandResult<FeedbackSubmitResult> {
    if let Some(message) = local_rate_limit_message() {
        return Err(message);
    }

    let message = draft.message.trim();
    if message.is_empty() {
        return Err("Please enter a short description of the issue or idea.".to_owned());
    }
    if message.chars().count() > MAX_MESSAGE_LEN {
        return Err(format!(
            "Feedback must be at most {MAX_MESSAGE_LEN} characters."
        ));
    }

    let contact = draft
        .contact
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if contact
        .as_ref()
        .is_some_and(|value| value.chars().count() > MAX_CONTACT_LEN)
    {
        return Err(format!(
            "Contact must be at most {MAX_CONTACT_LEN} characters."
        ));
    }

    let category = match draft.category.trim().to_ascii_lowercase().as_str() {
        "bug" | "idea" | "other" => draft.category.trim().to_ascii_lowercase(),
        _ => "bug".to_owned(),
    };

    let context = collect_feedback_context(&app);
    let payload = FeedbackPayload {
        message: message.to_owned(),
        contact,
        category,
        app_version: context.app_version,
        os: context.os,
        os_version: context.os_version,
        arch: context.arch,
        source: "desktop",
    };

    let endpoint = feedback_endpoint();
    let client = reqwest::Client::builder()
        .timeout(FEEDBACK_TIMEOUT)
        .user_agent(format!("Captures/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Couldn’t prepare the feedback request: {error}"))?;

    let response = client
        .post(&endpoint)
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "The feedback service timed out. Check your connection and try again.".to_owned()
            } else if error.is_connect() {
                "Couldn’t reach the feedback service. Check your connection and try again."
                    .to_owned()
            } else {
                format!("Couldn’t send feedback: {error}")
            }
        })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status.is_success() {
        // Prefer structured `{ ok: true }`, but accept any 2xx so a bare proxy still works.
        if let Ok(parsed) = serde_json::from_str::<FeedbackSubmitResult>(&body)
            && !parsed.ok
        {
            return Err("Feedback was not accepted. Please try again.".to_owned());
        }
        mark_local_rate_limit();
        return Ok(FeedbackSubmitResult { ok: true });
    }

    let detail = parse_api_error(&body).unwrap_or_else(|| {
        if body.trim().is_empty() {
            format!("Feedback service returned HTTP {status}")
        } else {
            body.trim().chars().take(200).collect()
        }
    });
    Err(match status.as_u16() {
        429 => "Please wait a minute before sending more feedback.".to_owned(),
        400 => detail,
        _ => format!("Couldn’t send feedback ({status}): {detail}"),
    })
}

fn local_rate_limit_message() -> Option<String> {
    let guard = LAST_SUCCESSFUL_SUBMIT.lock().ok()?;
    let last = (*guard)?;
    let elapsed = last.elapsed();
    if elapsed >= LOCAL_RATE_LIMIT_COOLDOWN {
        return None;
    }
    let remaining = LOCAL_RATE_LIMIT_COOLDOWN
        .saturating_sub(elapsed)
        .as_secs()
        .max(1);
    Some(format!(
        "Please wait {remaining}s before sending more feedback."
    ))
}

fn mark_local_rate_limit() {
    if let Ok(mut guard) = LAST_SUCCESSFUL_SUBMIT.lock() {
        *guard = Some(Instant::now());
    }
}

pub fn show_feedback(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("feedback") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let result = WebviewWindowBuilder::new(
            &handle,
            "feedback",
            WebviewUrl::App("index.html?view=feedback".into()),
        )
        .title("Send Feedback")
        .inner_size(520.0, 620.0)
        .min_inner_size(440.0, 520.0)
        .center()
        .resizable(true)
        .background_color(Color(11, 11, 13, 255))
        .focused(false)
        .visible(false)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished
                && let Err(error) = window.show().and_then(|_| window.set_focus())
            {
                eprintln!("failed to reveal feedback window: {error}");
            }
        })
        .build();
        if let Err(error) = result {
            eprintln!("failed to show feedback window: {error}");
        }
    });
}

fn feedback_endpoint() -> String {
    std::env::var("CAPTURES_FEEDBACK_URL").unwrap_or_else(|_| DEFAULT_FEEDBACK_URL.to_owned())
}

fn collect_feedback_context(app: &AppHandle) -> FeedbackContext {
    let package = app.package_info();
    FeedbackContext {
        app_version: package.version.to_string(),
        os: std::env::consts::OS.to_owned(),
        os_version: detect_os_version(),
        arch: std::env::consts::ARCH.to_owned(),
    }
}

fn detect_os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            && let Ok(version) = String::from_utf8(output.stdout)
        {
            let trimmed = version.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
        "macOS".to_owned()
    }

    #[cfg(target_os = "windows")]
    {
        // Prefer the marketing-friendly OS env when present; fall back to a stable label.
        std::env::var("OS").unwrap_or_else(|_| "Windows".to_owned())
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(contents) = std::fs::read_to_string("/etc/os-release") {
            for line in contents.lines() {
                if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                    let trimmed = value.trim().trim_matches('"');
                    if !trimmed.is_empty() {
                        return trimmed.to_owned();
                    }
                }
            }
        }
        "Linux".to_owned()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        "unknown".to_owned()
    }
}

fn parse_api_error(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ApiErr {
        error: String,
    }
    serde_json::from_str::<ApiErr>(body)
        .ok()
        .map(|value| value.error)
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_endpoint_is_production_api() {
        // Ensure packaged builds have a stable default when the env var is unset.
        // Runtime env is not forced here; this locks the constant itself.
        assert_eq!(DEFAULT_FEEDBACK_URL, "https://captur.es/api/feedback");
    }

    #[test]
    fn parse_api_error_reads_json_body() {
        assert_eq!(
            parse_api_error(r#"{"error":"message is required"}"#).as_deref(),
            Some("message is required")
        );
        assert!(parse_api_error("not-json").is_none());
    }
}
