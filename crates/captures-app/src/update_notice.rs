//! Update notice status model, release-note parsing and presentation copy.
//!
//! Ported from the shipping Tauri surface: the status model in
//! `apps/desktop/src-tauri/src/updates.rs`, `UpdateNotice` in
//! `apps/desktop/ui/src/App.tsx` and `apps/desktop/ui/src/lib/releaseNotes.ts`.
//! Both native hosts render [`present`] so their copy stays identical.
//!
//! Explicit signed development checks reuse this layout/notes through
//! `updater::checks`, with opt-in temporary verification and no install/restart route.
//! [`fixture`] and [`stub_next`] remain a separate deterministic status source;
//! they never download, verify or install anything.
use serde::{Deserialize, Serialize};

pub const CARD_WIDTH: f64 = 400.0;
const COMPACT_HEIGHT: f64 = 168.0;
/// An update card's title and version, and the gap below them.
const UPDATE_HEADER_HEIGHT: f64 = 67.0;
/// The hairline over the change list, its gaps, and the footer bar's
/// padding around one row of buttons.
const NOTES_CHROME: f64 = 85.0;
/// The footer bar's restart note above its buttons.
pub const FOOTER_NOTE_HEIGHT: f64 = 40.0;
/// A plain bullet (about two lines) and its gap.
const NOTE_HEIGHT: f64 = 44.0;
/// A highlight: its bold title over a two-line description, and its gap.
const HIGHLIGHT_HEIGHT: f64 = 66.0;
/// Past this the notes scroll inside the box.
const MAX_HEIGHT: f64 = 560.0;
pub const WARNING_HEIGHT: f64 = 56.0;
pub const STATUS_HEIGHT: f64 = 56.0;
const ERROR_HEIGHT: f64 = 96.0;

/// The footer bar's note while an update the app installs itself is offered.
/// It always shows, so it does not come and go as captures open and close.
pub const RESTART_NOTE: &str =
    "Captures restarts to finish. Open captures reopen, unsaved screenshot edits included.";

/// Shipping `openCapturesUpdateWarning`, shown when the update closes open
/// captures: an update the app installs itself reopens them, while a manual
/// download leaves them closed with their drafts.
pub fn open_captures_warning(installable: bool) -> &'static str {
    if installable {
        "Your open captures will reopen after the update. Unsaved screenshot edits are kept."
    } else {
        "Open captures will close. Unsaved edits are kept as drafts."
    }
}
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
    /// A highlight's bold title, from notes written as `Title — description`;
    /// `text` is then the description shown under it.
    #[serde(default)]
    pub title: Option<String>,
    pub text: String,
    pub pull_request: Option<PullRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteGroup {
    pub version: String,
    pub display_version: String,
    pub items: Vec<NoteItem>,
    /// In one list of several releases, an older release's version, shown
    /// as a small link after each of its changes' titles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<VersionLink>,
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

/// The desktop OS a release highlight applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePlatform {
    MacOs,
    Windows,
    Linux,
}

impl NotePlatform {
    pub const CURRENT: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Linux
    };

    fn parse(label: &str) -> Option<Self> {
        [Self::MacOs, Self::Windows, Self::Linux]
            .into_iter()
            .find(|platform| platform.label().eq_ignore_ascii_case(label.trim()))
    }

    fn label(self) -> &'static str {
        match self {
            Self::MacOs => "macOS",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
        }
    }
}

/// ai-release-notes.mjs prefixes platform-specific highlights with a bold
/// `**macOS, Linux:**` label. Returns the platforms and the rest of the line.
fn platform_label(body: &str) -> Option<(Vec<NotePlatform>, &str)> {
    let (label, rest) = body.strip_prefix("**")?.split_once(":**")?;
    let platforms = label
        .split(',')
        .map(NotePlatform::parse)
        .collect::<Option<Vec<_>>>()?;
    Some((platforms, rest.trim_start()))
}

/// Turns GitHub's generated release Markdown into concise notice copy for
/// this OS: highlights labelled for other platforms are left out.
pub fn release_note_items(markdown: &str) -> Vec<NoteItem> {
    platform_note_items(markdown, Some(NotePlatform::CURRENT))
}

