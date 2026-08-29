use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use tauri::AppHandle;

use crate::models::settings_path;

const RUNNING_MARKER: &str = "running";
const LAST_PANIC_FILE: &str = "last-panic";
const MAX_CRASH_SNIPPET_CHARS: usize = 3_500;
const MAX_STACK_FRAMES: usize = 12;
const MAX_BACKTRACE_LINES: usize = 32;
const REPORT_SKEW: Duration = Duration::from_secs(5);
const STALE_REPORT_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const BOILERPLATE: &str = "Captures closed unexpectedly. This is an automatic crash diagnostic; it does not include captures.";
const MISSING_EXCEPTION: &str = "No exception summary was available. The process did not shut down cleanly, and no panic or OS crash report was found.";

struct DirtyShutdown {
    started_at: Option<SystemTime>,
}

/// Records this launch and, in Preview/release builds, reports a previous
/// dirty shutdown through the existing feedback channel.
pub fn initialize(app: &AppHandle, skip_because_update_restart: bool) {
    let dirty = take_dirty_shutdown();
    let started_at = dirty.as_ref().and_then(|value| value.started_at);
    let should_report = dirty.is_some() && !skip_because_update_restart;
    mark_session_started();
    if cfg!(debug_assertions) || skip_because_update_restart {
        let _ = take_last_panic();
        return;
    }
    if !should_report && !last_panic_path().is_file() {
        return;
    }
    crate::feedback::post_crash_report(app, crash_message(started_at));
}

/// Persist Rust panics so the next launch can include the error in diagnostics.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_last_panic(&format_panic_from_info(info));
        previous(info);
    }));
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

fn last_panic_path() -> PathBuf {
    settings_path().with_file_name(LAST_PANIC_FILE)
}

fn take_dirty_shutdown() -> Option<DirtyShutdown> {
    take_dirty_shutdown_at(&session_marker_path())
}

fn take_dirty_shutdown_at(path: &Path) -> Option<DirtyShutdown> {
    match fs::read_to_string(path) {
        Ok(contents) if is_session_marker(&contents) => {
            let started_at = fs::metadata(path)
                .ok()
                .and_then(|meta| meta.modified().ok());
            let _ = fs::remove_file(path);
            Some(DirtyShutdown { started_at })
        }
        _ => None,
    }
}

fn is_session_marker(contents: &str) -> bool {
    contents.trim() == RUNNING_MARKER
}

fn crash_message(started_at: Option<SystemTime>) -> String {
    format_crash_message(collect_crash_details(started_at))
}

fn collect_crash_details(started_at: Option<SystemTime>) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(panic) = take_last_panic() {
        details.push(panic);
    }
    if let Some(os_report) = latest_os_crash_snippet(started_at) {
        details.push(os_report);
    }
    details
}

fn format_crash_message(details: Vec<String>) -> String {
    let mut message = String::from(BOILERPLATE);
    message.push_str("\n\n");
    if details.is_empty() {
        message.push_str(MISSING_EXCEPTION);
    } else {
        message.push_str(&truncate_chars(
            &details.join("\n\n"),
            MAX_CRASH_SNIPPET_CHARS,
        ));
    }
    message
}

fn latest_os_crash_snippet(started_at: Option<SystemTime>) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        latest_macos_crash_snippet(started_at)
    }
    #[cfg(target_os = "windows")]
    {
        latest_windows_crash_snippet(started_at)
    }
    #[cfg(target_os = "linux")]
    {
        latest_linux_crash_snippet(started_at)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = started_at;
        None
    }
}

fn format_panic_from_info(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unknown");
    let location = info
        .location()
        .map_or_else(|| "unknown".to_owned(), ToString::to_string);
    let payload = info.payload_as_str().unwrap_or("Box<dyn Any>");
    let backtrace = std::backtrace::Backtrace::force_capture();
    format_panic_report(thread_name, &location, payload, &backtrace.to_string())
}

fn format_panic_report(
    thread_name: &str,
    location: &str,
    payload: &str,
    backtrace: &str,
) -> String {
    let mut body = format!("Panic:\nthread '{thread_name}' panicked at {location}:\n{payload}");
    let trace = truncate_lines(backtrace.trim(), MAX_BACKTRACE_LINES);
    if !trace.is_empty() {
        body.push_str("\n\n");
        body.push_str(&trace);
    }
    redact_user_paths(&truncate_chars(&body, MAX_CRASH_SNIPPET_CHARS))
}

