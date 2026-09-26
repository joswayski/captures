//! Update notice status model, release-note parsing and presentation copy.
//!
//! Ported from the shipping Tauri surface: the status model in
//! `apps/desktop/src-tauri/src/updates.rs`, `UpdateNotice` in
//! `apps/desktop/ui/src/App.tsx` and `apps/desktop/ui/src/lib/releaseNotes.ts`.
//! Both native hosts render [`present`] so their copy stays identical.
//!
//! The native rewrite has no signed updater yet. [`fixture`] and [`stub_next`]
//! are a deterministic, injectable status source for workbench fixtures: they
//! never download, verify or install anything.
use serde::{Deserialize, Serialize};

pub const CARD_WIDTH: f64 = 400.0;
const COMPACT_HEIGHT: f64 = 168.0;
const NOTES_HEIGHT: f64 = 122.0;
const MAX_HEIGHT: f64 = 480.0;
const STACK_HEIGHT: f64 = 72.0;
const WARNING_HEIGHT: f64 = 56.0;
const STATUS_HEIGHT: f64 = 56.0;
const ERROR_HEIGHT: f64 = 96.0;

pub const OPEN_CAPTURES_WARNING: &str =
    "Open captures will close. Unsaved edits are kept as drafts.";
pub const DOWNLOAD_PAGE_URL: &str = "https://captur.es/#download";
const CAPTURES_GITHUB_OWNER: &str = "joswayski";
const CAPTURES_GITHUB_REPO: &str = "captures";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    Idle {
        current_version: String,
        current_display_version: String,
    },
    Checking {
        current_version: String,
        current_display_version: String,
    },
    UpToDate {
        current_version: String,
        current_display_version: String,
    },
    Available {
        current_version: String,
        current_display_version: String,
        version: String,
        display_version: String,
        notes: Option<String>,
        changelog: Vec<ChangelogEntry>,
        installable: bool,
        manual_download_url: Option<String>,
        download_size: Option<u64>,
        will_close_open_captures: bool,
    },
    Downloading {
        current_version: String,
        current_display_version: String,
        version: String,
        display_version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Restarting {
        current_version: String,
        current_display_version: String,
        version: String,
        display_version: String,
        seconds_remaining: u8,
    },
    Error {
        current_version: String,
        current_display_version: String,
        message: String,
        retry_install: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub version: String,
    pub display_version: String,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteItem {
    pub text: String,
    pub pull_request: Option<PullRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteGroup {
    pub version: String,
    pub display_version: String,
    pub items: Vec<NoteItem>,
}

// ---------------------------------------------------------------- notes ----

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `\[([^\]]+)\]\([^\s)]+(?:\s+"[^"]*")?\)` -> `$1`.
fn replace_links(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    'outer: while i < chars.len() {
        if chars[i] == '['
            && let Some(close) = chars[i + 1..].iter().position(|&c| c == ']')
            && close > 0
            && chars.get(i + 1 + close + 1) == Some(&'(')
        {
            let text_end = i + 1 + close;
            let mut j = text_end + 2;
            let url_start = j;
            while j < chars.len() && !chars[j].is_whitespace() && chars[j] != ')' {
                j += 1;
            }
            if j > url_start {
                let mut k = j;
                // Optional title: \s+"[^"]*"
                let mut t = k;
                while t < chars.len() && chars[t].is_whitespace() {
                    t += 1;
                }
                if t > k
                    && chars.get(t) == Some(&'"')
                    && let Some(end) = chars[t + 1..].iter().position(|&c| c == '"')
                    && chars.get(t + end + 2) == Some(&')')
                {
                    k = t + end + 2;
                }
                if chars.get(k) == Some(&')') {
                    out.extend(&chars[i + 1..text_end]);
                    i = k + 1;
                    continue 'outer;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `<([^>]+)>` -> `$1`.
fn strip_angle_brackets(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<'
            && let Some(close) = chars[i + 1..].iter().position(|&c| c == '>')
            && close > 0
        {
            out.extend(&chars[i + 1..i + 1 + close]);
            i += close + 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Whitespace-separated tokens with byte offsets.
fn tokens(input: &str) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut start = None;
    for (index, c) in input.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                result.push((s, &input[s..index]));
                start = None;
            }
            (false, None) => start = Some(index),
            _ => {}
        }
    }
    if let Some(s) = start {
        result.push((s, &input[s..]));
    }
    result
}

/// `\s+by\s+@[\w-]+(?:\[bot\])?\s+in\s+https?://\S+\s*$` (case-insensitive).
fn strip_author_suffix(input: &str) -> &str {
    let parts = tokens(input);
    let [.., (by_start, by), (_, author), (_, word_in), (_, url)] = parts.as_slice() else {
        return input;
    };
    let lower_url = url.to_ascii_lowercase();
    let rest = lower_url
        .strip_prefix("https://")
        .or_else(|| lower_url.strip_prefix("http://"));
    let name = author
        .strip_prefix('@')
        .map(|name| {
            if name.len() > 5 && name[name.len() - 5..].eq_ignore_ascii_case("[bot]") {
                &name[..name.len() - 5]
            } else {
                name
            }
        })
        .unwrap_or("");
    if *by_start > 0
        && by.eq_ignore_ascii_case("by")
        && word_in.eq_ignore_ascii_case("in")
        && rest.is_some_and(|rest| !rest.is_empty())
        && !name.is_empty()
        && name.chars().all(|c| is_word(c) || c == '-')
    {
        &input[..*by_start]
    } else {
        input
    }
}

/// `\s+\(#(\d+)\)\s*$`: returns the text before it and the number.
fn trailing_pr_number(input: &str) -> Option<(&str, u64)> {
    let trimmed = input.trim_end();
    let inner = trimmed.strip_suffix(')')?;
    let open = inner.rfind("(#")?;
    let digits = &inner[open + 2..];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let before = &inner[..open];
    if !before.ends_with(char::is_whitespace) {
        return None;
    }
    Some((before, digits.parse().ok()?))
}

fn plain_text(markdown: &str) -> String {
    let text = strip_angle_brackets(&replace_links(markdown));
    let text: String = text
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '~' | '`'))
        .collect();
    let text = strip_author_suffix(&text);
    let text = trailing_pr_number(text).map_or(text, |(before, _)| before);
    text.trim().to_owned()
}

fn pull_request(owner: &str, repo: &str, number: u64) -> PullRequest {
    PullRequest {
        number,
        url: format!("https://github.com/{owner}/{repo}/pull/{number}"),
    }
}

/// `https://(?:www\.)?github\.com/([\w.-]+)/([\w.-]+)/pull/(\d+)` (case-insensitive).
fn github_pull_url(line: &str) -> Option<PullRequest> {
    let lower = line.to_ascii_lowercase();
    let segment = |s: &str| -> usize {
        s.char_indices()
            .find(|(_, c)| !(is_word(*c) || *c == '.' || *c == '-'))
            .map_or(s.len(), |(i, _)| i)
    };
    let mut from = 0;
    while let Some(found) = lower[from..].find("https://") {
        let start = from + found;
        from = start + 1;
        let mut at = start + "https://".len();
        if lower[at..].starts_with("www.") {
            at += 4;
        }
        let Some(after_host) = lower[at..].strip_prefix("github.com/") else {
            continue;
        };
        at = line.len() - after_host.len();
        let owner_len = segment(&line[at..]);
        if owner_len == 0 || !line[at + owner_len..].starts_with('/') {
            continue;
        }
        let owner = &line[at..at + owner_len];
        at += owner_len + 1;
        let repo_len = segment(&line[at..]);
        if repo_len == 0 || !lower[at + repo_len..].starts_with("/pull/") {
            continue;
        }
        let repo = &line[at..at + repo_len];
        at += repo_len + "/pull/".len();
        let digits_len = line[at..]
            .char_indices()
            .find(|(_, c)| !c.is_ascii_digit())
            .map_or(line.len() - at, |(i, _)| i);
        if digits_len == 0 {
            continue;
        }
        if let Ok(number) = line[at..at + digits_len].parse() {
            return Some(pull_request(owner, repo, number));
        }
    }
    None
}

fn pull_request_from_line(line: &str) -> Option<PullRequest> {
    github_pull_url(line).or_else(|| {
        trailing_pr_number(line)
            .map(|(_, number)| pull_request(CAPTURES_GITHUB_OWNER, CAPTURES_GITHUB_REPO, number))
    })
}

fn contains_word_phrase(text: &str, phrase: &str) -> bool {
    let lower = text.to_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(phrase) {
        let start = from + found;
        let end = start + phrase.len();
        let before = lower[..start].chars().next_back();
        let after = lower[end..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            return true;
        }
        from = start + 1;
    }
    false
}

/// GitHub auto-appends these under New Contributors; they are not product changes.
fn is_first_contribution(text: &str) -> bool {
    contains_word_phrase(text, "made their first contribution")
}

/// Dependency maintenance is useful in GitHub history, not product-facing copy.
fn is_dependency_update(text: &str) -> bool {
    text.get(..4)
        .is_some_and(|word| word.eq_ignore_ascii_case("bump"))
        && !text[4..].chars().next().is_some_and(is_word)
}

fn is_alert_start(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('>') else {
        return false;
    };
    let Some(rest) = rest.trim_start().strip_prefix("[!") else {
        return false;
    };
    let kind_len = rest
        .char_indices()
        .find(|(_, c)| !c.is_ascii_uppercase())
        .map_or(rest.len(), |(i, _)| i);
    kind_len > 0 && rest[kind_len..].starts_with(']')
}

fn is_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(char::is_whitespace)
}

fn is_full_changelog(line: &str) -> bool {
    let rest = line.trim_start_matches('*');
    if line.len() - rest.len() > 2
        || !rest
            .get(..14)
            .is_some_and(|head| head.eq_ignore_ascii_case("full changelog"))
    {
        return false;
    }
    let after = &rest[14..];
    let stars = after.len() - after.trim_start_matches('*').len();
    stars <= 2 && after[stars..].trim_start().starts_with(':')
}

fn list_body(line: &str) -> &str {
    let bullet = line
        .strip_prefix(['-', '*', '+'])
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .or_else(|| {
            let digits = line.chars().take_while(char::is_ascii_digit).count();
            (digits > 0)
                .then(|| line[digits..].strip_prefix(['.', ')']))
                .flatten()
                .filter(|rest| rest.starts_with(char::is_whitespace))
        });
    let body = bullet.map_or(line, str::trim_start);
    body.strip_prefix('>').map_or(body, |rest| {
        rest.strip_prefix(char::is_whitespace).unwrap_or(rest)
    })
}

/// Turns GitHub's generated release Markdown into concise notice copy.
pub fn release_note_items(markdown: &str) -> Vec<NoteItem> {
    let mut items = Vec::new();
    let mut skipping_alert = false;
    for source_line in markdown.lines() {
        let line = source_line.trim();
        if is_alert_start(line) {
            skipping_alert = true;
            continue;
        }
        if skipping_alert && line.starts_with('>') {
            continue;
        }
        if line.is_empty() {
            skipping_alert = false;
            continue;
        }
        skipping_alert = false;
        if is_heading(line) || is_full_changelog(line) {
            continue;
        }
        let body = list_body(line);
        let text = plain_text(body);
        if text.is_empty()
            || is_first_contribution(&text)
            || is_first_contribution(body)
            || is_dependency_update(&text)
        {
            continue;
        }
        items.push(NoteItem {
            text,
            pull_request: pull_request_from_line(body),
        });
    }
    items
}

/// One group per skipped Preview with product-facing notes, newest first.
pub fn stacked_release_notes(
    changelog: &[ChangelogEntry],
    fallback_notes: Option<&str>,
    fallback_display_version: &str,
) -> Vec<NoteGroup> {
    if !changelog.is_empty() {
        return changelog
            .iter()
            .filter_map(|entry| {
                let items = entry
                    .notes
                    .as_deref()
                    .map(release_note_items)
                    .unwrap_or_default();
                (!items.is_empty()).then(|| NoteGroup {
                    version: entry.version.clone(),
                    display_version: entry.display_version.clone(),
                    items,
                })
            })
            .collect();
    }
    let items = fallback_notes.map(release_note_items).unwrap_or_default();
    if items.is_empty() {
        return Vec::new();
    }
    vec![NoteGroup {
        version: String::new(),
        display_version: fallback_display_version.to_owned(),
        items,
    }]
}

// --------------------------------------------------------------- format ----

fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1_000.0 && unit < UNITS.len() - 1 {
        value /= 1_000.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} {}", value.round(), UNITS[unit]);
    }
    if value >= 100.0 {
        format!("{} {}", value.round(), UNITS[unit])
    } else {
        // JavaScript `toFixed(1)` rounds ties up; match it for identical copy.
        format!("{:.1} {}", (value * 10.0).round() / 10.0, UNITS[unit])
    }
}

