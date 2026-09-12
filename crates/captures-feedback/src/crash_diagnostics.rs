//! Local-only crash evidence used to build a user-reviewable feedback draft.
//!
//! This module stores only session timestamps and a bounded, redacted Rust panic.
//! OS reports remain platform-owned: callers supply a candidate path or text and
//! an exact executable identity. No directories, captures, environment dumps,
//! permissions, signals, termination, UI, or network requests are handled here.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CURRENT: &str = "current-session";
const UNCLEAN: &str = "unclean-session";
const PANIC: &str = "last-panic";
const PREVIOUS_PANIC: &str = "previous-panic";
const MARKER: &str = "captures-session-v1";
const MAX_REPORT_BYTES: u64 = 256 * 1024;
const MAX_SUMMARY_CHARS: usize = 3_500;
const MAX_BACKTRACE_LINES: usize = 32;
const REPORT_SKEW: Duration = Duration::from_secs(5);
const STALE_REPORT_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Exact application identity supplied by the native frontend.
#[derive(Clone, Debug)]
pub struct ReportIdentity<'a> {
    pub executable_name: &'a str,
    pub bundle_id: Option<&'a str>,
    pub executable_path: Option<&'a Path>,
}

/// Evidence from the immediately preceding session. An unclean exit is not,
/// by itself, represented as a proven panic or OS exception.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticPreview {
    pub previous_session_started_at: Option<SystemTime>,
    pub unclean_exit: bool,
    pub rust_panic: Option<String>,
    pub os_report: Option<String>,
}

impl DiagnosticPreview {
    pub fn has_exception_evidence(&self) -> bool {
        self.rust_panic.is_some() || self.os_report.is_some()
    }
}

/// Profile-scoped session state. Use a distinct profile path per app/profile.
pub struct CrashSession {
    directory: PathBuf,
}

impl CrashSession {
    /// Preserves a prior running marker, then starts a new session. This does
    /// not inspect OS report directories or send feedback.
    pub fn start(profile_path: impl AsRef<Path>) -> io::Result<Self> {
        let directory = profile_path.as_ref().join("crash-diagnostics");
        fs::create_dir_all(&directory)?;
        let current = directory.join(CURRENT);
        if let Ok(value) = fs::read_to_string(&current)
            && parse_marker(&value).is_some()
        {
            fs::write(directory.join(UNCLEAN), value)?;
            remove_if_exists(&directory.join(PREVIOUS_PANIC))?;
            if directory.join(PANIC).is_file() {
                fs::rename(directory.join(PANIC), directory.join(PREVIOUS_PANIC))?;
            }
        }
        fs::write(&current, marker_now())?;
        Ok(Self { directory })
    }

    /// Reads retained evidence without consuming it, for preview/consent UI.
    pub fn preview(&self) -> DiagnosticPreview {
        let previous_session_started_at = fs::read_to_string(self.directory.join(UNCLEAN))
            .ok()
            .and_then(|value| parse_marker(&value));
        let rust_panic = read_bounded(&self.directory.join(PREVIOUS_PANIC))
            .ok()
            .flatten()
            .map(|value| sanitize(&value));
        DiagnosticPreview {
            unclean_exit: previous_session_started_at.is_some(),
            previous_session_started_at,
            rust_panic,
            os_report: None,
        }
    }

    /// Removes only retained prior-session evidence, not this session marker.
    pub fn dismiss_previous(&self) -> io::Result<()> {
        remove_if_exists(&self.directory.join(UNCLEAN))?;
        remove_if_exists(&self.directory.join(PREVIOUS_PANIC))
    }

    /// Call from every normal application/OS shutdown path.
    pub fn mark_clean_exit(&self) -> io::Result<()> {
        remove_if_exists(&self.directory.join(CURRENT))?;
        remove_if_exists(&self.directory.join(PANIC))
    }

