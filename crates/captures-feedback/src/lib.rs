//! Explicit, user-initiated native feedback. The client never collects files
//! or diagnostics and never sends at startup. Optional local crash evidence is
//! separately available through [`crash_diagnostics`] for review before consent.
//! Construct and call the blocking client on a worker, not a native UI thread.

pub mod crash_diagnostics;

use std::{
    io::Read,
    sync::Mutex,
    time::{Duration, Instant},
};

use reqwest::{blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};

pub const DEFAULT_FEEDBACK_URL: &str = "https://captur.es/api/feedback";
const COOLDOWN: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: u64 = 8_192;

#[derive(Clone, Debug, Deserialize)]
pub struct FeedbackDraft {
    pub message: String,
    pub contact: Option<String>,
    #[serde(default)]
    pub category: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FeedbackContext {
    pub app_version: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
}

#[derive(Serialize)]
struct Payload {
    message: String,
    contact: Option<String>,
    category: String,
    #[serde(flatten)]
    context: FeedbackContext,
    source: &'static str,
}

fn payload(draft: FeedbackDraft, context: FeedbackContext) -> Result<Payload, String> {
    let message = draft.message.trim().to_owned();
    if message.is_empty() {
        return Err("Please enter a short description of the issue or idea.".into());
    }
    if message.chars().count() > 8_000 {
        return Err("Feedback must be at most 8000 characters.".into());
    }
    let contact = draft
        .contact
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if contact
        .as_ref()
        .is_some_and(|value| value.chars().count() > 200)
    {
        return Err("Contact must be at most 200 characters.".into());
    }
    if [
        &context.app_version,
        &context.os,
        &context.os_version,
        &context.arch,
    ]
    .into_iter()
    .any(|value| value.chars().count() > 128)
    {
        return Err("Feedback app context must be at most 128 characters per field.".into());
    }
    let category = draft.category.trim().to_ascii_lowercase();
    let category = match category.as_str() {
        "bug" | "idea" | "other" | "crash" => category,
        _ => "bug".into(),
    };
    Ok(Payload {
        message,
        contact,
        category,
        context,
        source: "desktop",
    })
}

/// Keep one client for the app's lifetime so concurrent submit actions share
/// their cooldown. Failed requests may be retried immediately.
pub struct FeedbackClient {
    client: Client,
    endpoint: reqwest::Url,
    last_success: Mutex<Option<Instant>>,
}

impl FeedbackClient {
    /// No network request occurs here. HTTP is permitted for loopback test
    /// servers only; real feedback uses HTTPS and never follows redirects.
    pub fn new(endpoint: &str) -> Result<Self, String> {
        let endpoint = reqwest::Url::parse(endpoint).map_err(|_| "Invalid feedback endpoint.")?;
        let loopback = endpoint.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if (endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && loopback))
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            return Err("Feedback requires HTTPS (HTTP is allowed only on loopback).".into());
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(Policy::none())
            .user_agent(concat!("Captures-Native/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "Could not prepare the feedback request.")?;
        Ok(Self {
            client,
            endpoint,
            last_success: Mutex::new(None),
        })
    }

    pub fn submit(&self, draft: FeedbackDraft, context: FeedbackContext) -> Result<(), String> {
        let payload = payload(draft, context)?;
        // Serialize network submissions as well as the success timestamp: two
        // clicks cannot both pass the check before either request completes.
        let mut last = self
            .last_success
            .lock()
            .map_err(|_| "Feedback client is unavailable.")?;
        if let Some(last) = *last
            && last.elapsed() < COOLDOWN
        {
            return Err("Please wait a minute before sending more feedback.".into());
        }
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&payload)
            .send()
            .map_err(|error| {
                if error.is_timeout() {
                    "The feedback service timed out. Check your connection and try again."
                } else {
                    "Could not reach the feedback service. Check your connection and try again."
                }
            })?;
        let status = response.status();
        let mut body = String::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_string(&mut body)
            .map_err(|_| "Could not read the feedback service response.")?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err("The feedback service returned an oversized response.".into());
        }
        let data: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if status.is_success() {
            if data.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
                return Err("Feedback was not accepted. Please try again.".into());
            }
            *last = Some(Instant::now());
            return Ok(());
        }
        if status.as_u16() == 429 {
            return Err("Please wait a minute before sending more feedback.".into());
        }
        if let Some(message) = data.get("error").and_then(serde_json::Value::as_str)
            && !message.trim().is_empty()
        {
            return Err(message.chars().take(200).collect());
        }
        Err(format!(
            "Feedback service returned HTTP {}.",
            status.as_u16()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{Arc, Barrier},
        thread,
    };

    fn draft() -> FeedbackDraft {
        FeedbackDraft {
            message: "  Toolbar overlaps  ".into(),
            contact: Some("   ".into()),
            category: " IDEA ".into(),
        }
    }

    fn context() -> FeedbackContext {
        FeedbackContext {
            app_version: "0.1.0-native-test".into(),
            os: "linux".into(),
            os_version: "test".into(),
            arch: "x86_64".into(),
        }
    }

    fn server(
        responses: Vec<(&'static str, String)>,
    ) -> (String, thread::JoinHandle<Vec<serde_json::Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/feedback", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut length = None;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut bytes = vec![0; length.unwrap()];
                reader.read_exact(&mut bytes).unwrap();
                requests.push(serde_json::from_slice(&bytes).unwrap());
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        (url, handle)
    }

    #[test]
    fn concurrent_submissions_send_only_one_shipping_payload() {
        let (url, server) = server(vec![("201 Created", "{\"ok\":true}".into())]);
        let client = Arc::new(FeedbackClient::new(&url).unwrap());
        let ready = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let client = client.clone();
                let ready = ready.clone();
                thread::spawn(move || {
                    ready.wait();
                    client.submit(draft(), context())
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            results
                .into_iter()
                .find_map(Result::err)
                .unwrap()
                .contains("wait a minute")
        );
        let requests = server.join().unwrap();
        assert_eq!(
            requests,
            vec![
                serde_json::json!({"message":"Toolbar overlaps", "contact":null,
            "category":"idea", "source":"desktop", "app_version":"0.1.0-native-test",
            "os":"linux", "os_version":"test", "arch":"x86_64"})
            ]
        );
    }

    #[test]
    fn explicit_rejection_and_server_failure_allow_retry_but_not_false_success() {
        let (url, server) = server(vec![
            ("200 OK", "{\"ok\":false}".into()),
            ("400 Bad Request", "{\"error\":\"Invalid contact\"}".into()),
            ("204 No Content", String::new()),
        ]);
        let client = FeedbackClient::new(&url).unwrap();
        assert!(
            client
                .submit(draft(), context())
                .unwrap_err()
                .contains("not accepted")
        );
        assert_eq!(
            client.submit(draft(), context()).unwrap_err(),
            "Invalid contact"
        );
        client.submit(draft(), context()).unwrap();
        assert_eq!(server.join().unwrap().len(), 3);
    }

    #[test]
    fn rate_limits_redirects_and_oversized_bodies_cannot_report_success() {
        let (url, server) = server(vec![
            ("429 Too Many Requests", "{\"error\":\"Slow down\"}".into()),
            (
                "302 Found\r\nLocation: http://127.0.0.1:9/never-follow",
                String::new(),
            ),
            ("200 OK", "x".repeat(MAX_RESPONSE_BYTES as usize + 1)),
        ]);
        let client = FeedbackClient::new(&url).unwrap();
        assert!(
            client
                .submit(draft(), context())
                .unwrap_err()
                .contains("wait a minute")
        );
        assert_eq!(
            client.submit(draft(), context()).unwrap_err(),
            "Feedback service returned HTTP 302."
        );
        assert!(
            client
                .submit(draft(), context())
                .unwrap_err()
                .contains("oversized")
        );
        assert_eq!(server.join().unwrap().len(), 3);
    }

    #[test]
    fn validates_unicode_limits_and_never_treats_plain_http_as_production_transport() {
        for (count, valid) in [(8_000, true), (8_001, false)] {
            let mut input = draft();
            input.message = "é".repeat(count);
            assert_eq!(payload(input, context()).is_ok(), valid);
        }
        for (count, valid) in [(200, true), (201, false)] {
            let mut input = draft();
            input.contact = Some("é".repeat(count));
            assert_eq!(payload(input, context()).is_ok(), valid);
        }
        for (count, valid) in [(128, true), (129, false)] {
            let mut context = context();
            context.os_version = "é".repeat(count);
            assert_eq!(payload(draft(), context).is_ok(), valid);
        }
        let missing_category: FeedbackDraft =
            serde_json::from_str("{\"message\":\"Issue\"}").unwrap();
        assert_eq!(
            payload(missing_category, context()).unwrap().category,
            "bug"
        );
        let mut input = draft();
        input.message = " \n ".into();
        assert!(payload(input, context()).is_err());
        let mut input = draft();
        input.category = "unknown".into();
        assert_eq!(payload(input, context()).unwrap().category, "bug");
        assert!(FeedbackClient::new("http://example.com/api/feedback").is_err());
        assert!(FeedbackClient::new("https://user:secret@example.com/api/feedback").is_err());
        assert!(FeedbackClient::new(DEFAULT_FEEDBACK_URL).is_ok()); // Constructs only; no request.
    }
}
