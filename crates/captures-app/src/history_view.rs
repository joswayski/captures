//! Shared Capture History presentation, matching the shipping `CaptureHistory`
//! and `HistoryCard` in `apps/desktop/ui/src/App.tsx` and the `.history-*` rules
//! in `apps/desktop/ui/src/styles/windows.css`. Native hosts render these strings,
//! card actions and grid metrics instead of re-deriving copy, formatting or layout.
//! Nothing here performs I/O except [`media_missing`], which hosts call off the
//! UI thread.

use std::{ops::Range, path::Path};

use captures_history::{ArtifactKind, HistoryEntry};
use chrono::{DateTime, Local, TimeZone};
use serde::Serialize;

use crate::{preview::format_file_size, recording_timeline::format_recording_time};

/// Shipping two-step Delete / Delete all revert to their idle state after 4 s.
pub const CONFIRM_TIMEOUT_MS: u64 = 4_000;
/// Shipping shows "✓ Restored" (and "Saved") feedback for 2.5 s.
pub const ACTION_FEEDBACK_MS: u64 = 2_500;
/// `.history-grid`: `repeat(auto-fill, minmax(252px, 1fr))` with `gap: var(--s-6)`.
pub const GRID_MIN_CARD_WIDTH: f64 = 252.0;
pub const GRID_GAP: f64 = 16.0;
/// `.history-image-wrap` height plus its 1 px bottom border.
pub const CARD_IMAGE_HEIGHT: f64 = 168.0;
pub const CARD_DIVIDER: f64 = 1.0;
/// `.history-card-body`: 12 px padding, date (18), 4 px gap, details (16), one
/// optional dropped-frame line (4 + 16), 6 px action margin, a 32 px action row
/// and 12 px padding. Every card reserves the warning line so rows stay aligned.
pub const CARD_BODY_HEIGHT: f64 = 120.0;
pub const CARD_HEIGHT: f64 = CARD_IMAGE_HEIGHT + CARD_DIVIDER + CARD_BODY_HEIGHT;
/// Top of a card's action row. Cards in a grown row stretch (CSS grid rows fit
/// their tallest card) but keep their actions here, like shipping.
pub const CARD_ACTIONS_TOP: f64 = CARD_HEIGHT - 12.0 - 32.0;
/// `.history-card-error` under the actions: `margin-top: var(--s-3)` and
/// `padding: var(--s-3) var(--s-4)` around `--text-xs` / `--leading-snug` text.
pub const CARD_ERROR_MARGIN: f64 = 6.0;
pub const CARD_ERROR_PADDING_Y: f64 = 6.0;
pub const CARD_ERROR_PADDING_X: f64 = 8.0;

/// Height a card's `.history-card-error` adds for wrapped text `text_height`
/// points tall; the body's bottom padding stays below it.
pub fn card_error_extra(text_height: f64) -> f64 {
    if text_height.is_finite() && text_height > 0.0 {
        CARD_ERROR_MARGIN + 2.0 * CARD_ERROR_PADDING_Y + text_height
    } else {
        0.0
    }
}

/// Window copy. Field names are stable ABI keys for AppKit.
#[derive(Clone, Debug, Serialize)]
pub struct Copy {
    pub eyebrow: &'static str,
    pub title: &'static str,
    pub lede: &'static str,
    pub loading: &'static str,
    pub empty_title: &'static str,
    pub empty_body: &'static str,
    pub filtered_empty: &'static str,
    pub filter_group_label: &'static str,
    pub grid_label: &'static str,
    pub delete_all: &'static str,
    pub delete_all_confirm: &'static str,
    pub delete_all_busy: &'static str,
    pub delete_all_label: &'static str,
    pub delete_all_confirm_label: &'static str,
    pub cancel: &'static str,
    pub cancel_label: &'static str,
    pub missing: &'static str,
    pub recovery_title: &'static str,
    pub recovery_help: &'static str,
}