    /// Precompute these paths for platform-owned async-signal-safe shutdown
    /// handlers. Removing them marks a clean exit without erasing prior evidence.
    pub fn clean_exit_paths(&self) -> [PathBuf; 2] {
        [self.directory.join(CURRENT), self.directory.join(PANIC)]
    }

    /// Installs a process-global hook which writes a bounded redacted panic and
    /// then invokes the prior hook. Call once during startup if desired.
    pub fn install_panic_hook(&self) {
        self.install_panic_hook_with_homes(Vec::new());
    }

    /// As above, additionally redacting caller-known custom home directories.
    pub fn install_panic_hook_with_homes(&self, homes: Vec<PathBuf>) {
        let path = self.directory.join(PANIC);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let thread = thread.name().unwrap_or("unknown");
            let location = info
                .location()
                .map_or_else(|| "unknown".into(), ToString::to_string);
            let payload = info.payload_as_str().unwrap_or("Box<dyn Any>");
            let trace = std::backtrace::Backtrace::force_capture().to_string();
            let body = format!(
                "Panic:\nthread '{thread}' panicked at {location}:\n{payload}\n\n{}",
                truncate_lines(trace.trim(), MAX_BACKTRACE_LINES)
            );
            let _ = fs::write(&path, sanitize_with_homes(&body, &homes));
            previous(info);
        }));
    }
}

/// Reads at most 256 KiB and accepts the report only if its own application
/// fields match `identity` and its timestamp matches the prior session.
pub fn summarize_report_path(
    path: &Path,
    modified: SystemTime,
    session_started: Option<SystemTime>,
    identity: &ReportIdentity<'_>,
) -> io::Result<Option<String>> {
    if !report_is_recent(modified, session_started) {
        return Ok(None);
    }
    Ok(read_bounded(path)?.and_then(|text| summarize_report_text(&text, identity)))
}

/// Summarizes macOS IPS/text crash, Windows WER, or Linux Apport text after
/// exact application identity validation. Caller-provided text is char-bounded.
pub fn summarize_report_text(text: &str, identity: &ReportIdentity<'_>) -> Option<String> {
    let text = truncate_chars(text, MAX_REPORT_BYTES as usize);
    let trimmed = text.trim_start_matches(['\u{feff}', ' ', '\n', '\r']);
    let summary = if trimmed.starts_with('{') {
        summarize_ips(trimmed, identity)?
    } else if field(trimmed, "EventType", '=').is_some() || trimmed.contains("Sig[0].Name=") {
        summarize_wer(trimmed, identity)?
    } else if field(trimmed, "ProblemType", ':').is_some() {
        summarize_apport(trimmed, identity)?
    } else {
        summarize_macos_text(trimmed, identity)?
    };
    Some(truncate_chars(&sanitize(&summary), MAX_SUMMARY_CHARS))
}

fn summarize_ips(text: &str, id: &ReportIdentity<'_>) -> Option<String> {
    // Modern .ips files have a metadata JSON line followed by a report object.
    let value = serde_json::Deserializer::from_str(text)
        .into_iter::<serde_json::Value>()
        .take(2)
        .filter_map(Result::ok)
        .find(|value| value.get("exception").is_some())?;
    let process = value.get("procName").and_then(|v| v.as_str());
    let bundle = value
        .pointer("/bundleInfo/CFBundleIdentifier")
        .and_then(|v| v.as_str());
    if process != Some(id.executable_name)
        || matches!((bundle, id.bundle_id), (Some(a), Some(b)) if a != b)
        || id.executable_path.is_some_and(|expected| {
            value
                .get("procPath")
                .and_then(|v| v.as_str())
                .map(Path::new)
                != Some(expected)
        })
    {
        return None;
    }
    let exception = value.get("exception")?;
    let kind = exception
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let signal = exception
        .get("signal")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut lines = vec![format!("Exception Type: {kind}\nSignal: {signal}")];
    if let Some(reason) = value
        .pointer("/termination/indicator")
        .and_then(|v| v.as_str())
    {
        lines.push(format!("Termination Reason: {reason}"));
    }
    let threads = value.get("threads").and_then(|v| v.as_array());
    let thread = threads.and_then(|threads| {
        threads
            .iter()
            .find(|thread| thread.get("triggered").and_then(|v| v.as_bool()) == Some(true))
            .or_else(|| {
                value
                    .get("faultingThread")
                    .and_then(|v| v.as_u64())
                    .and_then(|index| threads.get(index as usize))
            })
    });
    if let Some(frames) = thread
        .and_then(|thread| thread.get("frames"))
        .and_then(|v| v.as_array())
    {
        for frame in frames.iter().take(12) {
            if let Some(symbol) = frame.get("symbol").and_then(|v| v.as_str()) {
                lines.push(format!("  {symbol}"));
            }
        }
    }
    Some(lines.join("\n"))
}