fn write_last_panic(body: &str) {
    let path = last_panic_path();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, body);
}

fn take_last_panic() -> Option<String> {
    take_last_panic_at(&last_panic_path())
}

fn take_last_panic_at(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let _ = fs::remove_file(path);
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(redact_user_paths(trimmed))
}

#[cfg(target_os = "macos")]
fn latest_macos_crash_snippet(started_at: Option<SystemTime>) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let reports = PathBuf::from(home).join("Library/Logs/DiagnosticReports");
    let candidate = newest_file_matching(std::iter::once(reports), started_at, |name| {
        let lowered = name.to_ascii_lowercase();
        lowered.starts_with("captures")
            && (lowered.ends_with(".ips") || lowered.ends_with(".crash"))
    })?;
    let contents = read_report_text(&candidate)?;
    summarize_os_report(&contents)
}

#[cfg(target_os = "windows")]
fn latest_windows_crash_snippet(started_at: Option<SystemTime>) -> Option<String> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let wer = local.join("Microsoft").join("Windows").join("WER");
    latest_wer_snippet_from(
        [wer.join("ReportArchive"), wer.join("ReportQueue")],
        started_at,
    )
}

#[cfg(target_os = "linux")]
fn latest_linux_crash_snippet(started_at: Option<SystemTime>) -> Option<String> {
    let mut directories = vec![PathBuf::from("/var/crash")];
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".cache").join("apport"));
    }
    let candidate = newest_file_matching(directories, started_at, |name| {
        let lowered = name.to_ascii_lowercase();
        lowered.contains("captures") && (lowered.ends_with(".crash") || lowered.ends_with(".ips"))
    })?;
    let contents = read_report_text(&candidate)?;
    summarize_os_report(&contents)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn latest_wer_snippet_from(
    directories: impl IntoIterator<Item = PathBuf>,
    started_at: Option<SystemTime>,
) -> Option<String> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            if !name
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("captures")
            {
                continue;
            }
            let report = path.join("Report.wer");
            if !report.is_file() {
                continue;
            }
            let Some(modified) = report
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .or_else(|| entry.metadata().ok().and_then(|meta| meta.modified().ok()))
            else {
                continue;
            };
            if !report_is_recent(modified, started_at) {
                continue;
            }
            if newest
                .as_ref()
                .is_none_or(|(current, _)| modified > *current)
            {
                newest = Some((modified, report));
            }
        }
    }
    let (_, path) = newest?;
    let contents = read_report_text(&path)?;
    summarize_wer_report(&contents)
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
fn newest_file_matching(
    directories: impl IntoIterator<Item = PathBuf>,
    started_at: Option<SystemTime>,
    mut matches: impl FnMut(&str) -> bool,
) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            if !matches(&name.to_string_lossy()) {
                continue;
            }
            let Some(modified) = entry.metadata().ok().and_then(|meta| meta.modified().ok()) else {
                continue;
            };
            if !report_is_recent(modified, started_at) {
                continue;
            }
            if newest
                .as_ref()
                .is_none_or(|(current, _)| modified > *current)
            {
                newest = Some((modified, path));
            }
        }
    }
    newest.map(|(_, path)| path)
}

fn report_is_recent(modified: SystemTime, started_at: Option<SystemTime>) -> bool {
    if let Some(started) = started_at {
        let floor = started.checked_sub(REPORT_SKEW).unwrap_or(started);
        return modified >= floor;
    }
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age <= STALE_REPORT_AGE)
}

fn read_report_text(path: &Path) -> Option<String> {
    decode_report_bytes(&fs::read(path).ok()?)
}

fn decode_report_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Some(decode_utf16(&bytes[2..], true));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Some(decode_utf16(&bytes[2..], false));
    }
    let rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if looks_like_utf16_le(rest) {
        return Some(decode_utf16(rest, true));
    }
    Some(String::from_utf8_lossy(rest).into_owned())
}