pub const COPY: Copy = Copy {
    eyebrow: "On this device",
    title: "Capture History",
    lede: "Screenshots, videos, GIFs, and interrupted recordings you can recover all appear here for 30 days.",
    loading: "Loading history…",
    empty_title: "No captures yet",
    empty_body: "New screenshots, videos, and GIFs appear here automatically.",
    filtered_empty: "No captures match this filter.",
    filter_group_label: "Filter captures",
    grid_label: "Recent captures",
    delete_all: "Delete all",
    delete_all_confirm: "Delete all forever",
    delete_all_busy: "Deleting…",
    delete_all_label: "Delete all captures",
    delete_all_confirm_label: "Confirm delete all captures",
    cancel: "Cancel",
    cancel_label: "Cancel delete all captures",
    missing: "File missing",
    recovery_title: "Interrupted recordings",
    recovery_help: "These recordings stopped before Captures could finish saving them. Recover one to add its playable segments to Capture History, or discard it.",
};

pub fn copy() -> &'static Copy {
    &COPY
}

pub fn load_error(error: &str) -> String {
    format!("Couldn’t load capture history: {error}")
}

pub fn clear_error(error: &str) -> String {
    format!("Couldn’t delete capture history: {error}")
}

/// A card button or context-menu command. Delete is separate: it is always the
/// card's trash control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardAction {
    Edit,
    /// Shipping History Restore: bring a screenshot back as a floating preview.
    Restore,
    Copy,
    SaveImage,
    SaveFile,
    ShowInFolder,
}

impl CardAction {
    /// Every action, in ABI order.
    pub const ALL: [Self; 6] = [
        Self::Edit,
        Self::Restore,
        Self::Copy,
        Self::SaveImage,
        Self::SaveFile,
        Self::ShowInFolder,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Restore => "Restore",
            Self::Copy => "Copy image",
            Self::SaveImage => "Save image",
            Self::SaveFile => "Save file",
            Self::ShowInFolder => "Show in Folder",
        }
    }

    /// Shipping in-flight labels ("Opening…", "Saving…", "Showing…").
    pub const fn busy_label(self) -> &'static str {
        match self {
            Self::Edit => "Opening…",
            Self::Restore => "Restoring…",
            Self::Copy => "Copying…",
            Self::SaveImage | Self::SaveFile => "Saving…",
            Self::ShowInFolder => "Showing…",
        }
    }

    /// Shipping success label, shown with the check icon for
    /// [`ACTION_FEEDBACK_MS`] ("✓ Restored"). Other actions have none.
    pub const fn done_label(self) -> Option<&'static str> {
        match self {
            Self::Restore => Some("Restored"),
            _ => None,
        }
    }

    /// Shipping `title` tooltip on the card button.
    pub const fn tooltip(self) -> Option<&'static str> {
        match self {
            Self::Restore => Some("Bring this screenshot back as a floating preview"),
            Self::SaveFile => Some("Save a permanent copy to your Captures folder"),
            _ => None,
        }
    }

    /// Leading shipping icon ([`crate::icons`] name) on the card button.
    pub const fn icon(self) -> Option<&'static str> {
        match self {
            Self::Edit => Some("edit"),
            Self::Restore => Some("restore"),
            Self::SaveImage | Self::SaveFile => Some("save"),
            Self::Copy | Self::ShowInFolder => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Card {
    pub kind_label: &'static str,
    pub date: String,
    /// "W × H · size" plus " · m:ss" for recordings.
    pub details: String,
    pub warning: Option<String>,
    /// A recording whose media file is gone ("File missing").
    pub missing: bool,
    pub image_label: &'static str,
    /// Accessible name of the thumbnail's open action; none when missing.
    pub open_label: Option<&'static str>,
    pub delete_label: &'static str,
    pub delete_confirm_label: &'static str,
    pub delete_confirm_title: &'static str,
    /// Missing entries are removed with one click, like shipping.
    pub delete_requires_confirmation: bool,
    /// The card's two action buttons (empty when the recording is missing).
    pub actions: Vec<CardAction>,
    /// Secondary-click commands, excluding Delete.
    pub menu: Vec<CardAction>,
}

pub fn card(entry: &HistoryEntry, missing: bool) -> Card {
    card_in(entry, missing, &Local)
}

pub fn card_in<Tz: TimeZone>(entry: &HistoryEntry, missing: bool, zone: &Tz) -> Card
where
    Tz::Offset: std::fmt::Display,
{
    let recording = entry.kind.is_recording();
    let missing = recording && missing;
    let saved = entry.saved_path.is_some();
    let mut details = format!(
        "{} × {} · {}",
        entry.width,
        entry.height,
        format_file_size(entry.size_bytes)
    );
    if recording {
        details.push_str(" · ");
        details.push_str(&format_recording_time(
            entry.duration_ms.unwrap_or_default(),
        ));
    }
    let (actions, menu) = match (recording, missing) {
        (_, true) => (Vec::new(), Vec::new()),
        // Shipping screenshot cards show Edit and Restore; Save image and
        // Show in Folder stay reachable from the native secondary-click menu.
        (false, false) => {
            let mut menu = vec![
                CardAction::Edit,
                CardAction::Restore,
                CardAction::Copy,
                CardAction::SaveImage,
            ];
            if saved {
                menu.push(CardAction::ShowInFolder);
            }
            (vec![CardAction::Edit, CardAction::Restore], menu)
        }
        (true, false) => {
            let export = if saved {
                CardAction::ShowInFolder
            } else {
                CardAction::SaveFile
            };
            (
                vec![CardAction::Edit, export],
                vec![CardAction::Edit, export],
            )
        }
    };
    Card {
        kind_label: kind_label(entry.kind),
        date: format_date_in(&entry.created_at, zone),
        details,
        warning: recording
            .then(|| dropped_frames_warning(entry.dropped_frames))
            .flatten(),
        missing,
        image_label: match entry.kind {
            ArtifactKind::Screenshot => "Screenshot from capture history",
            ArtifactKind::Video => "Video recording poster",
            ArtifactKind::Gif => "GIF recording poster",
        },
        open_label: match (entry.kind, missing) {
            (_, true) => None,
            (ArtifactKind::Screenshot, _) => Some("Open screenshot in editor"),
            (ArtifactKind::Video, _) => Some("Open video in editor"),
            (ArtifactKind::Gif, _) => Some("Open GIF in editor"),
        },
        delete_label: if missing {
            "Remove missing entry"
        } else {
            "Delete from History"
        },
        delete_confirm_label: "Confirm permanent deletion",
        delete_confirm_title: "Delete forever",
        delete_requires_confirmation: !missing,
        actions,
        menu,
    }
}

pub const fn kind_label(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Screenshot => "Screenshot",
        ArtifactKind::Video => "Video",
        ArtifactKind::Gif => "GIF",
    }
}