fn summarize_macos_text(text: &str, id: &ReportIdentity<'_>) -> Option<String> {
    let process = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Process:"))?
        .trim();
    let process = process.split_once(" [").map_or(process, |(name, _)| name);
    if process != id.executable_name
        || matches!((field(text, "Identifier", ':'), id.bundle_id), (Some(a), Some(b)) if a != b)
        || id
            .executable_path
            .is_some_and(|path| field(text, "Path", ':').map(Path::new) != Some(path))
    {
        return None;
    }
    selected_lines(
        text,
        &[
            "Exception Type:",
            "Termination Reason:",
            "Triggered by Thread:",
            "Application Specific Information:",
        ],
    )
}

fn summarize_wer(text: &str, id: &ReportIdentity<'_>) -> Option<String> {
    let app = wer_value(text, "Application Name")?;
    if !app.eq_ignore_ascii_case(id.executable_name)
        || id.executable_path.is_some_and(|path| {
            !field(text, "AppPath", '=').is_some_and(|actual| {
                actual
                    .replace('\\', "/")
                    .eq_ignore_ascii_case(&path.to_string_lossy().replace('\\', "/"))
            })
        })
    {
        return None;
    }
    let event = field(text, "EventType", '=').unwrap_or("APPCRASH");
    let code = wer_value(text, "Exception Code").unwrap_or("unknown");
    let module = wer_value(text, "Fault Module Name").unwrap_or("unknown");
    Some(format!(
        "Windows Error Reporting:\nEvent Type: {event}\nApplication: {app}\nException: {code}\nFault module: {module}"
    ))
}

fn summarize_apport(text: &str, id: &ReportIdentity<'_>) -> Option<String> {
    let executable = field(text, "ExecutablePath", ':')?;
    let exact_name =
        Path::new(executable).file_name().and_then(|v| v.to_str()) == Some(id.executable_name);
    if !exact_name
        || id
            .executable_path
            .is_some_and(|path| Path::new(executable) != path)
    {
        return None;
    }
    let signal = field(text, "Signal", ':').unwrap_or("unknown");
    let assertion = field(text, "AssertionMessage", ':');
    Some(format!(
        "Linux crash report:\nSignal: {signal}\nExecutable: {executable}{}",
        assertion.map_or(String::new(), |v| format!("\nAssertion: {v}"))
    ))
}

fn selected_lines(text: &str, prefixes: &[&str]) -> Option<String> {
    let lines: Vec<_> = prefixes
        .iter()
        .filter_map(|prefix| {
            text.lines()
                .find(|line| line.trim_start().starts_with(prefix))
                .map(|v| v.trim())
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn wer_value<'a>(text: &'a str, wanted: &str) -> Option<&'a str> {
    let index = text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Sig[")?;
        let (index, tail) = rest.split_once(']')?;
        (tail.strip_prefix(".Name=")? == wanted).then_some(index)
    })?;
    let prefix = format!("Sig[{index}].Value=");
    text.lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
}