/// `release_note_items` for `platform`, or every platform's notes for `None`.
fn platform_note_items(markdown: &str, platform: Option<NotePlatform>) -> Vec<NoteItem> {
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
        let mut body = list_body(line);
        if let Some((platforms, rest)) = platform_label(body) {
            if platform.is_some_and(|platform| !platforms.contains(&platform)) {
                continue;
            }
            body = rest;
        }
        let text = plain_text(body);
        if text.is_empty()
            || is_first_contribution(&text)
            || is_first_contribution(body)
            || is_dependency_update(&text)
        {
            continue;
        }
        // ai-release-notes.mjs writes each highlight as `Title — description`.
        let (title, text) = match text.split_once(" — ") {
            Some((title, description))
                if !title.trim().is_empty()
                    && !description.trim().is_empty()
                    && title.trim().chars().count() <= 100 =>
            {
                (Some(title.trim().to_owned()), description.trim().to_owned())
            }
            _ => (None, text),
        };
        items.push(NoteItem {
            title,
            text,
            pull_request: pull_request_from_line(body),
        });
    }
    items
}

/// The tray (menu bar) update row for an update status: shipping's
/// `tray_update_item`. An available, installing or restarting update is
/// pinned above New Capture; otherwise the row stays "Check for Updates…".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayUpdateItem {
    pub label: String,
    pub enabled: bool,
    pub pin_first: bool,
}

pub fn tray_update_item(status: Option<&UpdateStatus>) -> TrayUpdateItem {
    let item = |label: String, enabled, pin_first| TrayUpdateItem {
        label,
        enabled,
        pin_first,
    };
    match status {
        Some(UpdateStatus::Available {
            changelog, notes, ..
        }) => {
            let label = match change_count(changelog, notes.as_deref()) {
                0 => "Update Available".to_owned(),
                1 => "Update Available — 1 change".to_owned(),
                count => format!("Update Available — {count} changes"),
            };
            item(label, true, true)
        }
        Some(UpdateStatus::Downloading { .. }) => item("Installing Update…".into(), false, true),
        Some(UpdateStatus::Restarting { .. }) => item("Restarting Captures…".into(), false, true),
        Some(UpdateStatus::Checking { .. }) => item("Checking for Updates…".into(), false, false),
        _ => item("Check for Updates…".into(), true, false),
    }
}

/// Shipping's tray tooltip for an update status.
pub fn tray_tooltip(status: Option<&UpdateStatus>) -> &'static str {
    match status {
        Some(UpdateStatus::Available { .. }) => "Captures — Update available",
        Some(UpdateStatus::Downloading { .. } | UpdateStatus::Restarting { .. }) => {
            "Captures — Installing update"
        }
        _ => crate::preview_channel::TRAY_NAME,
    }
}

/// Counts the changes the notice lists, so the tray and the notice agree.
fn change_count(changelog: &[ChangelogEntry], notes: Option<&str>) -> usize {
    let from_changelog: usize = changelog
        .iter()
        .filter_map(|entry| entry.notes.as_deref())
        .map(|notes| release_note_items(notes).len())
        .sum();
    if from_changelog > 0 {
        from_changelog
    } else {
        notes.map_or(0, |notes| release_note_items(notes).len())
    }
}

/// One group per skipped Preview with product-facing notes for this OS,
/// newest first.
pub fn stacked_release_notes(
    changelog: &[ChangelogEntry],
    fallback_notes: Option<&str>,
    fallback_display_version: &str,
) -> Vec<NoteGroup> {
    stacked_platform_notes(
        changelog,
        fallback_notes,
        fallback_display_version,
        Some(NotePlatform::CURRENT),
    )
}