fn looks_like_utf16_le(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0] != 0 && bytes[1] == 0 && bytes[2] != 0 && bytes[3] == 0
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

fn summarize_os_report(contents: &str) -> Option<String> {
    let trimmed = contents.trim_start().trim_start_matches('\u{feff}');
    let json_start = trimmed.find('{').unwrap_or(0);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&trimmed[json_start..]) {
        return summarize_ips_report(&value);
    }
    if looks_like_wer(trimmed) {
        return summarize_wer_report(trimmed);
    }
    if apport_field(trimmed, "ProblemType").is_some() {
        return summarize_apport_report(trimmed);
    }
    summarize_text_crash_report(contents)
}

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
    if let Some(reason) = termination_reason(value) {
        lines.push(reason);
    }
    if let Some(asi) = asi {
        lines.push(format!("Application Specific Information: {asi}"));
    }
    let frames = crashing_thread_frames(value);
    if !frames.is_empty() {
        lines.push("Crashed thread:".to_owned());
        lines.extend(frames);
    }
    Some(redact_user_paths(&lines.join("\n")))
}

fn termination_reason(value: &serde_json::Value) -> Option<String> {
    let termination = value.get("termination")?;
    let namespace = termination
        .get("namespace")
        .and_then(serde_json::Value::as_str);
    let indicator = termination
        .get("indicator")
        .and_then(serde_json::Value::as_str);
    match (namespace, indicator) {
        (Some(namespace), Some(indicator)) => Some(format!(
            "Termination Reason: Namespace {namespace}, {indicator}"
        )),
        (None, Some(indicator)) => Some(format!("Termination Reason: {indicator}")),
        (Some(namespace), None) => Some(format!("Termination Reason: {namespace}")),
        _ => None,
    }
}

fn triggered_thread(value: &serde_json::Value) -> Option<&serde_json::Value> {
    let threads = value.get("threads")?.as_array()?;
    threads
        .iter()
        .find(|thread| thread.get("triggered").and_then(serde_json::Value::as_bool) == Some(true))
        .or_else(|| {
            let index = value
                .get("faultingThread")
                .and_then(serde_json::Value::as_u64)?;
            threads.get(index as usize)
        })
}

fn triggered_thread_name(value: &serde_json::Value) -> Option<&str> {
    let thread = triggered_thread(value)?;
    thread
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .or_else(|| thread.get("queue").and_then(serde_json::Value::as_str))
}

fn crashing_thread_frames(value: &serde_json::Value) -> Vec<String> {
    let Some(thread) = triggered_thread(value) else {
        return Vec::new();
    };
    let Some(frames) = thread.get("frames").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let images = value
        .get("usedImages")
        .and_then(serde_json::Value::as_array);
    frames
        .iter()
        .take(MAX_STACK_FRAMES)
        .enumerate()
        .map(|(index, frame)| {
            let image = frame
                .get("imageIndex")
                .and_then(serde_json::Value::as_u64)
                .and_then(|image_index| images.and_then(|images| images.get(image_index as usize)))
                .and_then(|image| image.get("name").and_then(serde_json::Value::as_str))
                .unwrap_or("???");
            let symbol = frame.get("symbol").and_then(serde_json::Value::as_str);
            let offset = frame
                .get("symbolLocation")
                .or_else(|| frame.get("imageOffset"))
                .and_then(serde_json::Value::as_u64);
            match (symbol, offset) {
                (Some(symbol), Some(offset)) => {
                    format!("{index:>2}  {image}  {symbol} + {offset}")
                }
                (Some(symbol), None) => format!("{index:>2}  {image}  {symbol}"),
                (None, Some(offset)) => format!("{index:>2}  {image}  0x{offset:x}"),
                (None, None) => format!("{index:>2}  {image}"),
            }
        })
        .collect()
}

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
    if let Some(asi_index) = contents.find("Application Specific Information:") {
        let rest = contents[asi_index..].lines().skip(1).take(4);
        for line in rest {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if !trimmed.starts_with("Thread") {
                lines.push(trimmed.to_owned());
            }
        }
    }
    let stack = crashed_thread_stack(contents);
    if !stack.is_empty() {
        lines.push("Crashed thread:".to_owned());
        lines.extend(stack);
    }
    if lines.is_empty() {
        return None;
    }
    let summary = lines.join("\n");
    Some(truncate_chars(
        &redact_user_paths(&summary),
        MAX_CRASH_SNIPPET_CHARS,
    ))
}