fn field<'a>(text: &'a str, key: &str, delimiter: char) -> Option<&'a str> {
    let prefix = format!("{key}{delimiter}");
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    })
}

fn report_is_recent(modified: SystemTime, started: Option<SystemTime>) -> bool {
    started.map_or_else(
        || {
            SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|age| age <= STALE_REPORT_AGE)
        },
        |start| modified >= start.checked_sub(REPORT_SKEW).unwrap_or(start),
    )
}

fn read_bounded(path: &Path) -> io::Result<Option<String>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_REPORT_BYTES).read_to_end(&mut bytes)?;
    let text = if bytes.starts_with(&[0xff, 0xfe]) {
        decode_utf16(&bytes[2..], true)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        decode_utf16(&bytes[2..], false)
    } else {
        String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
            .into_owned()
    };
    Ok(Some(text))
}

fn decode_utf16(bytes: &[u8], little: bool) -> String {
    String::from_utf16_lossy(
        &bytes
            .chunks_exact(2)
            .map(|v| {
                if little {
                    u16::from_le_bytes([v[0], v[1]])
                } else {
                    u16::from_be_bytes([v[0], v[1]])
                }
            })
            .collect::<Vec<_>>(),
    )
}

fn sanitize(value: &str) -> String {
    sanitize_with_homes(value, &[])
}

fn sanitize_with_homes(value: &str, homes: &[PathBuf]) -> String {
    let mut value = value.to_owned();
    let configured: Vec<PathBuf> = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    for home in homes.iter().chain(&configured) {
        let home = home.to_string_lossy();
        let home = home.trim_end_matches(['/', '\\']);
        if home.len() >= 3 {
            for variant in [
                home.to_owned(),
                home.replace('\\', "/"),
                home.replace('/', "\\"),
            ] {
                let lower = value.to_ascii_lowercase();
                let mut redacted = String::new();
                let mut copied = 0;
                for (index, _) in lower.match_indices(&variant.to_ascii_lowercase()) {
                    let end = index + variant.len();
                    if value[end..].chars().next().is_none_or(|ch| {
                        ch == '/' || ch == '\\' || ch.is_whitespace() || ch == '"' || ch == '\''
                    }) {
                        redacted.push_str(&value[copied..index]);
                        redacted.push('~');
                        copied = end;
                    }
                }
                redacted.push_str(&value[copied..]);
                value = redacted;
            }
        }
    }
    for prefix in ["/Users/", "/home/"] {
        value = redact_prefix(&value, prefix, '/');
    }
    value = redact_prefix(&value, "\\Users\\", '\\');
    value = redact_prefix(&value, "/Users/", '/');
    truncate_chars(&value, MAX_SUMMARY_CHARS)
}