pub fn format_update_size(bytes: u64) -> String {
    match bytes {
        0 => "0 KB".into(),
        1..1_000 => format!("{:.1} KB", (bytes as f64 / 100.0).round() / 10.0),
        _ => format_file_size(bytes),
    }
}

// --------------------------------------------------------- presentation ----

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Icon {
    App,
    Spinner,
    Check,
    Warning,
}

/// Fill behind the header icon: accent, positive (green), signal, or sunken.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconTone {
    Accent,
    Positive,
    Signal,
    Neutral,
}

/// A request from the notice to its status source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Later/Close/Escape.
    Dismiss,
    /// Update now, View release, or Try again after an install failure.
    Install,
    /// Check again, or Try again after a check failure.
    Check,
    /// Persist `show_update_changelog`.
    ShowNotes,
    HideNotes,
    OpenPullRequest {
        url: String,
    },
    OpenDownloadPage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Button {
    pub label: String,
    pub enabled: bool,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notes {
    pub heading: String,
    pub hide: Button,
    pub stacked: bool,
    pub intro: Option<String>,
    pub groups: Vec<NoteGroup>,
    /// Shown instead of groups when none have product-facing notes.
    pub empty: Option<String>,
    /// Shown inside a stacked group without items (never produced by
    /// [`stacked_release_notes`], kept for parity with the shipping markup).
    pub group_empty: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Download {
    pub label: String,
    pub detail: String,
    /// `None` renders an indeterminate bar.
    pub percent: Option<u8>,
    pub percent_label: Option<String>,
    pub accessible_value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Restart {
    pub message: String,
    pub seconds_remaining: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBlock {
    pub message: String,
    /// "You can also {link}." — hosts render `link` as a text button.
    pub fallback_prefix: String,
    pub fallback_link: String,
    pub fallback_suffix: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Footer {
    pub dismiss: Button,
    pub primary: Option<Button>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    /// `available`, `downloading`, `restarting`, `error`, `checking`,
    /// `up_to_date`, `idle` or `loading`.
    pub visual_state: String,
    pub icon: Icon,
    pub icon_tone: IconTone,
    pub title: String,
    pub description: String,
    /// Header "What's new" button while notes are hidden.
    pub reveal_notes: Option<Button>,
    pub notes: Option<Notes>,
    pub download: Option<Download>,
    pub restart: Option<Restart>,
    pub error: Option<ErrorBlock>,
    pub status_message: Option<String>,
    pub close_warning: Option<String>,
    pub footer: Option<Footer>,
    /// Escape and the window close button do nothing while busy.
    pub dismiss_blocked: bool,
    pub card_width: f64,
    pub card_height: f64,
}

/// Host-local view state that is not part of the status source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    #[serde(default = "default_true")]
    pub show_changelog: bool,
    #[serde(default)]
    pub action_error: Option<String>,
    #[serde(default)]
    pub installing: bool,
}

fn default_true() -> bool {
    true
}

/// Card height for the transparent notice window, as shipping sizes it.
pub fn card_height(status: Option<&UpdateStatus>, show_changelog: bool) -> f64 {
    let notes = match status {
        Some(UpdateStatus::Available { changelog, .. }) if show_changelog => {
            NOTES_HEIGHT + changelog.len().saturating_sub(1) as f64 * STACK_HEIGHT
        }
        _ => 0.0,
    };
    let warning = match status {
        Some(UpdateStatus::Available {
            will_close_open_captures: true,
            ..
        }) => WARNING_HEIGHT,
        _ => 0.0,
    };
    let body = match status {
        Some(UpdateStatus::Available { .. }) => 0.0,
        Some(UpdateStatus::Error { .. }) => ERROR_HEIGHT,
        _ => STATUS_HEIGHT,
    };
    (COMPACT_HEIGHT + notes + warning + body).min(MAX_HEIGHT)
}

fn button(label: &str, enabled: bool, action: Action) -> Button {
    Button {
        label: label.into(),
        enabled,
        action,
    }
}

pub fn present(status: Option<&UpdateStatus>, view: &ViewState) -> Presentation {
    use UpdateStatus as S;
    let action_error = view.action_error.as_deref().filter(|e| !e.is_empty());
    let error = action_error.or(match status {
        Some(S::Error { message, .. }) => Some(message.as_str()),
        _ => None,
    });
    let available = matches!(status, Some(S::Available { .. }));
    let state = match status {
        None => "loading",
        Some(S::Idle { .. }) => "idle",
        Some(S::Checking { .. }) => "checking",
        Some(S::UpToDate { .. }) => "up_to_date",
        Some(S::Available { .. }) => "available",
        Some(S::Downloading { .. }) => "downloading",
        Some(S::Restarting { .. }) => "restarting",
        Some(S::Error { .. }) => "error",
    };
    let visual_state = if error.is_some() { "error" } else { state };
    let (icon, icon_tone) = match visual_state {
        "restarting" => (Icon::Check, IconTone::Positive),
        "error" => (Icon::Warning, IconTone::Signal),
        "downloading" | "checking" | "loading" => (Icon::Spinner, IconTone::Neutral),
        "available" | "up_to_date" => (Icon::App, IconTone::Accent),
        _ => (Icon::App, IconTone::Neutral),
    };

    let (title, description) = match status {
        Some(S::Restarting {
            display_version, ..
        }) => ("Updated".into(), format!("Version {display_version}")),
        Some(S::Downloading {
            display_version, ..
        }) => (
            "Updating Captures".into(),
            format!("Version {display_version}"),
        ),
        _ if error.is_some() => (
            "Update failed".into(),
            "Your current version was not changed.".into(),
        ),
        Some(S::Available {
            display_version,
            installable,
            download_size,
            ..
        }) => {
            let size = download_size
                .filter(|size| *installable && *size > 0)
                .map(|size| format!(" · {}", format_update_size(size)))
                .unwrap_or_default();
            (
                "Update available".into(),
                format!("Version {display_version}{size}"),
            )
        }
        Some(S::UpToDate {
            current_display_version,
            ..
        }) => (
            "You’re up to date".into(),
            format!("Version {current_display_version}"),
        ),
        Some(S::Checking { .. }) => (
            "Checking for updates".into(),
            "This should only take a moment.".into(),
        ),
        _ => (
            "Loading update details".into(),
            "This should only take a moment.".into(),
        ),
    };

    let notes_visible = available && error.is_none() && view.show_changelog;
    let reveal_notes = (available && error.is_none() && !notes_visible)
        .then(|| button("What’s new", true, Action::ShowNotes));
    let notes = match status {
        Some(S::Available {
            changelog,
            notes,
            display_version,
            ..
        }) if notes_visible => {
            let groups = stacked_release_notes(changelog, notes.as_deref(), display_version);
            let stacked = groups.len() > 1;
            Some(Notes {
                heading: "What’s new".into(),
                hide: button("Hide", true, Action::HideNotes),
                stacked,
                intro: stacked
                    .then(|| "This update includes all of the following changes:".to_owned()),
                empty: groups
                    .is_empty()
                    .then(|| "Release notes aren’t available for this update.".to_owned()),
                group_empty: "Release notes aren’t available for this Preview.".into(),
                groups,
            })
        }
        _ => None,
    };

    let download = match status {
        Some(S::Downloading {
            downloaded, total, ..
        }) => {
            let total = total.filter(|total| *total > 0);
            let percent = total.map(|total| {
                ((*downloaded as f64 / total as f64) * 100.0)
                    .round()
                    .min(100.0) as u8
            });
            let detail = match total {
                Some(total) => format!(
                    "{} / {}",
                    format_update_size(*downloaded),
                    format_update_size(total)
                ),
                None => format!("{} downloaded", format_update_size(*downloaded)),
            };
            Some(Download {
                label: "Downloading".into(),
                accessible_value: match percent {
                    Some(percent) => format!("{detail}, {percent}% downloaded"),
                    None => detail.clone(),
                },
                percent_label: percent.map(|percent| format!("{percent}%")),
                detail,
                percent,
            })
        }
        _ => None,
    };

    let restart = match status {
        Some(S::Restarting {
            seconds_remaining, ..
        }) => Some(Restart {
            message: format!("Reopening in {seconds_remaining} seconds…"),
            seconds_remaining: *seconds_remaining,
        }),
        _ => None,
    };
    let downloading = download.is_some();
    let restarting = restart.is_some();

    let error_block = error.map(|message| ErrorBlock {
        message: message.to_owned(),
        fallback_prefix: "You can also ".into(),
        fallback_link: "download from captur.es".into(),
        fallback_suffix: ".".into(),
    });

    let status_message =
        (!available && !downloading && !restarting && error.is_none()).then(|| {
            if state == "up_to_date" {
                "No updates are available.".to_owned()
            } else {
                "Checking…".to_owned()
            }
        });

    let close_warning = match status {
        Some(S::Available {
            will_close_open_captures: true,
            ..
        }) if error.is_none() => Some(OPEN_CAPTURES_WARNING.to_owned()),
        _ => None,
    };

    let footer = (!downloading && !restarting).then(|| {
        let enabled = !view.installing;
        let primary = if available && error.is_none() {
            let installable = matches!(
                status,
                Some(S::Available {
                    installable: true,
                    ..
                })
            );
            Some(button(
                if installable {
                    "Update now"
                } else {
                    "View release"
                },
                enabled,
                Action::Install,
            ))
        } else if error.is_some() {
            let retry_install = available
                || matches!(
                    status,
                    Some(S::Error {
                        retry_install: true,
                        ..
                    })
                );
            Some(button(
                "Try again",
                enabled,
                if retry_install {
                    Action::Install
                } else {
                    Action::Check
                },
            ))
        } else if state != "checking" && state != "up_to_date" {
            Some(button("Check again", true, Action::Check))
        } else {
            None
        };
        Footer {
            dismiss: button(
                if available { "Later" } else { "Close" },
                enabled,
                Action::Dismiss,
            ),
            primary,
        }
    });

    Presentation {
        visual_state: visual_state.into(),
        icon,
        icon_tone,
        title,
        description,
        reveal_notes,
        notes,
        download,
        restart,
        error: error_block,
        status_message,
        close_warning,
        footer,
        dismiss_blocked: downloading || restarting || view.installing,
        card_width: CARD_WIDTH,
        card_height: card_height(status, view.show_changelog),
    }
}

// ---------------------------------------------------------- stub source ----

const FIXTURE_CURRENT: (&str, &str) = ("2026.8.2702", "2026.08.27.2");
const FIXTURE_LATEST: (&str, &str) = ("2026.8.2705", "2026.08.27.5");
const FIXTURE_SIZE: u64 = 12_582_912;
const FIXTURE_STEPS: u64 = 6;
const RESTART_COUNTDOWN_SECONDS: u8 = 3;

/// Workbench fixture names accepted by [`fixture`].
pub const FIXTURES: [&str; 9] = [
    "available",
    "single",
    "closing",
    "manual",
    "downloading",
    "restarting",
    "error",
    "checking",
    "up-to-date",
];

fn fixture_notes(summary: &str, pull: u32) -> String {
    [
        "> [!WARNING]".to_owned(),
        "> This Preview is functional, but experimental.".into(),
        String::new(),
        "## What's Changed".into(),
        format!("* {summary} by @joswayski in https://github.com/joswayski/captures/pull/{pull}"),
        "* Bump js-yaml from 4.3.1 to 4.3.2 ([#513](https://github.com/joswayski/captures/pull/513))".into(),
        format!("* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/{pull}"),
        String::new(),
        "## New Contributors".into(),
        format!("* @someone made their first contribution in https://github.com/joswayski/captures/pull/{pull}"),
        String::new(),
        "**Full Changelog**: https://github.com/joswayski/captures/compare/old...new".into(),
    ]
    .join("\n")
}

fn available_fixture(stacked: bool, closing: bool, installable: bool) -> UpdateStatus {
    let latest = fixture_notes("Fix post-update launch notice position on macOS", 265);
    let mut changelog = vec![ChangelogEntry {
        version: FIXTURE_LATEST.0.into(),
        display_version: FIXTURE_LATEST.1.into(),
        notes: Some(latest.clone()),
    }];
    if stacked {
        changelog.push(ChangelogEntry {
            version: "2026.8.2704".into(),
            display_version: "2026.08.27.4".into(),
            notes: Some(
                "* Bump @vitest/mocker and vitest ([#511](https://github.com/joswayski/captures/pull/511))"
                    .into(),
            ),
        });
        changelog.push(ChangelogEntry {
            version: "2026.8.2703".into(),
            display_version: "2026.08.27.3".into(),
            notes: Some(fixture_notes(
                "Redesign the desktop UI around one design system",
                262,
            )),
        });
    }
    UpdateStatus::Available {
        current_version: FIXTURE_CURRENT.0.into(),
        current_display_version: FIXTURE_CURRENT.1.into(),
        version: FIXTURE_LATEST.0.into(),
        display_version: FIXTURE_LATEST.1.into(),
        notes: Some(latest),
        changelog,
        installable,
        manual_download_url: (!installable).then(|| DOWNLOAD_PAGE_URL.to_owned()),
        download_size: Some(FIXTURE_SIZE),
        will_close_open_captures: closing,
    }
}

/// Synthetic statuses matching the Tauri dev harness (`?view=update&update=…`).
pub fn fixture(name: &str) -> Option<UpdateStatus> {
    let current_version = FIXTURE_CURRENT.0.to_owned();
    let current_display_version = FIXTURE_CURRENT.1.to_owned();
    Some(match name {
        "available" => available_fixture(true, false, true),
        "single" => available_fixture(false, false, true),
        "closing" => available_fixture(true, true, true),
        "manual" => available_fixture(false, false, false),
        "downloading" => UpdateStatus::Downloading {
            current_version,
            current_display_version,
            version: FIXTURE_LATEST.0.into(),
            display_version: FIXTURE_LATEST.1.into(),
            downloaded: 7_340_032,
            total: Some(FIXTURE_SIZE),
        },
        "restarting" => UpdateStatus::Restarting {
            current_version,
            current_display_version,
            version: FIXTURE_LATEST.0.into(),
            display_version: FIXTURE_LATEST.1.into(),
            seconds_remaining: RESTART_COUNTDOWN_SECONDS,
        },
        "error" => UpdateStatus::Error {
            current_version,
            current_display_version,
            message:
                "Could not install the update: Download request failed with status: 403 Forbidden"
                    .into(),
            retry_install: true,
        },
        "checking" => UpdateStatus::Checking {
            current_version,
            current_display_version,
        },
        "up-to-date" => UpdateStatus::UpToDate {
            current_version,
            current_display_version,
        },
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StubEvent {
    Install,
    Check,
    Tick,
}

fn current_versions(status: &UpdateStatus) -> (String, String) {
    match status {
        UpdateStatus::Idle {
            current_version,
            current_display_version,
        }
        | UpdateStatus::Checking {
            current_version,
            current_display_version,
        }
        | UpdateStatus::UpToDate {
            current_version,
            current_display_version,
        }
        | UpdateStatus::Available {
            current_version,
            current_display_version,
            ..
        }
        | UpdateStatus::Downloading {
            current_version,
            current_display_version,
            ..
        }
        | UpdateStatus::Restarting {
            current_version,
            current_display_version,
            ..
        }
        | UpdateStatus::Error {
            current_version,
            current_display_version,
            ..
        } => (current_version.clone(), current_display_version.clone()),
    }
}

/// Deterministic fixture transitions. `None` means the notice closes (the
/// stub "restart" finished); the stub never downloads or relaunches anything.
///
/// Install from Available/Error starts a simulated download, ticks advance it
/// to a three-second restart countdown, and Check resolves to up to date.
pub fn stub_next(status: &UpdateStatus, event: StubEvent) -> Option<UpdateStatus> {
    let (current_version, current_display_version) = current_versions(status);
    let downloading =
        |version: &str, display_version: &str, total: u64| UpdateStatus::Downloading {
            current_version: current_version.clone(),
            current_display_version: current_display_version.clone(),
            version: version.into(),
            display_version: display_version.into(),
            downloaded: 0,
            total: Some(total),
        };
    match (status, event) {
        (
            UpdateStatus::Available {
                version,
                display_version,
                download_size,
                ..
            },
            StubEvent::Install,
        ) => Some(downloading(
            version,
            display_version,
            download_size.unwrap_or(FIXTURE_SIZE),
        )),
        (UpdateStatus::Error { .. }, StubEvent::Install) => Some(downloading(
            FIXTURE_LATEST.0,
            FIXTURE_LATEST.1,
            FIXTURE_SIZE,
        )),
        (
            UpdateStatus::Idle { .. } | UpdateStatus::UpToDate { .. } | UpdateStatus::Error { .. },
            StubEvent::Check,
        ) => Some(UpdateStatus::Checking {
            current_version,
            current_display_version,
        }),
        (UpdateStatus::Checking { .. }, StubEvent::Tick) => Some(UpdateStatus::UpToDate {
            current_version,
            current_display_version,
        }),
        (
            UpdateStatus::Downloading {
                version,
                display_version,
                downloaded,
                total,
                ..
            },
            StubEvent::Tick,
        ) => {
            let total_bytes = total.unwrap_or(FIXTURE_SIZE);
            let next = (downloaded + total_bytes.div_ceil(FIXTURE_STEPS)).min(total_bytes);
            Some(if *downloaded >= total_bytes {
                UpdateStatus::Restarting {
                    current_version,
                    current_display_version,
                    version: version.clone(),
                    display_version: display_version.clone(),
                    seconds_remaining: RESTART_COUNTDOWN_SECONDS,
                }
            } else {
                UpdateStatus::Downloading {
                    current_version,
                    current_display_version,
                    version: version.clone(),
                    display_version: display_version.clone(),
                    downloaded: next,
                    total: *total,
                }
            })
        }
        (
            UpdateStatus::Restarting {
                seconds_remaining, ..
            },
            StubEvent::Tick,
        ) => match seconds_remaining.saturating_sub(1) {
            0 => None,
            seconds => {
                let mut next = status.clone();
                if let UpdateStatus::Restarting {
                    seconds_remaining, ..
                } = &mut next
                {
                    *seconds_remaining = seconds;
                }
                Some(next)
            }
        },
        _ => Some(status.clone()),
    }
}

/// Milliseconds until the next simulated tick, if the status animates.
pub fn stub_tick_interval_ms(status: &UpdateStatus) -> Option<u64> {
    match status {
        UpdateStatus::Checking { .. } => Some(900),
        UpdateStatus::Downloading { .. } => Some(400),
        UpdateStatus::Restarting { .. } => Some(1_000),
        _ => None,
    }
}

/// Width of the update notice caret's base (shipping `--tray-caret-span`).
pub const CARET_SPAN: f64 = 14.0;

/// Places an update notice card at the tray icon when its rect is usable, or
/// in this platform's tray corner otherwise, using the shared tray-notice policy.
pub fn placement(
    monitor: crate::tray_notice::LogicalRect,
    work_area: crate::tray_notice::LogicalRect,
    tray: Option<crate::tray_notice::LogicalRect>,
    menu_bar_at_top: bool,
    card_width: f64,
    card_height: f64,
) -> crate::tray_notice::Placement {
    crate::tray_notice::resolve_tray_notice_placement(
        monitor,
        work_area,
        tray,
        menu_bar_at_top,
        crate::tray_notice::FallbackEdge::for_current_platform(),
        card_width,
        card_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "> [!WARNING]\n> This Preview is functional, but experimental.\n\n## What's Changed\n* Improve capture parity by @joswayski in https://github.com/joswayski/captures/pull/249\n* **Fix** the `[region](https://example.com)` selector\n* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/297\n\n## New Contributors\n* @someone made their first contribution in https://github.com/joswayski/captures/pull/1\n\n**Full Changelog**: https://github.com/joswayski/captures/compare/old...new";

    fn item(text: &str, pr: Option<u64>) -> NoteItem {
        NoteItem {
            text: text.into(),
            pull_request: pr.map(|n| pull_request("joswayski", "captures", n)),
        }
    }

    fn view(show_changelog: bool) -> ViewState {
        ViewState {
            show_changelog,
            ..ViewState::default()
        }
    }

    #[test]
    fn release_notes_match_the_shipping_parser() {
        assert_eq!(
            release_note_items(SAMPLE),
            vec![
                item("Improve capture parity", Some(249)),
                item("Fix the region selector", None)
            ]
        );
        assert_eq!(
            release_note_items(
                "* Fix Linux startup crash by @devin-ai-integration[bot] in https://github.com/joswayski/captures/pull/297\n* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/297"
            ),
            vec![item("Fix Linux startup crash", Some(297))]
        );
        assert_eq!(
            release_note_items("* Keep the update notice capturable (#452)"),
            vec![item("Keep the update notice capturable", Some(452))]
        );
        assert_eq!(
            release_note_items(
                "* Fix false crash reports during Windows updates ([#520](https://github.com/joswayski/captures/pull/520))\n* Bump @vitest/mocker and vitest ([#511](https://github.com/joswayski/captures/pull/511))\n* Bump js-yaml from 4.3.1 to 4.3.2 ([#513](https://github.com/joswayski/captures/pull/513))"
            ),
            vec![item(
                "Fix false crash reports during Windows updates",
                Some(520)
            )]
        );
        // Other repositories keep their own link; numbered lists and quotes too.
        assert_eq!(
            release_note_items(
                "1. Try <https://example.com> in https://www.GitHub.com/other/repo.rs/pull/7 now\n2) > Quoted change\n# Heading\n### Also heading\n#NotAHeading"
            ),
            vec![
                NoteItem {
                    text:
                        "Try https://example.com in https://www.GitHub.com/other/repo.rs/pull/7 now"
                            .into(),
                    pull_request: Some(pull_request("other", "repo.rs", 7)),
                },
                item("Quoted change", None),
                item("#NotAHeading", None),
            ]
        );
        assert!(release_note_items("* Bumper cars are fun").len() == 1);
        assert_eq!(
            release_note_items("* See [the docs](https://example.com \"Docs\") and [x](y) now"),
            vec![item("See the docs and x now", None)]
        );
    }

    #[test]
    fn stacks_skipped_previews_and_falls_back_to_latest_notes() {
        let changelog = vec![
            ChangelogEntry {
                version: "2026.8.2705".into(),
                display_version: "2026.08.27.5".into(),
                notes: Some("* Fix the update notice".into()),
            },
            ChangelogEntry {
                version: "2026.8.2704".into(),
                display_version: "2026.08.27.4".into(),
                notes: Some("* Bump vitest from 4.1.10 to 4.1.11 (#511)".into()),
            },
            ChangelogEntry {
                version: "2026.8.2703".into(),
                display_version: "2026.08.27.3".into(),
                notes: Some("* Fix capture menu switching".into()),
            },
        ];
        let groups = stacked_release_notes(&changelog, Some("* Only the latest"), "x");
        assert_eq!(
            groups
                .iter()
                .map(|g| g.display_version.as_str())
                .collect::<Vec<_>>(),
            ["2026.08.27.5", "2026.08.27.3"]
        );
        let fallback = stacked_release_notes(&[], Some(SAMPLE), "2026.08.27.5");
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].version, "");
        assert_eq!(fallback[0].items.len(), 2);
        assert!(
            stacked_release_notes(&[], Some("* Bump js-yaml (#513)"), "2026.09.14.1").is_empty()
        );
    }

    #[test]
    fn formats_update_sizes_like_the_shipping_ui() {
        assert_eq!(format_update_size(0), "0 KB");
        assert_eq!(format_update_size(512), "0.5 KB");
        assert_eq!(format_update_size(7_340_032), "7.3 MB");
        assert_eq!(format_update_size(12_582_912), "12.6 MB");
        assert_eq!(format_update_size(1_250_000), "1.3 MB");
        assert_eq!(format_update_size(250_000_000), "250 MB");
    }

    #[test]
    fn available_shows_stacked_notes_and_update_now() {
        let status = fixture("available").unwrap();
        let p = present(Some(&status), &view(true));
        assert_eq!(p.visual_state, "available");
        assert_eq!(p.title, "Update available");
        assert_eq!(p.description, "Version 2026.08.27.5 · 12.6 MB");
        assert_eq!(p.icon, Icon::App);
        assert_eq!(p.icon_tone, IconTone::Accent);
        assert!(p.reveal_notes.is_none());
        let notes = p.notes.unwrap();
        assert!(notes.stacked);
        assert_eq!(
            notes.intro.as_deref(),
            Some("This update includes all of the following changes:")
        );
        assert_eq!(notes.groups.len(), 2);
        assert_eq!(
            notes.groups[0].items,
            vec![item(
                "Fix post-update launch notice position on macOS",
                Some(265)
            )]
        );
        assert_eq!(notes.hide.action, Action::HideNotes);
        let footer = p.footer.unwrap();
        assert_eq!(footer.dismiss.label, "Later");
        assert_eq!(footer.primary.unwrap().label, "Update now");
        assert!(p.close_warning.is_none());
        assert!(!p.dismiss_blocked);
        assert_eq!(p.card_height, 168.0 + 122.0 + 2.0 * 72.0);
    }

    #[test]
    fn hidden_notes_leave_a_compact_card_with_whats_new() {
        let status = fixture("closing").unwrap();
        let p = present(Some(&status), &view(false));
        assert!(p.notes.is_none());
        assert_eq!(p.reveal_notes.unwrap().action, Action::ShowNotes);
        assert_eq!(p.close_warning.as_deref(), Some(OPEN_CAPTURES_WARNING));
        assert_eq!(p.card_height, 168.0 + 56.0);
        let shown = present(Some(&status), &view(true));
        assert_eq!(shown.card_height, 480.0);
        let single = present(fixture("single").as_ref(), &view(true));
        assert!(!single.notes.as_ref().unwrap().stacked);
        assert!(single.notes.unwrap().intro.is_none());
        assert_eq!(single.card_height, 168.0 + 122.0);
        let manual = present(fixture("manual").as_ref(), &view(true));
        assert_eq!(manual.description, "Version 2026.08.27.5");
        assert_eq!(
            manual.footer.unwrap().primary.unwrap().label,
            "View release"
        );
    }

    #[test]
    fn downloading_and_restarting_block_dismissal_and_hide_the_footer() {
        let p = present(fixture("downloading").as_ref(), &view(true));
        assert_eq!(p.title, "Updating Captures");
        assert_eq!(p.icon, Icon::Spinner);
        let download = p.download.unwrap();
        assert_eq!(download.detail, "7.3 MB / 12.6 MB");
        assert_eq!(download.percent, Some(58));
        assert_eq!(download.percent_label.as_deref(), Some("58%"));
        assert!(p.footer.is_none());
        assert!(p.dismiss_blocked);
        assert_eq!(p.card_height, 224.0);

        let unknown = UpdateStatus::Downloading {
            current_version: "a".into(),
            current_display_version: "a".into(),
            version: "b".into(),
            display_version: "b".into(),
            downloaded: 2_000,
            total: None,
        };
        let download = present(Some(&unknown), &view(true)).download.unwrap();
        assert_eq!(download.detail, "2.0 KB downloaded");
        assert_eq!(download.percent, None);

        let p = present(fixture("restarting").as_ref(), &view(true));
        assert_eq!(p.title, "Updated");
        assert_eq!(p.description, "Version 2026.08.27.5");
        assert_eq!(p.icon, Icon::Check);
        assert_eq!(p.icon_tone, IconTone::Positive);
        assert_eq!(p.restart.unwrap().message, "Reopening in 3 seconds…");
        assert!(p.footer.is_none());
        assert!(p.dismiss_blocked);
    }

    #[test]
    fn errors_offer_try_again_and_the_download_page() {
        let p = present(fixture("error").as_ref(), &view(true));
        assert_eq!(p.title, "Update failed");
        assert_eq!(p.description, "Your current version was not changed.");
        assert_eq!(p.icon, Icon::Warning);
        let error = p.error.unwrap();
        assert!(error.message.contains("403 Forbidden"));
        assert_eq!(
            format!(
                "{}{}{}",
                error.fallback_prefix, error.fallback_link, error.fallback_suffix
            ),
            "You can also download from captur.es."
        );
        let footer = p.footer.unwrap();
        assert_eq!(footer.dismiss.label, "Close");
        assert_eq!(footer.primary.unwrap().action, Action::Install);
        assert_eq!(p.card_height, 168.0 + 96.0);

        let check_failed = UpdateStatus::Error {
            current_version: "a".into(),
            current_display_version: "a".into(),
            message: "offline".into(),
            retry_install: false,
        };
        let primary = present(Some(&check_failed), &view(true))
            .footer
            .unwrap()
            .primary
            .unwrap();
        assert_eq!(primary.action, Action::Check);

        // An install command failure over an available update keeps "Later".
        let status = fixture("available").unwrap();
        let p = present(
            Some(&status),
            &ViewState {
                show_changelog: true,
                action_error: Some("Could not start".into()),
                installing: false,
            },
        );
        assert_eq!(p.visual_state, "error");
        assert!(p.notes.is_none() && p.reveal_notes.is_none());
        let footer = p.footer.unwrap();
        assert_eq!(footer.dismiss.label, "Later");
        assert_eq!(footer.primary.unwrap().label, "Try again");
    }

    #[test]
    fn installing_disables_actions_and_blocks_escape() {
        let status = fixture("available").unwrap();
        let p = present(
            Some(&status),
            &ViewState {
                show_changelog: true,
                action_error: None,
                installing: true,
            },
        );
        let footer = p.footer.unwrap();
        assert!(!footer.dismiss.enabled && !footer.primary.unwrap().enabled);
        assert!(p.dismiss_blocked);
    }

    #[test]
    fn checking_idle_up_to_date_and_loading_copy() {
        let p = present(fixture("checking").as_ref(), &view(true));
        assert_eq!(p.title, "Checking for updates");
        assert_eq!(p.status_message.as_deref(), Some("Checking…"));
        assert!(p.footer.as_ref().unwrap().primary.is_none());
        let p = present(fixture("up-to-date").as_ref(), &view(true));
        assert_eq!(p.title, "You’re up to date");
        assert_eq!(p.description, "Version 2026.08.27.2");
        assert_eq!(
            p.status_message.as_deref(),
            Some("No updates are available.")
        );
        assert!(p.footer.as_ref().unwrap().primary.is_none());
        let p = present(None, &view(true));
        assert_eq!(p.title, "Loading update details");
        assert_eq!(p.footer.unwrap().primary.unwrap().label, "Check again");
        let idle = UpdateStatus::Idle {
            current_version: "a".into(),
            current_display_version: "a".into(),
        };
        assert_eq!(
            present(Some(&idle), &view(true)).icon_tone,
            IconTone::Neutral
        );
    }

    #[test]
    fn stub_source_simulates_install_restart_and_check_without_side_effects() {
        let mut status = fixture("available").unwrap();
        status = stub_next(&status, StubEvent::Install).unwrap();
        assert!(matches!(
            status,
            UpdateStatus::Downloading { downloaded: 0, .. }
        ));
        let mut ticks = 0;
        while matches!(status, UpdateStatus::Downloading { .. }) {
            status = stub_next(&status, StubEvent::Tick).unwrap();
            ticks += 1;
            assert!(ticks < 20);
        }
        assert_eq!(ticks, FIXTURE_STEPS as usize + 1);
        assert!(matches!(
            status,
            UpdateStatus::Restarting {
                seconds_remaining: 3,
                ..
            }
        ));
        status = stub_next(&status, StubEvent::Tick).unwrap();
        status = stub_next(&status, StubEvent::Tick).unwrap();
        assert_eq!(
            present(Some(&status), &view(true)).restart.unwrap().message,
            "Reopening in 1 seconds…"
        );
        assert_eq!(stub_next(&status, StubEvent::Tick), None);

        let error = fixture("error").unwrap();
        assert!(matches!(
            stub_next(&error, StubEvent::Install),
            Some(UpdateStatus::Downloading { .. })
        ));
        let checking = stub_next(&error, StubEvent::Check).unwrap();
        assert!(matches!(checking, UpdateStatus::Checking { .. }));
        assert!(matches!(
            stub_next(&checking, StubEvent::Tick),
            Some(UpdateStatus::UpToDate { .. })
        ));
        // Static fixtures do not animate on their own.
        let available = fixture("available").unwrap();
        assert_eq!(stub_tick_interval_ms(&available), None);
        assert_eq!(
            stub_next(&available, StubEvent::Tick),
            Some(available.clone())
        );
        for name in FIXTURES {
            assert!(fixture(name).is_some(), "{name}");
        }
        assert!(fixture("unknown").is_none());
    }

    #[test]
    fn status_json_matches_the_tauri_wire_shape() {
        let value = serde_json::to_value(fixture("downloading").unwrap()).unwrap();
        assert_eq!(value["state"], "downloading");
        assert_eq!(value["total"], 12_582_912);
        let parsed: UpdateStatus = serde_json::from_value(serde_json::json!({
            "state": "up_to_date", "current_version": "1", "current_display_version": "1"
        }))
        .unwrap();
        assert!(matches!(parsed, UpdateStatus::UpToDate { .. }));
        let action = serde_json::to_value(Action::OpenPullRequest { url: "u".into() }).unwrap();
        assert_eq!(
            action,
            serde_json::json!({"action":"open_pull_request","url":"u"})
        );
    }
}
