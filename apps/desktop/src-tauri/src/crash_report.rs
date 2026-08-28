use std::{
    fs,
    path::{Path, PathBuf},
};

use tauri::AppHandle;

use crate::models::settings_path;

const RUNNING_MARKER: &str = "running";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAX_CRASH_SNIPPET_CHARS: usize = 3_500;

/// Records this launch and, in Preview/release builds, reports a previous
/// dirty shutdown through the existing feedback channel.
pub fn initialize(app: &AppHandle, skip_because_update_restart: bool) {
    let dirty = take_dirty_shutdown() && !skip_because_update_restart;
    mark_session_started();
    if cfg!(debug_assertions) || !dirty {
        return;
    }
    crate::feedback::post_crash_report(app, crash_message());
}

pub fn mark_clean_exit() {
    let _ = fs::remove_file(session_marker_path());
}

fn mark_session_started() {
    let path = session_marker_path();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, RUNNING_MARKER);
}

fn session_marker_path() -> PathBuf {
    settings_path().with_file_name("session-running")
}

fn take_dirty_shutdown() -> bool {
    take_dirty_shutdown_at(&session_marker_path())
}

fn take_dirty_shutdown_at(path: &Path) -> bool {
    match fs::read_to_string(path) {
        Ok(contents) if contents.trim() == RUNNING_MARKER => {
            let _ = fs::remove_file(path);
            true
        }
        _ => false,
    }
}

fn crash_message() -> String {
    let mut message = String::from(
        "Captures closed unexpectedly. This is an automatic crash diagnostic; it does not include captures.",
    );
    if let Some(snippet) = latest_crash_snippet() {
        message.push_str("\n\n");
        message.push_str(&snippet);
    }
    message
}

fn latest_crash_snippet() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        latest_macos_crash_snippet()
    }
    #[cfg(not(target_os = "macos"))]
    None
}

#[cfg(target_os = "macos")]
fn latest_macos_crash_snippet() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let reports = PathBuf::from(home).join("Library/Logs/DiagnosticReports");
    let candidate = newest_captures_report(&reports)?;
    let contents = fs::read_to_string(&candidate).ok()?;
    summarize_crash_report(&contents)
}