fn crashed_thread_stack(contents: &str) -> Vec<String> {
    let Some(start) = contents.lines().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("Thread ") && trimmed.contains("Crashed")
    }) else {
        return Vec::new();
    };
    contents
        .lines()
        .skip(start + 1)
        .map(str::trim)
        .take_while(|line| !line.is_empty() && !line.starts_with("Thread "))
        .take(MAX_STACK_FRAMES)
        .map(ToOwned::to_owned)
        .collect()
}

fn looks_like_wer(contents: &str) -> bool {
    let lowered = contents.to_ascii_lowercase();
    lowered.contains("eventtype=")
        || lowered.contains("sig[0].name=")
        || lowered.contains("friendlyeventname=")
}

fn summarize_wer_report(contents: &str) -> Option<String> {
    let sigs = parse_wer_sigs(contents);
    let event = ini_field(contents, "EventType")
        .or_else(|| ini_field(contents, "FriendlyEventName"))
        .or_else(|| ini_field(contents, "EventName"))
        .unwrap_or("APPCRASH");
    let application = wer_sig_value(&sigs, "Application Name");
    let module = wer_sig_value(&sigs, "Fault Module Name");
    let exception = wer_sig_value(&sigs, "Exception Code");
    let offset = wer_sig_value(&sigs, "Exception Offset");
    if application.is_none() && exception.is_none() && module.is_none() {
        return None;
    }

    let mut lines = vec![
        "Windows Error Reporting:".to_owned(),
        format!("Event Type: {event}"),
    ];
    if let Some(application) = application {
        lines.push(format!("Application: {application}"));
    }
    if let Some(exception) = exception {
        lines.push(format!("Exception: {}", describe_ntstatus(exception)));
    }
    if let Some(module) = module {
        lines.push(format!("Fault module: {module}"));
    }
    if let Some(offset) = offset {
        lines.push(format!("Exception offset: {offset}"));
    }
    Some(redact_user_paths(&lines.join("\n")))
}

struct WerSig {
    name: String,
    value: String,
}

fn parse_wer_sigs(contents: &str) -> Vec<WerSig> {
    let mut names = Vec::<(u32, String)>::new();
    let mut values = Vec::<(u32, String)>::new();
    for line in contents.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        let Some(rest) = line.strip_prefix("Sig[") else {
            continue;
        };
        let Some((index_text, rest)) = rest.split_once(']') else {
            continue;
        };
        let Ok(index) = index_text.parse::<u32>() else {
            continue;
        };
        if let Some(name) = rest.strip_prefix(".Name=") {
            names.push((index, name.to_owned()));
        } else if let Some(value) = rest.strip_prefix(".Value=") {
            values.push((index, value.to_owned()));
        }
    }
    let mut sigs = Vec::new();
    for (index, name) in names {
        let value = values
            .iter()
            .find(|(value_index, _)| *value_index == index)
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        sigs.push(WerSig { name, value });
    }
    sigs
}

fn wer_sig_value<'a>(sigs: &'a [WerSig], name: &str) -> Option<&'a str> {
    sigs.iter()
        .find(|sig| sig.name.eq_ignore_ascii_case(name))
        .map(|sig| sig.value.as_str())
        .filter(|value| !value.is_empty())
}