/// Shipping `Intl.DateTimeFormat(undefined, {dateStyle: "medium", timeStyle:
/// "short"})` in the en-US form, in the local time zone.
pub fn format_date(created_at: &str) -> String {
    format_date_in(created_at, &Local)
}

fn format_date_in<Tz: TimeZone>(created_at: &str, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    DateTime::parse_from_rfc3339(created_at)
        .map(|date| {
            date.with_timezone(zone)
                .format("%b %-d, %Y, %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|_| "Unknown date".to_owned())
}

pub fn dropped_frames_warning(dropped: u64) -> Option<String> {
    (dropped > 0).then(|| {
        format!(
            "{} frame{} dropped while recording",
            group_thousands(dropped),
            if dropped == 1 { "" } else { "s" }
        )
    })
}

fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Shipping `missing`: a recording whose media file is not on disk. Screenshots
/// are never missing (their private PNG is the History entry). Performs I/O.
pub fn media_missing(root: &Path, entry: &HistoryEntry) -> bool {
    entry.kind.is_recording()
        && entry
            .recording_media_path(root)
            .is_none_or(|path| !path.is_file())
}

/// Auto-fill card grid for one content width.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Grid {
    pub columns: usize,
    pub card_width: f64,
    pub card_height: f64,
    pub gap: f64,
}

pub fn grid(available_width: f64) -> Grid {
    let width = if available_width.is_finite() {
        available_width.max(0.0)
    } else {
        0.0
    };
    let columns = (((width + GRID_GAP) / (GRID_MIN_CARD_WIDTH + GRID_GAP)).floor() as usize).max(1);
    let card_width = ((width - GRID_GAP * (columns - 1) as f64) / columns as f64).max(0.0);
    Grid {
        columns,
        card_width,
        card_height: CARD_HEIGHT,
        gap: GRID_GAP,
    }
}

/// The grid in a window that may be compact: shipping's
/// `@media (max-width: 720px)` uses one full-width card column.
pub fn grid_in_window(available_width: f64, compact: bool) -> Grid {
    let grid = grid(available_width);
    if compact {
        Grid {
            columns: 1,
            card_width: if available_width.is_finite() {
                available_width.max(0.0)
            } else {
                0.0
            },
            ..grid
        }
    } else {
        grid
    }
}

impl Grid {
    pub fn rows(&self, count: usize) -> usize {
        count.div_ceil(self.columns)
    }

    pub fn row_stride(&self) -> f64 {
        self.card_height + self.gap
    }

    pub fn content_height(&self, count: usize) -> f64 {
        let rows = self.rows(count);
        if rows == 0 {
            0.0
        } else {
            rows as f64 * self.card_height + (rows - 1) as f64 * self.gap
        }
    }

    /// Top-left of card `index`, relative to the grid origin.
    pub fn origin(&self, index: usize) -> (f64, f64) {
        let column = index % self.columns;
        let row = index / self.columns;
        (
            column as f64 * (self.card_width + self.gap),
            row as f64 * self.row_stride(),
        )
    }

    /// Rows intersecting the viewport, for virtualized hosts.
    pub fn visible_rows(
        &self,
        count: usize,
        scroll_top: f64,
        viewport_height: f64,
    ) -> Range<usize> {
        let rows = self.rows(count);
        if rows == 0 || !scroll_top.is_finite() || !viewport_height.is_finite() {
            return 0..0;
        }
        let stride = self.row_stride();
        let first = (scroll_top.max(0.0) / stride).floor() as usize;
        let last = ((scroll_top.max(0.0) + viewport_height.max(0.0)) / stride).floor() as usize + 1;
        first.min(rows)..last.min(rows)
    }

    /// Card under a grid-relative point, excluding gaps.
    pub fn index_at(&self, x: f64, y: f64, count: usize) -> Option<usize> {
        if !(x >= 0.0 && y >= 0.0) {
            return None;
        }
        let column = (x / (self.card_width + self.gap)).floor() as usize;
        let row = (y / self.row_stride()).floor() as usize;
        let inside = x - column as f64 * (self.card_width + self.gap) < self.card_width
            && y - row as f64 * self.row_stride() < self.card_height;
        let index = row * self.columns + column;
        (column < self.columns && inside && index < count).then_some(index)
    }

    /// Top of `row` when `extras` rows have grown.
    pub fn row_top(&self, row: usize, extras: &RowExtras) -> f64 {
        row as f64 * self.row_stride() + extras.before(row)
    }

    /// Card height in `row`: every card in a grown row stretches to it.
    pub fn row_card_height(&self, row: usize, extras: &RowExtras) -> f64 {
        self.card_height + extras.of(row)
    }

    /// [`Self::content_height`] with grown rows.
    pub fn content_height_with(&self, count: usize, extras: &RowExtras) -> f64 {
        let rows = self.rows(count);
        self.content_height(count) + extras.before(rows)
    }

    /// [`Self::origin`] with grown rows.
    pub fn origin_with(&self, index: usize, extras: &RowExtras) -> (f64, f64) {
        let (x, _) = self.origin(index);
        (x, self.row_top(index / self.columns, extras))
    }

    /// [`Self::visible_rows`] with grown rows.
    pub fn visible_rows_with(
        &self,
        count: usize,
        scroll_top: f64,
        viewport_height: f64,
        extras: &RowExtras,
    ) -> Range<usize> {
        if extras.is_empty() {
            return self.visible_rows(count, scroll_top, viewport_height);
        }
        let rows = self.rows(count);
        if rows == 0 || !scroll_top.is_finite() || !viewport_height.is_finite() {
            return 0..0;
        }
        let top = scroll_top.max(0.0);
        let bottom = top + viewport_height.max(0.0);
        // The last row starting at or above `y` (row tops only increase).
        let row_at = |y: f64| {
            let (mut low, mut high) = (0, rows);
            while high - low > 1 {
                let middle = (low + high) / 2;
                if self.row_top(middle, extras) <= y {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            low
        };
        row_at(top)..(row_at(bottom) + 1).min(rows)
    }

    /// Keyboard navigation: the card `delta_columns`/`delta_rows` away, clamped.
    pub fn step(
        &self,
        index: usize,
        count: usize,
        delta_columns: isize,
        delta_rows: isize,
    ) -> usize {
        if count == 0 {
            return 0;
        }
        let target = index as isize + delta_columns + delta_rows * self.columns as isize;
        target.clamp(0, count as isize - 1) as usize
    }
}

/// Rows grown by a card's `.history-card-error`, as (row, extra height),
/// keeping each row's tallest card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowExtras(Vec<(usize, f64)>);

impl RowExtras {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn grow(&mut self, row: usize, extra: f64) {
        if !(extra.is_finite() && extra > 0.0) {
            return;
        }
        match self.0.binary_search_by_key(&row, |(grown, _)| *grown) {
            Ok(index) => self.0[index].1 = self.0[index].1.max(extra),
            Err(index) => self.0.insert(index, (row, extra)),
        }
    }

    /// Extra height of `row`.
    pub fn of(&self, row: usize) -> f64 {
        self.0
            .binary_search_by_key(&row, |(grown, _)| *grown)
            .map_or(0.0, |index| self.0[index].1)
    }

    /// Total extra height of the rows above `row`.
    pub fn before(&self, row: usize) -> f64 {
        self.0
            .iter()
            .take_while(|(grown, _)| *grown < row)
            .map(|(_, extra)| extra)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn entry(kind: ArtifactKind) -> HistoryEntry {
        serde_json::from_value(serde_json::json!({
            "id": "a", "kind": kind, "preview_url": "", "full_url": "",
            "width": 1920, "height": 1080, "size_bytes": 1_234_567,
            "created_at": "2026-09-26T15:04:05Z",
        }))
        .unwrap()
    }

    #[test]
    fn screenshot_card_matches_shipping_copy_and_actions() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let mut screenshot = entry(ArtifactKind::Screenshot);
        let card = card_in(&screenshot, true, &utc);
        assert_eq!(card.date, "Sep 26, 2026, 3:04 PM");
        assert_eq!(card.details, "1920 × 1080 · 1.2 MB");
        assert!(!card.missing, "screenshots are never missing");
        assert_eq!(card.warning, None);
        assert_eq!(card.open_label, Some("Open screenshot in editor"));
        assert_eq!(card.delete_label, "Delete from History");
        assert!(card.delete_requires_confirmation);
        assert_eq!(card.actions, [CardAction::Edit, CardAction::Restore]);
        assert_eq!(
            card.menu,
            [
                CardAction::Edit,
                CardAction::Restore,
                CardAction::Copy,
                CardAction::SaveImage
            ]
        );
        screenshot.saved_path = Some("/tmp/export.png".into());
        let saved = card_in(&screenshot, false, &utc);
        // Shipping keeps Restore after export; Show in Folder is menu-only.
        assert_eq!(saved.actions, [CardAction::Edit, CardAction::Restore]);
        assert_eq!(saved.menu.last(), Some(&CardAction::ShowInFolder));
        let offset = FixedOffset::west_opt(7 * 3600).unwrap();
        assert_eq!(
            card_in(&screenshot, false, &offset).date,
            "Sep 26, 2026, 8:04 AM"
        );
    }

    #[test]
    fn recording_cards_cover_duration_drops_saved_and_missing() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let mut video = entry(ArtifactKind::Video);
        video.duration_ms = Some(3_725_000);
        video.dropped_frames = 1_234;
        let card = card_in(&video, false, &utc);
        assert_eq!(card.details, "1920 × 1080 · 1.2 MB · 1:02:05");
        assert_eq!(
            card.warning.as_deref(),
            Some("1,234 frames dropped while recording")
        );
        assert_eq!(card.actions, [CardAction::Edit, CardAction::SaveFile]);
        assert_eq!(card.open_label, Some("Open video in editor"));
        video.saved_path = Some("/tmp/out.mp4".into());
        video.dropped_frames = 1;
        let saved = card_in(&video, false, &utc);
        assert_eq!(saved.actions, [CardAction::Edit, CardAction::ShowInFolder]);
        assert_eq!(
            saved.warning.as_deref(),
            Some("1 frame dropped while recording")
        );
        let missing = card_in(&video, true, &utc);
        assert!(missing.missing);
        assert!(missing.actions.is_empty() && missing.menu.is_empty());
        assert_eq!(missing.open_label, None);
        assert_eq!(missing.delete_label, "Remove missing entry");
        assert!(!missing.delete_requires_confirmation);
        let gif = card_in(&entry(ArtifactKind::Gif), false, &utc);
        assert_eq!(gif.details, "1920 × 1080 · 1.2 MB · 0:00");
        assert_eq!(gif.image_label, "GIF recording poster");
        assert_eq!(gif.open_label, Some("Open GIF in editor"));
    }

    #[test]
    fn restore_matches_the_shipping_history_button() {
        let restore = CardAction::Restore;
        assert_eq!(restore.label(), "Restore");
        assert_eq!(restore.busy_label(), "Restoring…");
        assert_eq!(restore.done_label(), Some("Restored"));
        assert_eq!(
            restore.tooltip(),
            Some("Bring this screenshot back as a floating preview")
        );
        assert_eq!(ACTION_FEEDBACK_MS, 2_500);
        // Shipping History recording `Save file` title (App.tsx).
        assert_eq!(
            CardAction::SaveFile.tooltip(),
            Some("Save a permanent copy to your Captures folder")
        );
        for action in CardAction::ALL {
            if let Some(icon) = action.icon() {
                assert!(
                    crate::icons::paths(icon).is_some(),
                    "{icon} is not a shipping icon"
                );
            }
            if action != CardAction::Restore {
                assert_eq!(action.done_label(), None);
            }
            if !matches!(action, CardAction::Restore | CardAction::SaveFile) {
                assert_eq!(action.tooltip(), None);
            }
        }
        // Recordings never offer Restore (shipping rejects non-screenshots).
        for kind in [ArtifactKind::Video, ArtifactKind::Gif] {
            let card = card(&entry(kind), false);
            assert!(!card.actions.contains(&restore) && !card.menu.contains(&restore));
        }
    }

    #[test]
    fn invalid_dates_and_grouping_match_shipping() {
        assert_eq!(format_date("yesterday"), "Unknown date");
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000), "1,000");
        assert_eq!(group_thousands(12_345_678), "12,345,678");
        assert_eq!(dropped_frames_warning(0), None);
        assert_eq!(load_error("x"), "Couldn’t load capture history: x");
        assert_eq!(clear_error("x"), "Couldn’t delete capture history: x");
    }

    #[test]
    fn compact_windows_use_one_full_width_column() {
        // A 700 px window's 588 px content would fit two cards.
        assert_eq!(grid_in_window(588.0, false).columns, 2);
        let compact = grid_in_window(588.0, true);
        assert_eq!((compact.columns, compact.card_width), (1, 588.0));
        assert_eq!(grid_in_window(952.0, false), grid(952.0));
        assert_eq!(grid_in_window(f64::NAN, true).card_width, 0.0);
    }

    #[test]
    fn card_errors_grow_their_row_like_a_css_grid() {
        let wide = grid(952.0);
        assert_eq!(card_error_extra(0.0), 0.0);
        assert_eq!(card_error_extra(f64::NAN), 0.0);
        let extra = card_error_extra(15.0);
        assert_eq!(extra, 6.0 + 12.0 + 15.0);
        assert_eq!(CARD_ACTIONS_TOP + 32.0 + 12.0, CARD_HEIGHT);
        let mut extras = RowExtras::default();
        assert!(extras.is_empty());
        extras.grow(1, extra);
        extras.grow(1, 10.0);
        extras.grow(3, -4.0);
        assert_eq!(extras.of(1), extra, "a row keeps its tallest card");
        assert_eq!(extras.of(0), 0.0);
        assert_eq!(extras.before(1), 0.0);
        assert_eq!(extras.before(2), extra);
        assert_eq!(wide.row_card_height(1, &extras), CARD_HEIGHT + extra);
        assert_eq!(wide.row_card_height(2, &extras), CARD_HEIGHT);
        assert_eq!(wide.origin_with(4, &extras), wide.origin(4));
        assert_eq!(
            wide.origin_with(7, &extras),
            (wide.card_width + GRID_GAP, 2.0 * wide.row_stride() + extra)
        );
        assert_eq!(
            wide.content_height_with(7, &extras),
            wide.content_height(7) + extra
        );
        assert_eq!(wide.content_height_with(3, &extras), wide.content_height(3));
        let stride = wide.row_stride();
        assert_eq!(wide.visible_rows_with(100, 0.0, 10.0, &extras), 0..1);
        assert_eq!(
            wide.visible_rows_with(100, 2.0 * stride + 1.0, 10.0, &extras),
            1..2,
            "row 1 now reaches past its unextended bottom"
        );
        assert_eq!(
            wide.visible_rows_with(100, 2.0 * stride + extra + 1.0, 10.0, &extras),
            2..3
        );
        assert_eq!(wide.visible_rows_with(0, 0.0, 10.0, &extras), 0..0);
        assert_eq!(
            wide.visible_rows_with(100, 10.5 * stride, 10.0, &RowExtras::default()),
            wide.visible_rows(100, 10.5 * stride, 10.0)
        );
    }

    #[test]
    fn grid_auto_fills_like_css_minmax() {
        // 1000 px window minus 2 × 24 px padding: three columns.
        let wide = grid(952.0);
        assert_eq!(wide.columns, 3);
        assert!((wide.card_width - (952.0 - 32.0) / 3.0).abs() < 1e-9);
        assert_eq!(grid(252.0).columns, 1);
        assert_eq!(grid(519.0).columns, 1);
        assert_eq!(grid(520.0).columns, 2);
        assert_eq!(grid(0.0).columns, 1);
        assert_eq!(grid(f64::NAN).columns, 1);
        assert_eq!(wide.rows(0), 0);
        assert_eq!(wide.rows(7), 3);
        assert_eq!(wide.content_height(0), 0.0);
        assert_eq!(wide.content_height(4), CARD_HEIGHT * 2.0 + GRID_GAP);
        assert_eq!(
            wide.origin(4),
            (wide.card_width + GRID_GAP, wide.row_stride())
        );
        assert_eq!(wide.visible_rows(0, 0.0, 500.0), 0..0);
        assert_eq!(wide.visible_rows(100, 0.0, 300.0), 0..1);
        assert_eq!(wide.visible_rows(100, 0.0, 400.0), 0..2);
        assert_eq!(
            wide.visible_rows(100, wide.row_stride() * 10.5, 10.0),
            10..11
        );
        assert_eq!(wide.visible_rows(4, 10_000.0, 300.0), 2..2);
        assert_eq!(wide.index_at(1.0, 1.0, 5), Some(0));
        assert_eq!(wide.index_at(wide.card_width + 1.0, 1.0, 5), None, "gap");
        assert_eq!(wide.index_at(1.0, wide.row_stride() + 1.0, 5), Some(3));
        assert_eq!(
            wide.index_at(wide.card_width * 2.0 + 40.0, wide.row_stride() + 1.0, 5),
            None
        );
        assert_eq!(wide.index_at(-1.0, 0.0, 5), None);
        assert_eq!(wide.step(0, 5, 1, 0), 1);
        assert_eq!(wide.step(1, 5, 0, 1), 4);
        assert_eq!(wide.step(4, 5, 0, 1), 4);
        assert_eq!(wide.step(1, 5, -2, 0), 0);
        assert_eq!(wide.step(0, 0, 1, 0), 0);
    }

    #[test]
    fn missing_media_is_only_reported_for_absent_recordings() {
        let root = tempfile::tempdir().unwrap();
        let screenshot = entry(ArtifactKind::Screenshot);
        assert!(!media_missing(root.path(), &screenshot));
        let mut video = entry(ArtifactKind::Video);
        video.id = "0e3c8c52-2a55-4d8f-9d2a-3f1a3b6f7c10".into();
        assert!(media_missing(root.path(), &video));
        let directory = root.path().join(&video.id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("media.mp4"), b"mp4").unwrap();
        assert!(!media_missing(root.path(), &video));
        video.id = "not-a-uuid".into();
        assert!(media_missing(root.path(), &video));
    }
}