fn stacked_platform_notes(
    changelog: &[ChangelogEntry],
    fallback_notes: Option<&str>,
    fallback_display_version: &str,
    platform: Option<NotePlatform>,
) -> Vec<NoteGroup> {
    if !changelog.is_empty() {
        return changelog
            .iter()
            .filter_map(|entry| {
                let items = entry
                    .notes
                    .as_deref()
                    .map(|notes| platform_note_items(notes, platform))
                    .unwrap_or_default();
                (!items.is_empty()).then(|| NoteGroup {
                    version: entry.version.clone(),
                    display_version: entry.display_version.clone(),
                    items,
                    tag: None,
                })
            })
            .collect();
    }
    let items = fallback_notes
        .map(|notes| platform_note_items(notes, platform))
        .unwrap_or_default();
    if items.is_empty() {
        return Vec::new();
    }
    vec![NoteGroup {
        version: String::new(),
        display_version: fallback_display_version.to_owned(),
        items,
        tag: None,
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
    // 999.5 and up would round to "1000" in the smaller unit.
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1_000.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} {}", value.round(), UNITS[unit]);
    }
    if value >= 99.95 {
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
    /// No header icon: an update's own cards lead with their title.
    None,
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
    /// Not now/Close/Escape.
    Dismiss,
    /// Update now, View release, or Try again after an install failure.
    Install,
    /// Check again, or Try again after a check failure.
    Check,
    /// Explicitly download and stage an authenticated development package.
    DownloadVerify,
    /// Cancel temporary acquisition, waiting for I/O and cleanup acknowledgement.
    CancelDownload,
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
    /// A trailing shipping icon (`crate::icons`): "external-link" when the
    /// button opens a page outside Captures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailing_icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notes {
    /// The list's accessible name; hosts do not draw it.
    pub heading: String,
    /// Several releases in one list; older releases' groups carry a `tag`.
    pub stacked: bool,
    pub intro: Option<String>,
    pub groups: Vec<NoteGroup>,
    /// Shown instead of groups when none have product-facing notes.
    pub empty: Option<String>,
    /// Follows `empty` inline when every highlight is for another OS, so the
    /// curious can read them all on the release page.
    #[serde(default)]
    pub empty_link: Option<VersionLink>,
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
    pub notes: Option<Notes>,
    pub download: Option<Download>,
    pub restart: Option<Restart>,
    pub error: Option<ErrorBlock>,
    pub status_message: Option<String>,
    /// A muted line with the restart icon above the footer bar's buttons.
    pub footer_note: Option<String>,
    pub footer: Option<Footer>,
    /// Escape and the window close button do nothing while busy.
    pub dismiss_blocked: bool,
    /// The version in `description`, which opens its GitHub release.
    pub version_link: Option<VersionLink>,
    pub card_width: f64,
    pub card_height: f64,
}

/// `text` appears once in the description; hosts render it as an external
/// link to `url`, with the external-link icon raised after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionLink {
    pub text: String,
    pub url: String,
}

/// The GitHub release page for a dated display version such as
/// `2026.11.22.2` (tag `v2026.11.22.2`); development versions have none.
pub fn version_link(display_version: &str) -> Option<VersionLink> {
    let parts: Vec<_> = display_version.split('.').collect();
    let dated = parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && parts[0].len() == 4;
    dated.then(|| VersionLink {
        text: format!("Version {display_version}"),
        url: format!("https://github.com/joswayski/captures/releases/tag/v{display_version}"),
    })
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
    /// The height the card keeps while one update proceeds; see
    /// [`ViewState::follow`].
    #[serde(default)]
    pub held_height: f64,
    /// The offered update's change list, kept on screen while it downloads
    /// and restarts; see [`ViewState::follow`].
    #[serde(default)]
    pub held_notes: Option<Box<Notes>>,
}

impl ViewState {
    /// Track a status change. From Available through Downloading,
    /// Restarting or an install error the card keeps its tallest height so
    /// far, so the notice does not jump as each step finishes; any other
    /// change (a new check, a verified update offered again) sizes itself.
    pub fn follow(&mut self, previous: Option<&UpdateStatus>, next: &UpdateStatus) {
        use UpdateStatus as S;
        let proceeding = matches!(
            next,
            S::Downloading { .. } | S::Restarting { .. } | S::Error { .. }
        ) && matches!(
            previous,
            Some(
                S::Available { .. }
                    | S::Downloading { .. }
                    | S::Restarting { .. }
                    | S::Error { .. }
            )
        );
        self.held_height = if proceeding {
            self.held_height
                .max(card_height(previous, self.show_changelog))
        } else {
            0.0
        };
        self.held_notes = match previous {
            _ if !proceeding => None,
            Some(available @ S::Available { .. }) => {
                notes(Some(available), self.show_changelog).map(Box::new)
            }
            _ => self.held_notes.take(),
        };
    }
}

fn default_true() -> bool {
    true
}