fn ini_field<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    contents.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix(&prefix)
            .or_else(|| {
                let lowered = line.to_ascii_lowercase();
                let needle = prefix.to_ascii_lowercase();
                lowered.starts_with(&needle).then(|| &line[prefix.len()..])
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn describe_ntstatus(code: &str) -> String {
    let normalized = code.trim().trim_start_matches("0x").to_ascii_lowercase();
    let name = match normalized.as_str() {
        "c0000005" => Some("STATUS_ACCESS_VIOLATION"),
        "c00000fd" => Some("STATUS_STACK_OVERFLOW"),
        "80000003" => Some("STATUS_BREAKPOINT"),
        "c0000409" => Some("STATUS_STACK_BUFFER_OVERRUN"),
        "c000001d" => Some("STATUS_ILLEGAL_INSTRUCTION"),
        "40000015" => Some("STATUS_FATAL_APP_EXIT"),
        "c000013a" => Some("STATUS_CONTROL_C_EXIT"),
        "c0000374" => Some("STATUS_HEAP_CORRUPTION"),
        "c0000094" => Some("STATUS_INTEGER_DIVIDE_BY_ZERO"),
        _ => None,
    };
    match name {
        Some(name) => format!("0x{normalized} ({name})"),
        None => {
            if normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
                format!("0x{normalized}")
            } else {
                code.trim().to_owned()
            }
        }
    }
}

fn summarize_apport_report(contents: &str) -> Option<String> {
    let mut lines = vec!["Linux crash report:".to_owned()];
    if let Some(signal) = apport_field(contents, "Signal") {
        lines.push(format!("Signal: {signal} ({})", signal_name(signal)));
    }
    if let Some(executable) = apport_field(contents, "ExecutablePath") {
        lines.push(format!("Executable: {executable}"));
    }
    if let Some(message) = apport_field(contents, "AssertionMessage") {
        lines.push(format!("Assertion: {message}"));
    }
    if let Some(reason) = apport_field(contents, "UnreportableReason") {
        lines.push(format!("Reason: {reason}"));
    }
    let stack = apport_section(contents, "Stacktrace");
    if !stack.is_empty() {
        lines.push("Stacktrace:".to_owned());
        lines.extend(
            stack
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(MAX_STACK_FRAMES)
                .map(ToOwned::to_owned),
        );
    }
    if lines.len() == 1 {
        return None;
    }
    Some(truncate_chars(
        &redact_user_paths(&lines.join("\n")),
        MAX_CRASH_SNIPPET_CHARS,
    ))
}

fn apport_field<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    contents.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn apport_section<'a>(contents: &'a str, key: &str) -> &'a str {
    let header = format!("{key}:");
    let Some(start) = contents.find(&header) else {
        return "";
    };
    let after = &contents[start + header.len()..];
    let end = after.find("\n\n").unwrap_or(after.len());
    after[..end].trim()
}

fn signal_name(code: &str) -> &'static str {
    match code.trim() {
        "4" => "SIGILL",
        "5" => "SIGTRAP",
        "6" => "SIGABRT",
        "8" => "SIGFPE",
        "9" => "SIGKILL",
        "11" => "SIGSEGV",
        _ => "unknown",
    }
}

fn redact_user_paths(value: &str) -> String {
    let unix = redact_home_prefix(&redact_home_prefix(value, "/Users/"), "/home/");
    redact_windows_user_path(&redact_windows_user_path(&unix, "\\Users\\"), "/Users/")
}

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

fn redact_windows_user_path(value: &str, users_segment: &str) -> String {
    let needle = users_segment.to_ascii_lowercase();
    let mut output = String::with_capacity(value.len());
    let mut last_copy = 0;
    let mut search_from = 0;
    let lower = value.to_ascii_lowercase();
    while let Some(relative) = lower[search_from..].find(&needle) {
        let index = search_from + relative;
        let start = value[..index]
            .char_indices()
            .rev()
            .find(|(_, ch)| !is_windows_path_prefix_char(*ch))
            .map(|(offset, ch)| offset + ch.len_utf8())
            .unwrap_or(0);
        output.push_str(&value[last_copy..start]);
        output.push('~');
        let after_users = &value[index + users_segment.len()..];
        let rest_start = after_users.find(['\\', '/']).unwrap_or(after_users.len());
        last_copy = index + users_segment.len() + rest_start;
        search_from = last_copy;
    }
    output.push_str(&value[last_copy..]);
    output
}

fn is_windows_path_prefix_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, ':' | '?' | '\\' | '/')
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let kept: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}