#[cfg(target_os = "macos")]
fn newest_captures_report(directory: &Path) -> Option<PathBuf> {
    use std::time::SystemTime;
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    let entries = fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy();
        let lowered = name.to_ascii_lowercase();
        if !(lowered.starts_with("captures")
            && (lowered.ends_with(".ips") || lowered.ends_with(".crash")))
        {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())?;
        if newest
            .as_ref()
            .is_none_or(|(current, _)| modified > *current)
        {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn summarize_crash_report(contents: &str) -> Option<String> {
    let trimmed = contents.trim_start();
    let json_start = trimmed.find('{').unwrap_or(0);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&trimmed[json_start..]) {
        return summarize_ips_report(&value);
    }
    summarize_text_crash_report(contents)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn summarize_ips_report(value: &serde_json::Value) -> Option<String> {
    let exception = value
        .get("exception")
        .or_else(|| value.get("crash").and_then(|crash| crash.get("exception")));
    let exception_type = exception
        .and_then(|exception| exception.get("type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let signal = exception
        .and_then(|exception| exception.get("signal"))
        .or_else(|| exception.and_then(|exception| exception.get("codes")))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let thread_name = triggered_thread_name(value).unwrap_or("unknown");
    let asi = application_specific_information(value);
    let mut lines = vec![
        format!("Exception Type: {exception_type}"),
        format!("Triggered by Thread: {thread_name}"),
    ];
    if !signal.is_empty() {
        lines.insert(1, format!("Signal: {signal}"));
    }
    if let Some(asi) = asi {
        lines.push(format!("Application Specific Information: {asi}"));
    }
    Some(redact_user_paths(&lines.join("\n")))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn triggered_thread_name(value: &serde_json::Value) -> Option<&str> {
    let threads = value.get("threads")?.as_array()?;
    threads
        .iter()
        .find(|thread| thread.get("triggered").and_then(serde_json::Value::as_bool) == Some(true))
        .and_then(|thread| thread.get("name").and_then(serde_json::Value::as_str))
        .filter(|name| !name.is_empty())
        .or_else(|| {
            threads
                .iter()
                .find(|thread| {
                    thread.get("triggered").and_then(serde_json::Value::as_bool) == Some(true)
                })
                .and_then(|thread| thread.get("queue").and_then(serde_json::Value::as_str))
        })
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn application_specific_information(value: &serde_json::Value) -> Option<String> {
    let asi = value.get("asi")?;
    if let Some(text) = asi.as_str() {
        return Some(text.trim().to_owned());
    }
    let object = asi.as_object()?;
    let mut parts = Vec::new();
    for messages in object.values() {
        match messages {
            serde_json::Value::String(message) => parts.push(message.clone()),
            serde_json::Value::Array(items) => {
                for item in items {
                    if let Some(message) = item.as_str() {
                        parts.push(message.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    let joined = parts.join(" ");
    (!joined.trim().is_empty()).then_some(joined)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn summarize_text_crash_report(contents: &str) -> Option<String> {
    let mut lines = Vec::new();
    for prefix in [
        "Exception Type:",
        "Termination Reason:",
        "Triggered by Thread:",
        "Application Specific Information:",
    ] {
        if let Some(line) = contents
            .lines()
            .find(|line| line.trim_start().starts_with(prefix))
        {
            lines.push(line.trim().to_owned());
        }
    }
    if lines.is_empty() {
        return None;
    }
    let mut summary = lines.join("\n");
    if let Some(asi_index) = contents.find("Application Specific Information:") {
        let rest = contents[asi_index..].lines().skip(1).take(4);
        for line in rest {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if !trimmed.starts_with("Thread") {
                summary.push('\n');
                summary.push_str(trimmed);
            }
        }
    }
    let redacted = redact_user_paths(&summary);
    Some(truncate_chars(&redacted, MAX_CRASH_SNIPPET_CHARS))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn redact_user_paths(value: &str) -> String {
    redact_home_prefix(&redact_home_prefix(value, "/Users/"), "/home/")
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn redact_home_prefix(value: &str, prefix: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find(prefix) {
        output.push_str(&rest[..start]);
        output.push('~');
        let after = &rest[start + prefix.len()..];
        rest = after.split_once('/').map_or("", |(_, tail)| tail);
        if !rest.is_empty() {
            output.push('/');
        }
    }
    output.push_str(rest);
    output
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let kept: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RUNNING_MARKER, redact_user_paths, summarize_crash_report, take_dirty_shutdown_at,
    };

    #[test]
    fn dirty_shutdown_marker_is_consumed_once() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let marker = directory.path().join("session-running");
        std::fs::write(&marker, RUNNING_MARKER).expect("marker should be written");

        assert!(take_dirty_shutdown_at(&marker));
        assert!(!take_dirty_shutdown_at(&marker));
    }

    #[test]
    fn ignores_missing_or_clean_markers() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let marker = directory.path().join("session-running");
        assert!(!take_dirty_shutdown_at(&marker));
        std::fs::write(&marker, "clean").expect("marker should be written");
        assert!(!take_dirty_shutdown_at(&marker));
    }

    #[test]
    fn redacts_home_directory_paths() {
        assert_eq!(
            redact_user_paths("Path: /Users/jose/Library/Logs/DiagnosticReports/captures.ips"),
            "Path: ~/Library/Logs/DiagnosticReports/captures.ips"
        );
        assert_eq!(
            redact_user_paths("from /home/ubuntu/.local/share"),
            "from ~/.local/share"
        );
    }

    #[test]
    fn summarizes_translated_crash_text() {
        let report = "\
Process: captures [74083]
Exception Type: EXC_BREAKPOINT (SIGTRAP)
Termination Reason: Namespace SIGNAL, Code 5, Trace/BPT trap: 5
Triggered by Thread: 4 tokio-rt-worker
Application Specific Information:
Must only be used from the main thread
Thread 0:: main";
        let summary = summarize_crash_report(report).expect("summary");
        assert!(summary.contains("EXC_BREAKPOINT"));
        assert!(summary.contains("tokio-rt-worker"));
        assert!(summary.contains("Must only be used from the main thread"));
    }

    #[test]
    fn summarizes_ips_json_crash_reports() {
        let report = serde_json::json!({
            "exception": {
                "type": "EXC_BREAKPOINT",
                "signal": "SIGTRAP"
            },
            "asi": {
                "AppKit": ["Must only be used from the main thread"]
            },
            "threads": [
                { "name": "main", "triggered": false },
                { "name": "tokio-rt-worker", "triggered": true }
            ]
        });
        let summary = summarize_crash_report(&report.to_string()).expect("summary");
        assert!(summary.contains("EXC_BREAKPOINT"));
        assert!(summary.contains("SIGTRAP"));
        assert!(summary.contains("tokio-rt-worker"));
        assert!(summary.contains("Must only be used from the main thread"));
    }
}