/// Card height for the transparent notice window, as shipping sizes it.
pub fn card_height(status: Option<&UpdateStatus>, show_changelog: bool) -> f64 {
    let notes = match status {
        Some(UpdateStatus::Available {
            changelog,
            notes,
            display_version,
            ..
        }) if show_changelog => {
            let groups = stacked_release_notes(changelog, notes.as_deref(), display_version);
            let items: f64 = groups
                .iter()
                .flat_map(|group| &group.items)
                .map(|item| match item.title {
                    Some(_) => HIGHLIGHT_HEIGHT,
                    None => NOTE_HEIGHT,
                })
                .sum();
            NOTES_CHROME + items.max(NOTE_HEIGHT)
        }
        _ => 0.0,
    };
    if let Some(available @ UpdateStatus::Available { .. }) = status {
        let note = if footer_note(available).is_some() {
            FOOTER_NOTE_HEIGHT
        } else {
            0.0
        };
        let notes = if notes > 0.0 { notes } else { NOTES_CHROME };
        return (UPDATE_HEADER_HEIGHT + notes + note).min(MAX_HEIGHT);
    }
    // Progress takes the footer bar's note and buttons: the same height as
    // an offered update without its list.
    if let Some(UpdateStatus::Downloading { .. } | UpdateStatus::Restarting { .. }) = status {
        return UPDATE_HEADER_HEIGHT + NOTES_CHROME + FOOTER_NOTE_HEIGHT;
    }
    let body = match status {
        Some(UpdateStatus::Error { .. }) => ERROR_HEIGHT,
        _ => STATUS_HEIGHT,
    };
    (COMPACT_HEIGHT + body).min(MAX_HEIGHT)
}

/// The footer bar's note for an offered update: what Update does to open
/// captures. A manual download only mentions them while some are open.
fn footer_note(status: &UpdateStatus) -> Option<String> {
    match status {
        UpdateStatus::Available {
            installable: true, ..
        } => Some(RESTART_NOTE.to_owned()),
        UpdateStatus::Available {
            will_close_open_captures: true,
            ..
        } => Some(open_captures_warning(false).to_owned()),
        _ => None,
    }
}

/// An offered update's change list: one list of every skipped release's
/// changes, newest first, older releases tagged with their version.
pub fn notes(status: Option<&UpdateStatus>, show_changelog: bool) -> Option<Notes> {
    let Some(UpdateStatus::Available {
        changelog,
        notes,
        display_version,
        ..
    }) = status
    else {
        return None;
    };
    if !show_changelog {
        return None;
    }
    let mut groups = stacked_release_notes(changelog, notes.as_deref(), display_version);
    let stacked = groups.len() > 1;
    if stacked {
        for group in groups.iter_mut().skip(1) {
            group.tag = Some(VersionLink {
                text: group.display_version.clone(),
                url: version_link(&group.display_version)
                    .map(|link| link.url)
                    .unwrap_or_default(),
            });
        }
    }
    let other_platforms_only = groups.is_empty()
        && !stacked_platform_notes(changelog, notes.as_deref(), display_version, None).is_empty();
    Some(Notes {
        heading: "Release notes".into(),
        stacked,
        intro: None,
        empty: groups.is_empty().then(|| {
            if other_platforms_only {
                format!(
                    "This update has no highlights for {}.",
                    NotePlatform::CURRENT.label()
                )
            } else {
                "Release notes aren’t available for this update.".to_owned()
            }
        }),
        empty_link: other_platforms_only
            .then(|| version_link(display_version))
            .flatten()
            .map(|link| VersionLink {
                text: "See all changes on GitHub".into(),
                url: link.url,
            }),
        group_empty: "Release notes aren’t available for this release.".into(),
        groups,
    })
}