fn truncate_lines(value: &str, max_lines: usize) -> String {
    let mut lines = value.lines();
    let mut kept = Vec::new();
    for _ in 0..max_lines {
        match lines.next() {
            Some(line) => kept.push(line),
            None => return kept.join("\n"),
        }
    }
    if lines.next().is_some() {
        kept.push("…");
    }
    kept.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        RUNNING_MARKER, decode_report_bytes, format_crash_message, format_panic_report,
        latest_wer_snippet_from, redact_user_paths, summarize_os_report, take_dirty_shutdown_at,
        take_last_panic_at,
    };
    use std::time::{Duration, SystemTime};

    #[test]
    fn dirty_shutdown_marker_is_consumed_once() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let marker = directory.path().join("session-running");
        std::fs::write(&marker, RUNNING_MARKER).expect("marker should be written");

        assert!(take_dirty_shutdown_at(&marker).is_some());
        assert!(take_dirty_shutdown_at(&marker).is_none());
    }

    #[test]
    fn ignores_missing_or_clean_markers() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let marker = directory.path().join("session-running");
        assert!(take_dirty_shutdown_at(&marker).is_none());
        std::fs::write(&marker, "clean").expect("marker should be written");
        assert!(take_dirty_shutdown_at(&marker).is_none());
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
        assert_eq!(
            redact_user_paths(r"from C:\Users\jose\AppData\Local\Captures"),
            r"from ~\AppData\Local\Captures"
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
Thread 4 Crashed:: tokio-rt-worker
0   libsystem_kernel.dylib  0x00000001800d5c34 __pthread_kill + 8
1   captures                0x0000000100123456 captures_desktop_lib::run + 32
Thread 0:: main";
        let summary = summarize_os_report(report).expect("summary");
        assert!(summary.contains("EXC_BREAKPOINT"));
        assert!(summary.contains("tokio-rt-worker"));
        assert!(summary.contains("Must only be used from the main thread"));
        assert!(summary.contains("captures_desktop_lib::run"));
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
            "termination": {
                "namespace": "SIGNAL",
                "indicator": "Trace/BPT trap: 5"
            },
            "usedImages": [
                { "name": "libsystem_kernel.dylib" },
                { "name": "captures" }
            ],
            "threads": [
                { "name": "main", "triggered": false },
                {
                    "name": "tokio-rt-worker",
                    "triggered": true,
                    "frames": [
                        { "imageIndex": 0, "symbol": "__pthread_kill", "symbolLocation": 8 },
                        { "imageIndex": 1, "symbol": "captures_desktop_lib::run", "symbolLocation": 32 }
                    ]
                }
            ]
        });
        let summary = summarize_os_report(&report.to_string()).expect("summary");
        assert!(summary.contains("EXC_BREAKPOINT"));
        assert!(summary.contains("SIGTRAP"));
        assert!(summary.contains("tokio-rt-worker"));
        assert!(summary.contains("Must only be used from the main thread"));
        assert!(summary.contains("captures_desktop_lib::run"));
        assert!(summary.contains("Termination Reason: Namespace SIGNAL"));
    }

    #[test]
    fn formats_panic_reports_with_the_error_and_backtrace() {
        let summary = format_panic_report(
            "tokio-rt-worker",
            "apps/desktop/src-tauri/src/lib.rs:99:5",
            "Must only be used from the main thread",
            "stack backtrace:\n   0: captures_desktop_lib::run\n   1: /Users/jose/.cargo/bin/captures",
        );
        assert!(summary.contains("Panic:"));
        assert!(summary.contains("thread 'tokio-rt-worker' panicked"));
        assert!(summary.contains("Must only be used from the main thread"));
        assert!(summary.contains("captures_desktop_lib::run"));
        assert!(summary.contains("~/.cargo/bin/captures"));
        assert!(!summary.contains("/Users/jose"));
    }

    #[test]
    fn consumes_last_panic_file_once() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let path = directory.path().join("last-panic");
        std::fs::write(
            &path,
            "Panic:\nthread 'main' panicked at src/lib.rs:1:1:\nbang",
        )
        .expect("panic file should be written");
        let first = take_last_panic_at(&path).expect("panic snippet");
        assert!(first.contains("bang"));
        assert!(take_last_panic_at(&path).is_none());
    }

    #[test]
    fn summarizes_windows_error_reporting() {
        let report = "\
Version=1
EventType=APPCRASH
Sig[0].Name=Application Name
Sig[0].Value=captures.exe
Sig[3].Name=Fault Module Name
Sig[3].Value=ntdll.dll
Sig[6].Name=Exception Code
Sig[6].Value=c0000005
Sig[7].Name=Exception Offset
Sig[7].Value=000000000006d3a5
";
        let summary = summarize_os_report(report).expect("summary");
        assert!(summary.contains("Windows Error Reporting:"));
        assert!(summary.contains("captures.exe"));
        assert!(summary.contains("0xc0000005 (STATUS_ACCESS_VIOLATION)"));
        assert!(summary.contains("ntdll.dll"));
    }

    #[test]
    fn decodes_utf16_windows_error_reports() {
        let text = "EventType=APPCRASH\r\nSig[0].Name=Application Name\r\nSig[0].Value=captures.exe\r\nSig[6].Name=Exception Code\r\nSig[6].Value=c0000409\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let decoded = decode_report_bytes(&bytes).expect("decoded");
        let summary = summarize_os_report(&decoded).expect("summary");
        assert!(summary.contains("0xc0000409 (STATUS_STACK_BUFFER_OVERRUN)"));
    }

    #[test]
    fn reads_recent_wer_reports_and_ignores_stale_ones() {
        let directory = tempfile::tempdir().expect("temporary directory should exist");
        let archive = directory.path().join("ReportArchive");
        std::fs::create_dir_all(&archive).expect("archive should exist");

        let stale_dir = archive.join("AppCrash_captures.exe_old");
        std::fs::create_dir_all(&stale_dir).expect("stale folder");
        let stale_report = stale_dir.join("Report.wer");
        std::fs::write(
            &stale_report,
            "EventType=APPCRASH\nSig[0].Name=Application Name\nSig[0].Value=captures.exe\nSig[6].Name=Exception Code\nSig[6].Value=c0000005\n",
        )
        .expect("stale report");
        let stale_time = SystemTime::now()
            .checked_sub(Duration::from_secs(60 * 60))
            .expect("stale time");
        std::fs::File::options()
            .write(true)
            .open(&stale_report)
            .expect("open stale")
            .set_modified(stale_time)
            .expect("set stale mtime");

        let fresh_dir = archive.join("AppCrash_captures.exe_new");
        std::fs::create_dir_all(&fresh_dir).expect("fresh folder");
        std::fs::write(
            fresh_dir.join("Report.wer"),
            "EventType=APPCRASH\nSig[0].Name=Application Name\nSig[0].Value=captures.exe\nSig[6].Name=Exception Code\nSig[6].Value=c0000409\n",
        )
        .expect("fresh report");

        let started_at = SystemTime::now()
            .checked_sub(Duration::from_secs(5))
            .expect("started at");
        let summary = latest_wer_snippet_from([archive], Some(started_at)).expect("summary");
        assert!(summary.contains("0xc0000409 (STATUS_STACK_BUFFER_OVERRUN)"));
        assert!(!summary.contains("0xc0000005"));
    }

    #[test]
    fn summarizes_linux_apport_reports() {
        let report = "\
ProblemType: Crash
Architecture: amd64
ExecutablePath: /home/ubuntu/.local/bin/captures
Signal: 11
Stacktrace:
#0  raise () from /lib/x86_64-linux-gnu/libc.so.6
#1  abort () from /lib/x86_64-linux-gnu/libc.so.6
#2  rust_panic at src/lib.rs:12
";
        let summary = summarize_os_report(report).expect("summary");
        assert!(summary.contains("Linux crash report:"));
        assert!(summary.contains("SIGSEGV"));
        assert!(summary.contains("rust_panic"));
        assert!(summary.contains("~/.local/bin/captures"));
    }

    #[test]
    fn crash_message_includes_the_exception_instead_of_boilerplate_alone() {
        let message = format_crash_message(vec![
            "Panic:\nthread 'main' panicked at src/lib.rs:9:1:\nboom".to_owned(),
        ]);
        assert!(message.starts_with(
            "Captures closed unexpectedly. This is an automatic crash diagnostic; it does not include captures."
        ));
        assert!(message.contains("thread 'main' panicked"));
        assert!(message.contains("boom"));
    }

    #[test]
    fn crash_message_notes_when_no_exception_was_found() {
        let message = format_crash_message(Vec::new());
        assert!(message.contains("No exception summary was available"));
    }
}