fn redact_prefix(value: &str, prefix: &str, separator: char) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.to_ascii_lowercase().find(&prefix.to_ascii_lowercase()) {
        out.push_str(&rest[..start]);
        if out.len() >= 2
            && out.ends_with(':')
            && out.as_bytes()[out.len() - 2].is_ascii_alphabetic()
        {
            out.truncate(out.len() - 2);
        }
        out.push('~');
        let after = &rest[start + prefix.len()..];
        rest = after
            .find(|ch: char| ch == separator || ch.is_whitespace() || ch == '"' || ch == '\'')
            .map_or("", |i| &after[i..]);
    }
    out.push_str(rest);
    out
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let kept: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}
fn truncate_lines(value: &str, limit: usize) -> String {
    value.lines().take(limit).collect::<Vec<_>>().join("\n")
}
fn marker_now() -> String {
    format!(
        "{MARKER}\n{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}
fn parse_marker(value: &str) -> Option<SystemTime> {
    let mut lines = value.lines();
    (lines.next()? == MARKER).then_some(())?;
    let millis = lines.next()?.parse::<u64>().ok()?;
    Some(UNIX_EPOCH + Duration::from_millis(millis))
}
fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity<'a>() -> ReportIdentity<'a> {
        ReportIdentity {
            executable_name: "captures-native",
            bundle_id: Some("es.captures.native"),
            executable_path: None,
        }
    }

    #[test]
    fn prior_marker_survives_until_dismissal_and_clean_exit_is_not_dirty() {
        let root = tempfile::tempdir().unwrap();
        let first = CrashSession::start(root.path()).unwrap();
        let second = CrashSession::start(root.path()).unwrap();
        assert!(second.preview().unclean_exit);
        second.dismiss_previous().unwrap();
        assert!(!second.preview().unclean_exit);
        second.mark_clean_exit().unwrap();
        assert!(
            !CrashSession::start(root.path())
                .unwrap()
                .preview()
                .unclean_exit
        );
        drop(first);
    }

    #[test]
    fn sanitizes_unix_windows_and_custom_home_shapes() {
        let value = sanitize_with_homes(
            "/Users/alice/a /home/bob/b C:\\Users\\carol\\c D:/Users/dan/d /var/home/erin/e",
            &[PathBuf::from("/var/home/erin")],
        );
        assert_eq!(value, "~/a ~/b ~\\c ~/d ~/e");
        assert_eq!(
            sanitize("panic at /home/alice\nnext line"),
            "panic at ~\nnext line"
        );
        assert_eq!(
            sanitize_with_homes(
                "D:/CUSTOM/ERIN/file D:/Custom/ErinExtra/file",
                &[PathBuf::from("D:\\Custom\\Erin")]
            ),
            "~/file D:/Custom/ErinExtra/file"
        );
    }

    #[test]
    fn previous_panic_survives_clean_relaunch_until_dismissed() {
        let root = tempfile::tempdir().unwrap();
        let first = CrashSession::start(root.path()).unwrap();
        fs::write(
            first.directory.join(PANIC),
            "Panic at /home/alice/project/main.rs",
        )
        .unwrap();
        let second = CrashSession::start(root.path()).unwrap();
        let preview = second.preview();
        assert!(preview.has_exception_evidence());
        assert_eq!(
            preview.rust_panic.as_deref(),
            Some("Panic at ~/project/main.rs")
        );
        second.mark_clean_exit().unwrap();
        let third = CrashSession::start(root.path()).unwrap();
        assert_eq!(third.preview(), preview);
        third.dismiss_previous().unwrap();
        assert!(!third.preview().has_exception_evidence());
        assert!(third.directory.join(CURRENT).exists());
        third.mark_clean_exit().unwrap();
        assert!(
            !CrashSession::start(root.path())
                .unwrap()
                .preview()
                .unclean_exit
        );
    }

    #[test]
    fn panic_hook_writes_redacted_evidence_in_an_isolated_process() {
        const CHILD_PROFILE: &str = "CAPTURES_CRASH_TEST_PROFILE";
        if let Some(profile) = std::env::var_os(CHILD_PROFILE) {
            let session = CrashSession::start(profile).unwrap();
            session.install_panic_hook_with_homes(vec![PathBuf::from("/private/custom-person")]);
            panic!("failed /private/custom-person/code.rs and C:\\Users\\alice\\test.rs");
        }
        let root = tempfile::tempdir().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_diagnostics::tests::panic_hook_writes_redacted_evidence_in_an_isolated_process"])
            .env(CHILD_PROFILE, root.path()).output().unwrap();
        assert!(!child.status.success());
        let session = CrashSession::start(root.path()).unwrap();
        let panic = session.preview().rust_panic.unwrap();
        assert!(panic.contains("~/code.rs and ~\\test.rs"));
        assert!(!panic.contains("custom-person") && !panic.contains("alice"));
        assert!(panic.chars().count() <= MAX_SUMMARY_CHARS + 1);
    }

    #[test]
    fn ips_metadata_and_same_named_other_installations_are_checked() {
        let id = ReportIdentity {
            executable_path: Some(Path::new("/opt/captures-native")),
            ..identity()
        };
        let ips = concat!(
            "{\"app_name\":\"captures-native\",\"bug_type\":\"309\"}\n",
            "{\"procName\":\"captures-native\",\"procPath\":\"/opt/captures-native\",",
            "\"bundleInfo\":{\"CFBundleIdentifier\":\"es.captures.native\"},",
            "\"exception\":{\"type\":\"EXC_BAD_ACCESS\",\"signal\":\"SIGSEGV\"},",
            "\"threads\":[{\"triggered\":true,\"frames\":[{\"symbol\":\"draw_native\"}]}]}"
        );
        let summary = summarize_report_text(ips, &id).unwrap();
        assert!(summary.contains("EXC_BAD_ACCESS") && summary.contains("draw_native"));
        assert!(summarize_report_text(&ips.replace("/opt/", "/other/"), &id).is_none());
        assert!(
            summarize_report_text(&ips.replace("es.captures.native", "es.captures.tauri"), &id)
                .is_none()
        );
        let apport = "ProblemType: Crash\nExecutablePath: /opt/captures-native\nSignal: 11";
        assert!(summarize_report_text(apport, &id).is_some());
        assert!(summarize_report_text(&apport.replace("/opt/", "/other/"), &id).is_none());
        let wer_id = ReportIdentity {
            executable_path: Some(Path::new("C:\\Native\\captures-native")),
            ..identity()
        };
        let wer = "EventType=APPCRASH\nAppPath=c:\\native\\captures-native\nSig[0].Name=Application Name\nSig[0].Value=captures-native\nSig[1].Name=Exception Code\nSig[1].Value=c0000005";
        assert!(summarize_report_text(wer, &wer_id).is_some());
        assert!(summarize_report_text(&wer.replace("native\\", "other\\"), &wer_id).is_none());
    }

    #[test]
    fn bounded_unicode_report_and_identity_rejection() {
        let good =
            "Process: captures-native [1]\nException Type: EXC_BAD_ACCESS 💥\n".repeat(100_000);
        let summary = summarize_report_text(&good, &identity()).unwrap();
        assert!(summary.chars().count() <= MAX_SUMMARY_CHARS + 1);
        assert!(
            summarize_report_text("Process: other [1]\nException Type: X", &identity()).is_none()
        );
        assert!(
            summarize_report_text(
                "ProblemType: Crash\nExecutablePath: /usr/bin/other\nSignal: 11",
                &identity()
            )
            .is_none()
        );
    }

    #[test]
    fn report_path_checks_timestamp_and_does_not_mutate_files() {
        let root = tempfile::tempdir().unwrap();
        let report = root.path().join("one.crash");
        let unrelated = root.path().join("unrelated");
        fs::write(
            &report,
            "ProblemType: Crash\nExecutablePath: /opt/captures-native\nSignal: 11",
        )
        .unwrap();
        fs::write(&unrelated, "keep").unwrap();
        let now = SystemTime::now();
        assert!(
            summarize_report_path(&report, now, Some(now), &identity())
                .unwrap()
                .is_some()
        );
        assert!(
            summarize_report_path(&report, UNIX_EPOCH, Some(now), &identity())
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "keep");
        assert!(report.exists());
    }

    #[test]
    fn utf16_is_decoded_with_a_bounded_read() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Report.wer");
        let text = "EventType=APPCRASH\nSig[0].Name=Application Name\nSig[0].Value=captures-native\nSig[1].Name=Exception Code\nSig[1].Value=c0000005";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(&path, bytes).unwrap();
        let summary = summarize_report_path(&path, SystemTime::now(), None, &identity())
            .unwrap()
            .unwrap();
        assert!(summary.contains("c0000005"));
    }
}