fn button(label: &str, enabled: bool, action: Action) -> Button {
    Button {
        label: label.into(),
        enabled,
        action,
        trailing_icon: None,
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
        "error" => (Icon::Warning, IconTone::Signal),
        "checking" | "loading" => (Icon::Spinner, IconTone::Neutral),
        // An update's own cards lead with their title, without an icon.
        "available" | "downloading" | "restarting" => (Icon::None, IconTone::Neutral),
        "up_to_date" => (Icon::App, IconTone::Accent),
        _ => (Icon::App, IconTone::Neutral),
    };

    let (title, description) = match status {
        Some(S::Restarting {
            display_version, ..
        }) => (
            "Captures updated".into(),
            format!("Version {display_version}"),
        ),
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
                "Captures update available".into(),
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

    // Preferences (`show_update_changelog`) is the only way to hide notes.
    // Preferences (`show_update_changelog`) is the only way to hide notes.
    // The offered list stays on screen while that update downloads and restarts.
    let notes = match status {
        _ if error.is_some() => None,
        Some(S::Available { .. }) => notes(status, view.show_changelog),
        Some(S::Downloading { .. } | S::Restarting { .. }) => view.held_notes.as_deref().cloned(),
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
            message: format!("Restarting in {seconds_remaining}…"),
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

    let footer_note = match status {
        Some(available @ S::Available { .. }) if error.is_none() => footer_note(available),
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
            Some(if installable {
                button("Update", enabled, Action::Install)
            } else {
                // Opens the release page in the browser.
                Button {
                    trailing_icon: Some("external-link".into()),
                    ..button("View release", enabled, Action::Install)
                }
            })
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
                if available { "Not now" } else { "Close" },
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
        notes,
        download,
        restart,
        error: error_block,
        status_message,
        footer_note,
        footer,
        dismiss_blocked: downloading || restarting || view.installing,
        version_link: match status {
            _ if error.is_some() => None,
            Some(S::Available {
                display_version, ..
            })
            | Some(S::Downloading {
                display_version, ..
            })
            | Some(S::Restarting {
                display_version, ..
            }) => version_link(display_version),
            Some(S::UpToDate {
                current_display_version,
                ..
            }) => version_link(current_display_version),
            _ => None,
        },
        card_width: CARD_WIDTH,
        card_height: card_height(status, view.show_changelog).max(view.held_height),
    }
}

// ---------------------------------------------------------- stub source ----

const FIXTURE_CURRENT: (&str, &str) = ("2026.8.2702", "2026.08.27.2");
const FIXTURE_LATEST: (&str, &str) = ("2026.8.2705", "2026.08.27.5");
const FIXTURE_SIZE: u64 = 12_582_912;
const FIXTURE_STEPS: u64 = 6;
const RESTART_COUNTDOWN_SECONDS: u8 = 3;

/// Workbench fixture names accepted by [`fixture`].
pub const FIXTURES: [&str; 10] = [
    "available",
    "highlights",
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

/// Three releases written as AI highlights (`Title — description`).
fn highlights_fixture() -> UpdateStatus {
    let entry = |version: &str, display: &str, notes: &[&str]| ChangelogEntry {
        version: version.into(),
        display_version: display.into(),
        notes: Some(format!(
            "## What's Changed\n{}",
            notes
                .iter()
                .map(|note| format!("* {note}"))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    };
    let changelog = vec![
        entry(
            "2026.11.2203",
            "2026.11.22.3",
            &[
                "Shortcuts survive a taken chord — When another app already owns one of your shortcuts, the rest keep working instead of all going quiet.",
            ],
        ),
        entry(
            "2026.11.2202",
            "2026.11.22.2",
            &[
                "Mini previews finally get the theme memo — On macOS and Windows, mini previews now update right away when you change your theme or accent color.",
            ],
        ),
        entry(
            "2026.11.2201",
            "2026.11.22.1",
            &[
                "Screenshots keep working after macOS updates — In-app updates on macOS now relaunch Captures properly so Screen Recording keeps working.",
                "Captures goes lighter on macOS memory — Screenshots no longer hoard extra full-display copies in memory until you quit.",
            ],
        ),
    ];
    UpdateStatus::Available {
        current_version: "2026.11.2110".into(),
        current_display_version: "2026.11.21.10".into(),
        version: "2026.11.2203".into(),
        display_version: "2026.11.22.3".into(),
        notes: changelog[0].notes.clone(),
        changelog,
        installable: true,
        manual_download_url: None,
        download_size: Some(FIXTURE_SIZE),
        will_close_open_captures: false,
    }
}

/// Synthetic statuses matching the Tauri dev harness (`?view=update&update=…`).
pub fn fixture(name: &str) -> Option<UpdateStatus> {
    let current_version = FIXTURE_CURRENT.0.to_owned();
    let current_display_version = FIXTURE_CURRENT.1.to_owned();
    Some(match name {
        "available" => available_fixture(true, false, true),
        "highlights" => highlights_fixture(),
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
    #[test]
    fn tray_update_item_counts_listed_changes_and_pins_active_updates() {
        use super::*;
        let notes = "> [!WARNING]\n> Experimental.\n\n## What's Changed\n* First fix by @a in https://example.com/1\n* Second fix by @b in https://example.com/2\n* @bot made their first contribution in https://example.com/3\n\n**Full Changelog**: https://example.com";
        let available =
            |changelog: Vec<ChangelogEntry>, notes: Option<&str>| UpdateStatus::Available {
                current_version: "2026.11.2201".into(),
                current_display_version: "2026.11.22.1".into(),
                version: "2026.11.2203".into(),
                display_version: "2026.11.22.3".into(),
                notes: notes.map(str::to_owned),
                changelog,
                installable: true,
                manual_download_url: None,
                download_size: None,
                will_close_open_captures: false,
            };
        let changelog = vec![
            ChangelogEntry {
                version: "2026.11.2203".into(),
                display_version: "2026.11.22.3".into(),
                notes: Some(notes.into()),
            },
            ChangelogEntry {
                version: "2026.11.2202".into(),
                display_version: "2026.11.22.2".into(),
                notes: Some("* Third fix by @c in https://example.com/4".into()),
            },
        ];
        let item = tray_update_item(Some(&available(changelog, Some(notes))));
        assert_eq!(item.label, "Update Available — 3 changes");
        assert!(item.enabled && item.pin_first);
        assert_eq!(
            tray_update_item(Some(&available(vec![], Some("* One fix")))).label,
            "Update Available — 1 change"
        );
        assert_eq!(
            tray_update_item(Some(&available(vec![], None))).label,
            "Update Available"
        );
        let downloading = UpdateStatus::Downloading {
            current_version: "a".into(),
            current_display_version: "a".into(),
            version: "b".into(),
            display_version: "b".into(),
            downloaded: 1,
            total: None,
        };
        let item = tray_update_item(Some(&downloading));
        assert_eq!(item.label, "Installing Update…");
        assert!(!item.enabled && item.pin_first);
        assert_eq!(
            tray_tooltip(Some(&downloading)),
            "Captures — Installing update"
        );
        let idle = tray_update_item(None);
        assert_eq!(idle.label, "Check for Updates…");
        assert!(idle.enabled && !idle.pin_first);
        assert_eq!(tray_tooltip(None), crate::preview_channel::TRAY_NAME);
    }

    use super::*;

    const SAMPLE: &str = "> [!WARNING]\n> This Preview is functional, but experimental.\n\n## What's Changed\n* Improve capture parity by @joswayski in https://github.com/joswayski/captures/pull/249\n* **Fix** the `[region](https://example.com)` selector\n* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/297\n\n## New Contributors\n* @someone made their first contribution in https://github.com/joswayski/captures/pull/1\n\n**Full Changelog**: https://github.com/joswayski/captures/compare/old...new";

    fn item(text: &str, pr: Option<u64>) -> NoteItem {
        NoteItem {
            title: None,
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
                    title: None,
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
    fn highlights_split_into_a_bold_title_and_grow_the_card() {
        let notes = "## What's Changed\n* Mini previews get the memo — They follow your theme now.\n* Plain fix — \n* Fix a crash (#12)";
        assert_eq!(
            release_note_items(notes),
            vec![
                NoteItem {
                    title: Some("Mini previews get the memo".into()),
                    text: "They follow your theme now.".into(),
                    pull_request: None,
                },
                item("Plain fix —", None),
                item("Fix a crash", Some(12)),
            ]
        );
        let status = UpdateStatus::Available {
            current_version: "2026.11.2201".into(),
            current_display_version: "2026.11.22.1".into(),
            version: "2026.11.2202".into(),
            display_version: "2026.11.22.2".into(),
            notes: Some(notes.into()),
            changelog: Vec::new(),
            installable: true,
            manual_download_url: None,
            download_size: None,
            will_close_open_captures: false,
        };
        assert_eq!(
            card_height(Some(&status), true),
            // Header, list chrome and footer bar, a highlight, two bullets,
            // the restart note.
            67.0 + 85.0 + 66.0 + 2.0 * 44.0 + 40.0
        );
        assert_eq!(version_link("0.1.0"), None);
        assert_eq!(version_link("2026.11.2202"), None);
    }

    #[test]
    fn highlights_labelled_for_other_platforms_are_left_out() {
        let notes = "## What's Changed\n* **Windows:** Copy text — Select it in Preferences.\n* **macOS, Linux:** Tray follows theme — It matches your menu bar.\n* Shared fix — Everyone gets it.\n* **Not a platform:** Odd — Stays as written.";
        let highlight = |title: &str, text: &str| NoteItem {
            title: Some(title.into()),
            text: text.into(),
            pull_request: None,
        };
        let titles = |platform| {
            platform_note_items(notes, platform)
                .into_iter()
                .map(|item| item.title.unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(Some(NotePlatform::MacOs)),
            ["Tray follows theme", "Shared fix", "Not a platform: Odd"]
        );
        assert_eq!(
            titles(Some(NotePlatform::Windows)),
            ["Copy text", "Shared fix", "Not a platform: Odd"]
        );
        assert_eq!(
            platform_note_items(notes, Some(NotePlatform::Linux))[0],
            highlight("Tray follows theme", "It matches your menu bar.")
        );

        // A release with only another platform's highlights says so.
        let other = match NotePlatform::CURRENT {
            NotePlatform::Windows => "macOS",
            _ => "Windows",
        };
        let status = UpdateStatus::Available {
            current_version: "2026.11.2201".into(),
            current_display_version: "2026.11.22.1".into(),
            version: "2026.11.2202".into(),
            display_version: "2026.11.22.2".into(),
            notes: Some(format!("* **{other}:** Copy text — Select it.")),
            changelog: Vec::new(),
            installable: true,
            manual_download_url: None,
            download_size: None,
            will_close_open_captures: false,
        };
        let notes = present(Some(&status), &view(true)).notes.unwrap();
        assert!(notes.groups.is_empty());
        assert_eq!(
            notes.empty.unwrap(),
            format!(
                "This update has no highlights for {}.",
                NotePlatform::CURRENT.label()
            )
        );
        assert_eq!(
            notes.empty_link,
            Some(VersionLink {
                text: "See all changes on GitHub".into(),
                url: "https://github.com/joswayski/captures/releases/tag/v2026.11.22.2".into(),
            })
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
        // Just under a unit boundary moves up instead of reading "1000 KB".
        assert_eq!(format_update_size(999_400), "999 KB");
        assert_eq!(format_update_size(999_600), "1.0 MB");
        assert_eq!(format_update_size(99_960), "100 KB");
    }

    #[test]
    fn available_shows_one_list_and_update() {
        let status = fixture("available").unwrap();
        let p = present(Some(&status), &view(true));
        assert_eq!(p.visual_state, "available");
        assert_eq!(p.title, "Captures update available");
        assert_eq!(p.description, "Version 2026.08.27.5 · 12.6 MB");
        assert_eq!(p.icon, Icon::None);
        let notes = p.notes.unwrap();
        assert!(notes.stacked);
        assert!(notes.intro.is_none());
        assert_eq!(notes.groups.len(), 2);
        // The newest release is untagged; an older one links its own release.
        assert!(notes.groups[0].tag.is_none());
        assert_eq!(
            notes.groups[1].tag,
            Some(VersionLink {
                text: "2026.08.27.3".into(),
                url: "https://github.com/joswayski/captures/releases/tag/v2026.08.27.3".into(),
            })
        );
        assert_eq!(
            notes.groups[0].items,
            vec![item(
                "Fix post-update launch notice position on macOS",
                Some(265)
            )]
        );
        let footer = p.footer.unwrap();
        assert_eq!(footer.dismiss.label, "Not now");
        let update = footer.primary.unwrap();
        assert_eq!(update.label, "Update");
        assert_eq!(update.action, Action::Install);
        assert!(update.trailing_icon.is_none());
        // Shown with nothing open, so it does not come and go with an editor.
        assert_eq!(p.footer_note.as_deref(), Some(RESTART_NOTE));
        assert!(!p.dismiss_blocked);
        // Header, list chrome and footer bar, two bullets, the restart note.
        assert_eq!(p.card_height, 67.0 + 85.0 + 2.0 * 44.0 + 40.0);
        let link = p.version_link.unwrap();
        assert_eq!(link.text, "Version 2026.08.27.5");
        assert!(p.description.starts_with(&link.text));
        assert_eq!(
            link.url,
            "https://github.com/joswayski/captures/releases/tag/v2026.08.27.5"
        );
    }

    #[test]
    fn notes_turned_off_in_preferences_leave_a_compact_card() {
        let status = fixture("closing").unwrap();
        let p = present(Some(&status), &view(false));
        assert!(p.notes.is_none());
        // Open captures change nothing: the same restart note shows.
        assert_eq!(p.footer_note.as_deref(), Some(RESTART_NOTE));
        assert_eq!(p.card_height, 67.0 + 85.0 + 40.0);
        let shown = present(Some(&status), &view(true));
        assert_eq!(shown.card_height, 67.0 + 85.0 + 88.0 + 40.0);
        let single = present(fixture("single").as_ref(), &view(true));
        let notes = single.notes.as_ref().unwrap();
        assert!(!notes.stacked);
        assert!(notes.groups.iter().all(|group| group.tag.is_none()));
        assert_eq!(single.card_height, 67.0 + 85.0 + 44.0 + 40.0);
        let manual = present(fixture("manual").as_ref(), &view(true));
        assert_eq!(manual.description, "Version 2026.08.27.5");
        // Nothing open and nothing restarts: no note.
        assert!(manual.footer_note.is_none());
        // It opens the release page, so it carries the trailing link icon.
        let view_release = manual.footer.unwrap().primary.unwrap();
        assert_eq!(view_release.label, "View release");
        assert_eq!(view_release.trailing_icon.as_deref(), Some("external-link"));
    }

    #[test]
    fn a_manual_update_warns_that_open_captures_close() {
        let status = available_fixture(false, true, false);
        let p = present(Some(&status), &view(true));
        assert_eq!(p.footer.unwrap().primary.unwrap().label, "View release");
        assert_eq!(
            p.footer_note.as_deref(),
            Some("Open captures will close. Unsaved edits are kept as drafts.")
        );
        // Shipping hides it while an action error shows.
        let failed = ViewState {
            action_error: Some("GitHub is unavailable".into()),
            ..view(true)
        };
        assert!(present(Some(&status), &failed).footer_note.is_none());
    }

    #[test]
    fn one_update_keeps_its_card_and_list_until_a_new_check() {
        let available = fixture("available").unwrap();
        let offered = present(Some(&available), &view(true));
        let mut view = view(true);
        let mut previous = available;
        for name in ["downloading", "restarting"] {
            let next = fixture(name).unwrap();
            view.follow(Some(&previous), &next);
            let p = present(Some(&next), &view);
            assert_eq!(p.card_height, offered.card_height, "{name}");
            assert_eq!(p.notes, offered.notes, "{name}");
            assert!(p.card_height > card_height(Some(&next), true));
            previous = next;
        }
        let checking = fixture("checking").unwrap();
        view.follow(Some(&previous), &checking);
        let p = present(Some(&checking), &view);
        assert_eq!(p.card_height, 224.0);
        assert!(p.notes.is_none());
    }

    #[test]
    fn downloading_and_restarting_block_dismissal_and_hide_the_footer() {
        let p = present(fixture("downloading").as_ref(), &view(true));
        assert_eq!(p.title, "Updating Captures");
        assert_eq!(p.icon, Icon::None);
        let download = p.download.unwrap();
        assert_eq!(download.detail, "7.3 MB / 12.6 MB");
        assert_eq!(download.percent, Some(58));
        assert_eq!(download.percent_label.as_deref(), Some("58%"));
        assert!(p.footer.is_none());
        assert!(p.dismiss_blocked);
        // As tall as the offered card without its list, so nothing jumps.
        assert_eq!(p.card_height, 67.0 + 85.0 + 40.0);

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
        assert_eq!(p.title, "Captures updated");
        assert_eq!(p.description, "Version 2026.08.27.5");
        assert_eq!(p.icon, Icon::None);
        assert_eq!(p.restart.unwrap().message, "Restarting in 3…");
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

        // An install command failure over an available update keeps "Not now".
        let status = fixture("available").unwrap();
        let p = present(
            Some(&status),
            &ViewState {
                action_error: Some("Could not start".into()),
                ..view(true)
            },
        );
        assert_eq!(p.visual_state, "error");
        assert!(p.notes.is_none());
        let footer = p.footer.unwrap();
        assert_eq!(footer.dismiss.label, "Not now");
        assert_eq!(footer.primary.unwrap().label, "Try again");
    }

    #[test]
    fn installing_disables_actions_and_blocks_escape() {
        let status = fixture("available").unwrap();
        let p = present(
            Some(&status),
            &ViewState {
                installing: true,
                ..view(true)
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
            "Restarting in 1…"
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
