//! Host-independent mini-preview placement and visibility policy.

use captures_settings::MiniPreviewPlacement;
use serde::{Deserialize, Serialize};

pub const THUMBNAIL_WIDTH: f64 = 340.0;
pub const THUMBNAIL_CARD_HEIGHT: f64 = 160.0;
pub const THUMBNAIL_GAP: f64 = 24.0;
pub const THUMBNAIL_PADDING: f64 = 28.0;
pub const THUMBNAIL_CONTROL_GUTTER: f64 = 52.0;

/// Extra logical pixels to keep the stack clear of system chrome.
/// Applied on every platform so previews never sit flush against a dock/taskbar.
pub const THUMBNAIL_SYSTEM_CHROME_GAP: f64 = 12.0;

/// When the work area reaches the monitor bottom (auto-hide taskbar/dock/panel),
/// reserve this many logical pixels so revealing chrome cannot cover cards.
pub const THUMBNAIL_AUTO_HIDE_RESERVE: f64 = 48.0;

/// Session-only preview membership, oldest first. Removing a preview never
/// removes a history entry or file. Hosts retain media resources keyed by ID.
#[derive(Default)]
pub struct PreviewStack {
    ids: Vec<String>,
    collapsed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewCardLayout {
    /// Logical top-left position in the unscrolled stack content.
    pub y: f64,
    /// Newest is depth zero and must paint above older cards when collapsed.
    pub depth: usize,
    pub interactive: bool,
}

impl PreviewStack {
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// Duplicate delivery neither reorders an existing card nor expands a pile.
    pub fn insert(&mut self, id: String) -> bool {
        if id.is_empty() || self.ids.contains(&id) {
            return false;
        }
        self.ids.push(id);
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.ids.len();
        self.ids.retain(|existing| existing != id);
        if self.ids.is_empty() {
            self.collapsed = false;
        }
        self.ids.len() != before
    }

    /// Clear the caller's snapshot only: a capture arriving later must survive.
    pub fn remove_all(&mut self, ids: &[String]) -> usize {
        let before = self.ids.len();
        self.ids.retain(|existing| !ids.contains(existing));
        if self.ids.is_empty() {
            self.collapsed = false;
        }
        before - self.ids.len()
    }

    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed && !self.ids.is_empty();
    }

    /// Unclamped document height, including the shared control gutter. Native
    /// window height may be smaller; scroll the document rather than its toolbar.
    pub fn content_height(&self) -> f64 {
        if self.ids.is_empty() {
            0.0
        } else if self.collapsed {
            collapsed_frame_height(self.ids.len())
        } else {
            stack_height(self.ids.len())
        }
    }

    /// Index is chronological, never visual. Top-anchored expanded piles put
    /// newest first; collapsed piles draw oldest first and newest on top.
    /// Hosts clip/scroll expanded content rather than capping membership.
    pub fn card_layout(&self, index: usize, top_anchor: bool) -> Option<PreviewCardLayout> {
        let depth = self.ids.len().checked_sub(index.checked_add(1)?)?;
        let y = if self.collapsed {
            let direction = if top_anchor { 1.0 } else { -1.0 };
            collapsed_padding(self.ids.len()) + direction * collapsed_peek(depth + 1, false)
        } else {
            let (padding, slot) = if top_anchor {
                (THUMBNAIL_CONTROL_GUTTER, depth)
            } else {
                (THUMBNAIL_PADDING, index)
            };
            padding + slot as f64 * (THUMBNAIL_CARD_HEIGHT + THUMBNAIL_GAP)
        };
        Some(PreviewCardLayout {
            y,
            depth,
            interactive: !self.collapsed || depth == 0,
        })
    }
}

/// Which edge of the visible pile stays put when the stack opens or closes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailStackAnchor {
    #[default]
    Bottom,
    Top,
}

impl ThumbnailStackAnchor {
    pub const fn is_top(self) -> bool {
        matches!(self, Self::Top)
    }
}

impl From<MiniPreviewPlacement> for ThumbnailStackAnchor {
    fn from(placement: MiniPreviewPlacement) -> Self {
        if placement.is_top() {
            Self::Top
        } else {
            Self::Bottom
        }
    }
}

/// Session-only: last user-dragged position of the mini-preview pile.
///
/// `edge` is the anchored edge of the visible pile in logical pixels: the
/// pile bottom when `anchor` is bottom, or the pile top when it is top.
#[derive(Clone, Copy, Debug)]
pub struct ThumbnailStackOrigin {
    pub x: f64,
    pub edge: f64,
    pub anchor: ThumbnailStackAnchor,
}

#[derive(Default)]
pub struct ThumbnailVisibility {
    next_capture_generation: u64,
    suppressed_capture_generation: Option<u64>,
    pending_artifact_id: Option<String>,
    capture_ui_suppressed: bool,
    /// Session-only: the user parked the stack behind the restore chip.
    user_collapsed: bool,
    stack_origin: Option<ThumbnailStackOrigin>,
}

impl ThumbnailVisibility {
    pub fn begin_capture(&mut self) -> Option<u64> {
        if self.suppressed_capture_generation.is_some() && self.pending_artifact_id.is_none() {
            return None;
        }
        self.next_capture_generation = self.next_capture_generation.wrapping_add(1);
        self.suppressed_capture_generation = Some(self.next_capture_generation);
        self.pending_artifact_id = None;
        Some(self.next_capture_generation)
    }

    pub fn wait_for_artifact(&mut self, capture_generation: u64, artifact_id: String) -> bool {
        if self.suppressed_capture_generation != Some(capture_generation) {
            return false;
        }
        self.pending_artifact_id = Some(artifact_id);
        true
    }

    pub fn mark_artifact_ready(&mut self, artifact_id: &str) -> bool {
        if self.pending_artifact_id.as_deref() != Some(artifact_id) {
            return false;
        }
        self.pending_artifact_id = None;
        self.suppressed_capture_generation = None;
        // Un-hide the stack so the new shot lands on the pile. Leave parking
        // alone: auto-expanding resized the window to the expanded bar while
        // the webview stayed collapsed, which pinned drag to that bar's top.
        true
    }

    pub fn restore_capture(&mut self, capture_generation: u64) -> bool {
        if self.suppressed_capture_generation != Some(capture_generation) {
            return false;
        }
        self.suppressed_capture_generation = None;
        self.pending_artifact_id = None;
        true
    }

    pub fn stop_waiting_for_artifact(&mut self) -> bool {
        if self.pending_artifact_id.is_none() {
            return false;
        }
        self.suppressed_capture_generation = None;
        self.pending_artifact_id = None;
        true
    }

    pub fn suppress_for_capture_ui(&mut self) {
        self.capture_ui_suppressed = true;
    }

    pub fn restore_capture_ui(&mut self) {
        self.capture_ui_suppressed = false;
    }

    pub fn collapse(&mut self) {
        self.user_collapsed = true;
    }

    pub fn expand(&mut self) {
        self.user_collapsed = false;
    }

    pub fn reset_session_placement(&mut self) {
        self.user_collapsed = false;
        self.stack_origin = None;
    }

    pub fn set_stack_origin(&mut self, origin: ThumbnailStackOrigin) {
        self.stack_origin = Some(origin);
    }

    pub fn stack_origin(&self) -> Option<ThumbnailStackOrigin> {
        self.stack_origin
    }

    pub fn clear_stack_origin(&mut self) {
        self.stack_origin = None;
    }

    pub fn is_collapsed(&self) -> bool {
        self.user_collapsed
    }

    pub fn is_suppressed(&self) -> bool {
        self.suppressed_capture_generation.is_some() || self.capture_ui_suppressed
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbnailCollapsedWindowPosition {
    pub x: f64,
    pub frame_y: f64,
    pub front_y: f64,
    pub content_y: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ThumbnailMonitorBounds {
    pub work_x: i32,
    pub work_y: i32,
    pub work_width: u32,
    pub work_height: u32,
    pub full_x: i32,
    pub full_y: i32,
    pub full_width: u32,
    pub full_height: u32,
    pub scale_factor: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ThumbnailWorkArea {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub top_gap: f64,
    pub bottom_gap: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbnailWindowGeometry {
    pub x: f64,
    pub y: f64,
    pub height: f64,
    pub anchor: ThumbnailStackAnchor,
}

pub fn stack_should_be_visible(
    count: usize,
    suppressed: bool,
    show_mini_previews: bool,
    include_mini_previews_in_captures: bool,
) -> bool {
    // Capture flows suppress the stack so it does not appear in screenshots or
    // recordings. Opting in keeps it visible for self-capture / feedback.
    count > 0 && show_mini_previews && (!suppressed || include_mini_previews_in_captures)
}

pub fn visible_window_height(desired: f64, current: Option<f64>, preserve_current: bool) -> f64 {
    match (preserve_current, current) {
        (true, Some(current)) => desired.max(current),
        _ => desired,
    }
}

/// Place the visible collapsed pile within a retained, potentially much taller
/// WebView frame. The native frame stays inside the work area while the card
/// moves through its empty space, avoiding AppKit's off-screen frame clamp.
pub fn collapsed_window_position(
    x: f64,
    front_y: f64,
    frame_height: f64,
    padding: f64,
    work: ThumbnailWorkArea,
) -> ThumbnailCollapsedWindowPosition {
    let min_x = work.left;
    let max_x = (work.left + work.width - THUMBNAIL_WIDTH).max(min_x);
    let work_bottom = work.top + work.height - work.bottom_gap;
    let min_front_y = work.top + THUMBNAIL_CONTROL_GUTTER;
    let max_front_y =
        (work_bottom - THUMBNAIL_CARD_HEIGHT - THUMBNAIL_CONTROL_GUTTER).max(min_front_y);
    let front_y = front_y.clamp(min_front_y, max_front_y);
    let frame_height = frame_height.max(THUMBNAIL_CARD_HEIGHT + 2.0 * padding);
    let min_frame_y = work.top;
    let max_frame_y = (work_bottom - frame_height).max(min_frame_y);
    let bottom_content_y = (frame_height - padding - THUMBNAIL_CARD_HEIGHT).max(padding);
    let frame_y = (front_y - bottom_content_y).clamp(min_frame_y, max_frame_y);

    ThumbnailCollapsedWindowPosition {
        x: x.clamp(min_x, max_x),
        frame_y,
        front_y,
        content_y: front_y - frame_y,
    }
}

pub fn stack_pose_depth(depth: f64) -> f64 {
    // Keep in sync with thumbnailStackPoseDepth in thumbnailLayout.ts.
    const RECEDE: f64 = 0.55;
    const EASE_K: f64 = 24.0;
    if depth <= 0.0 {
        0.0
    } else {
        depth * (EASE_K + RECEDE * depth) / (depth + EASE_K)
    }
}

pub fn collapsed_peek(count: usize, hovered: bool) -> f64 {
    let extra = count.saturating_sub(1) as f64;
    let pose = stack_pose_depth(extra);
    // Keep in sync with THUMBNAIL_STACK_IDLE_PEEK_PX / HOVER_PEEK_PX.
    pose * if hovered { 16.0 } else { 13.0 }
}

pub fn collapsed_padding(count: usize) -> f64 {
    (collapsed_peek(count.max(1), true) + THUMBNAIL_PADDING).max(THUMBNAIL_CONTROL_GUTTER)
}

pub fn collapsed_frame_height(count: usize) -> f64 {
    THUMBNAIL_CARD_HEIGHT + 2.0 * collapsed_padding(count)
}

fn collapsed_virtual_y(front_y: f64, frame_height: f64, anchor: ThumbnailStackAnchor) -> f64 {
    if anchor.is_top() {
        front_y - THUMBNAIL_CONTROL_GUTTER
    } else {
        front_y + THUMBNAIL_CARD_HEIGHT + THUMBNAIL_CONTROL_GUTTER - frame_height
    }
}

fn collapsed_front_y(virtual_y: f64, frame_height: f64, anchor: ThumbnailStackAnchor) -> f64 {
    if anchor.is_top() {
        virtual_y + THUMBNAIL_CONTROL_GUTTER
    } else {
        virtual_y + frame_height - THUMBNAIL_CARD_HEIGHT - THUMBNAIL_CONTROL_GUTTER
    }
}

pub fn stack_height(count: usize) -> f64 {
    let cards = count.max(1) as f64;
    THUMBNAIL_PADDING
        + THUMBNAIL_CONTROL_GUTTER
        + cards * THUMBNAIL_CARD_HEIGHT
        + (cards - 1.0) * THUMBNAIL_GAP
}

pub fn work_area(bounds: ThumbnailMonitorBounds) -> ThumbnailWorkArea {
    let scale = bounds.scale_factor.max(1.0);
    let left = f64::from(bounds.work_x) / scale;
    let top = f64::from(bounds.work_y) / scale;
    let width = f64::from(bounds.work_width) / scale;
    let mut height = f64::from(bounds.work_height) / scale;

    // Auto-hide taskbars/docks leave the work area flush with the monitor's
    // bottom edge. Compare bottom edges instead of whole rectangles: macOS
    // still excludes its top menu bar, so its work area never equals the full
    // monitor even when an auto-hidden bottom Dock is unreserved.
    let work_bottom = i64::from(bounds.work_y) + i64::from(bounds.work_height);
    let full_bottom = i64::from(bounds.full_y) + i64::from(bounds.full_height);
    let work_spans_full_width =
        bounds.work_x == bounds.full_x && bounds.work_width == bounds.full_width;
    if work_bottom == full_bottom && work_spans_full_width {
        let bottom_reserve = THUMBNAIL_AUTO_HIDE_RESERVE.min((height * 0.12).max(0.0));
        height = (height - bottom_reserve).max(1.0);
    }

    ThumbnailWorkArea {
        left,
        top,
        width,
        height,
        top_gap: THUMBNAIL_SYSTEM_CHROME_GAP,
        bottom_gap: THUMBNAIL_SYSTEM_CHROME_GAP,
    }
}

/// Keep the visible pile in the work area.
///
/// Collapsed macOS/Linux windows stay at their expanded height so WebKit does
/// not blank cards. Bottom piles sit at the bottom of that frame (empty chrome
/// may leave the work area above so the stack can reach the top). Top piles
/// sit at the top so peek-down has room; empty chrome may leave below so the
/// stack can still reach the bottom.
pub fn clamp_aligned_frame(
    x: f64,
    y: f64,
    frame_height: f64,
    content_height: f64,
    work: ThumbnailWorkArea,
    anchor: ThumbnailStackAnchor,
) -> (f64, f64) {
    let content_height = content_height.min(frame_height).max(0.0);
    let slack = (frame_height - content_height).max(0.0);
    let min_x = work.left;
    let max_x = (work.left + work.width - THUMBNAIL_WIDTH).max(min_x);
    let (min_y, max_y) = if anchor.is_top() {
        let min_y = work.top;
        let max_y = (work.top + work.height - work.bottom_gap - content_height).max(min_y);
        (min_y, max_y)
    } else {
        let min_y = work.top - slack;
        let max_y = (work.top + work.height - work.bottom_gap - frame_height).max(min_y);
        (min_y, max_y)
    };
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

pub fn window_top(
    desired_y: f64,
    frame_height: f64,
    content_height: f64,
    anchor: ThumbnailStackAnchor,
) -> f64 {
    if anchor.is_top() {
        desired_y
    } else {
        desired_y - (frame_height - content_height)
    }
}

pub fn thumbnail_geometry(
    bounds: ThumbnailMonitorBounds,
    count: usize,
    collapsed: bool,
    origin: Option<ThumbnailStackOrigin>,
    placement: MiniPreviewPlacement,
) -> ThumbnailWindowGeometry {
    let work = work_area(bounds);
    let available_height = (work.height - work.bottom_gap - THUMBNAIL_PADDING).max(1.0);
    let stack_height = if collapsed {
        collapsed_frame_height(count)
    } else {
        stack_height(count).min(available_height)
    };
    let default_x = if placement.is_right() {
        (work.left + work.width - THUMBNAIL_WIDTH).max(work.left)
    } else {
        work.left
            .min(work.left + work.width - THUMBNAIL_WIDTH)
            .max(work.left)
    };
    let default_anchor = ThumbnailStackAnchor::from(placement);
    let (x, desired_y, anchor) = match origin {
        Some(origin) => {
            let desired_y = if origin.anchor.is_top() {
                origin.edge
            } else {
                origin.edge - stack_height
            };
            (origin.x, desired_y, origin.anchor)
        }
        None => {
            let desired_y = if default_anchor.is_top() {
                work.top + work.top_gap
            } else {
                work.top + work.height - work.bottom_gap - stack_height
            };
            (default_x, desired_y, default_anchor)
        }
    };
    if collapsed {
        let front_y = match origin {
            Some(origin) if origin.anchor.is_top() => origin.edge + THUMBNAIL_CONTROL_GUTTER,
            Some(origin) => origin.edge - THUMBNAIL_CARD_HEIGHT - THUMBNAIL_CONTROL_GUTTER,
            None if default_anchor.is_top() => work.top + work.top_gap + THUMBNAIL_CONTROL_GUTTER,
            None => {
                work.top + work.height
                    - work.bottom_gap
                    - THUMBNAIL_CARD_HEIGHT
                    - THUMBNAIL_CONTROL_GUTTER
            }
        };
        let virtual_y = collapsed_virtual_y(front_y, stack_height, anchor);
        let (x, virtual_y) = clamp_aligned_frame(
            x,
            virtual_y,
            stack_height,
            THUMBNAIL_CARD_HEIGHT + 2.0 * THUMBNAIL_CONTROL_GUTTER,
            work,
            anchor,
        );
        let front_y = collapsed_front_y(virtual_y, stack_height, anchor);
        let padding = collapsed_padding(count);
        return ThumbnailWindowGeometry {
            x,
            y: front_y - padding,
            height: stack_height,
            anchor: ThumbnailStackAnchor::Bottom,
        };
    }
    let (x, y) = clamp_aligned_frame(x, desired_y, stack_height, stack_height, work, anchor);
    ThumbnailWindowGeometry {
        x,
        y,
        height: stack_height,
        anchor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_snapshot_clear_preserves_new_capture_and_empty_resets_parking() {
        let mut stack = PreviewStack::default();
        assert!(!stack.insert(String::new()));
        for id in ["older", "middle", "latest"] {
            assert!(stack.insert(id.into()));
        }
        assert!(!stack.insert("older".into()));
        stack.set_collapsed(true);
        let snapshot = stack.ids().to_vec();
        assert!(stack.insert("incoming".into()));
        assert!(stack.is_collapsed());
        assert!(stack.remove("middle"));
        assert!(!stack.remove("middle"));
        assert_eq!(stack.remove_all(&snapshot), 2);
        assert_eq!(stack.ids(), &["incoming"]);
        assert!(stack.is_collapsed());
        assert!(stack.remove("incoming"));
        assert!(!stack.is_collapsed());
        stack.set_collapsed(true);
        stack.insert("fresh".into());
        assert!(!stack.is_collapsed());
    }

    #[test]
    fn stack_layout_reverses_only_top_expansion_and_front_is_the_only_pile_target() {
        let mut stack = PreviewStack::default();
        assert_eq!(stack.content_height(), 0.);
        for id in ["A", "B", "C"] {
            stack.insert(id.into());
        }
        assert_eq!(stack.content_height(), 608.);
        assert_eq!(stack.card_layout(0, false).unwrap().y, 28.);
        assert_eq!(stack.card_layout(2, false).unwrap().y, 396.);
        assert_eq!(stack.card_layout(0, true).unwrap().y, 420.);
        assert_eq!(stack.card_layout(2, true).unwrap().y, 52.);
        assert!(stack.card_layout(0, true).unwrap().interactive);
        assert!(stack.card_layout(3, true).is_none());
        assert!(stack.card_layout(usize::MAX, false).is_none());
        stack.set_collapsed(true);
        // Independent shipping peek: 13 * 2 * (24 + .55 * 2) / (2 + 24).
        let peek = 25.1;
        // Frame reserves hover peeks, even though this pose uses idle peeks.
        let padding = 28. + 16. * 2. * 25.1 / 26.;
        assert!((stack.content_height() - (160. + 2. * padding)).abs() < 1e-9);
        assert!((stack.card_layout(0, false).unwrap().y - (padding - peek)).abs() < 1e-9);
        assert!((stack.card_layout(0, true).unwrap().y - (padding + peek)).abs() < 1e-9);
        assert!(!stack.card_layout(0, false).unwrap().interactive);
        assert_eq!(
            stack.card_layout(2, false).unwrap(),
            PreviewCardLayout {
                y: padding,
                depth: 0,
                interactive: true,
            }
        );
        for index in 3..40 {
            stack.insert(format!("capture-{index}"));
        }
        assert_eq!(stack.ids().len(), 40);
        assert_eq!(stack.card_layout(39, true).unwrap().depth, 0);
    }

    fn bounds(
        work: (i32, i32, u32, u32),
        full: (i32, i32, u32, u32),
        scale_factor: f64,
    ) -> ThumbnailMonitorBounds {
        ThumbnailMonitorBounds {
            work_x: work.0,
            work_y: work.1,
            work_width: work.2,
            work_height: work.3,
            full_x: full.0,
            full_y: full.1,
            full_width: full.2,
            full_height: full.3,
            scale_factor,
        }
    }

    fn stack_geometry(
        bounds: ThumbnailMonitorBounds,
        count: usize,
        collapsed: bool,
        origin: Option<ThumbnailStackOrigin>,
    ) -> ThumbnailWindowGeometry {
        thumbnail_geometry(
            bounds,
            count,
            collapsed,
            origin,
            MiniPreviewPlacement::BottomLeft,
        )
    }

    fn stack_xyh(
        bounds: ThumbnailMonitorBounds,
        count: usize,
        collapsed: bool,
        origin: Option<ThumbnailStackOrigin>,
    ) -> (f64, f64, f64) {
        let geometry = stack_geometry(bounds, count, collapsed, origin);
        (geometry.x, geometry.y, geometry.height)
    }

    #[test]
    fn blocks_overlapping_capture_preparation() {
        let mut visibility = ThumbnailVisibility::default();

        let first = visibility
            .begin_capture()
            .expect("first capture should start");
        assert!(visibility.begin_capture().is_none());
        assert!(visibility.is_suppressed());

        assert!(visibility.restore_capture(first));
        assert!(!visibility.is_suppressed());
        assert!(visibility.begin_capture().is_some());
    }

    #[test]
    fn ignores_stale_restore_and_image_ready_events_after_the_next_capture_starts() {
        let mut visibility = ThumbnailVisibility::default();

        let first = visibility
            .begin_capture()
            .expect("first capture should start");
        assert!(visibility.wait_for_artifact(first, "first".to_owned()));
        let second = visibility
            .begin_capture()
            .expect("second capture should start");

        assert!(!visibility.restore_capture(first));
        assert!(visibility.is_suppressed());
        assert!(visibility.wait_for_artifact(second, "second".to_owned()));

        assert!(!visibility.mark_artifact_ready("first"));
        assert!(visibility.is_suppressed());
        assert!(visibility.mark_artifact_ready("second"));
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn disabling_previews_releases_only_an_artifact_wait() {
        let mut visibility = ThumbnailVisibility::default();

        let capture = visibility.begin_capture().expect("capture should start");
        assert!(!visibility.stop_waiting_for_artifact());
        assert!(visibility.is_suppressed());

        assert!(visibility.wait_for_artifact(capture, "artifact".to_owned()));
        assert!(visibility.stop_waiting_for_artifact());
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn capture_ui_suppression_stays_active_across_a_screenshot_preview() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.suppress_for_capture_ui();

        let capture = visibility.begin_capture().expect("capture should start");
        assert!(visibility.wait_for_artifact(capture, "artifact".to_owned()));
        assert!(visibility.mark_artifact_ready("artifact"));
        assert!(visibility.is_suppressed());

        visibility.restore_capture_ui();
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn stack_origin_survives_expand_and_clears_with_session_placement() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.set_stack_origin(ThumbnailStackOrigin {
            x: 120.0,
            edge: 640.0,
            anchor: ThumbnailStackAnchor::Bottom,
        });
        visibility.collapse();
        visibility.expand();
        assert!(!visibility.is_collapsed());
        assert_eq!(visibility.stack_origin().unwrap().x, 120.0);
        assert_eq!(visibility.stack_origin().unwrap().edge, 640.0);
        assert_eq!(
            visibility.stack_origin().unwrap().anchor,
            ThumbnailStackAnchor::Bottom
        );

        visibility.reset_session_placement();
        assert!(visibility.stack_origin().is_none());
        assert!(!visibility.is_collapsed());
    }

    #[test]
    fn a_new_preview_keeps_the_stack_collapsed() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.collapse();
        assert!(visibility.is_collapsed());

        visibility.expand();
        assert!(!visibility.is_collapsed());

        visibility.collapse();
        let capture = visibility.begin_capture().expect("capture should start");
        assert!(visibility.is_collapsed());
        assert!(visibility.wait_for_artifact(capture, "artifact".to_owned()));
        assert!(visibility.mark_artifact_ready("artifact"));
        assert!(visibility.is_collapsed());
    }

    #[test]
    fn cancelling_a_capture_keeps_the_stack_collapsed() {
        let mut visibility = ThumbnailVisibility::default();
        visibility.collapse();
        let capture = visibility.begin_capture().expect("capture should start");
        assert!(visibility.restore_capture(capture));
        assert!(visibility.is_collapsed());
        assert!(!visibility.is_suppressed());
    }

    #[test]
    fn stacks_thumbnails_upward_in_logical_pixels_on_retina_displays() {
        assert_eq!(
            stack_xyh(
                bounds((0, 0, 3_992, 2_048), (0, 0, 3_992, 2_160), 2.0),
                1,
                false,
                None,
            ),
            (0.0, 772.0, 240.0)
        );
        assert_eq!(
            stack_xyh(
                bounds((-3_840, 0, 3_840, 2_048), (-3_840, 0, 3_840, 2_160), 2.0),
                2,
                false,
                None,
            ),
            (-1_920.0, 588.0, 424.0)
        );
        assert_eq!(
            stack_xyh(
                bounds((0, 0, 3_992, 2_048), (0, 0, 3_992, 2_160), 2.0),
                1,
                true,
                None,
            ),
            (0.0, 748.0, 264.0)
        );
    }

    #[test]
    fn keeps_the_thumbnail_stack_inside_the_monitor_work_area() {
        let (_, top, height) = stack_xyh(
            bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0),
            1,
            false,
            None,
        );

        assert_eq!(top + height, 1_040.0 - THUMBNAIL_SYSTEM_CHROME_GAP);
        assert!(top + height < 1_040.0);
    }

    #[test]
    fn reserves_space_when_work_area_matches_full_monitor_auto_hide() {
        let (_, top, height) = stack_xyh(
            bounds((0, 0, 1_920, 1_080), (0, 0, 1_920, 1_080), 1.0),
            1,
            false,
            None,
        );

        let window_bottom = top + height;
        assert!(window_bottom <= 1_080.0 - THUMBNAIL_AUTO_HIDE_RESERVE);
        assert_eq!(
            window_bottom,
            1_080.0 - THUMBNAIL_AUTO_HIDE_RESERVE - THUMBNAIL_SYSTEM_CHROME_GAP
        );
    }

    #[test]
    fn reserves_bottom_space_when_top_system_chrome_remains_visible() {
        let (_, top, height) = stack_xyh(
            bounds((0, 48, 3_992, 2_112), (0, 0, 3_992, 2_160), 2.0),
            1,
            false,
            None,
        );

        assert_eq!(
            top + height,
            1_080.0 - THUMBNAIL_AUTO_HIDE_RESERVE - THUMBNAIL_SYSTEM_CHROME_GAP
        );
    }

    #[test]
    fn places_a_dragged_stack_at_the_stored_origin() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let (x, y, height) = stack_xyh(
            work,
            1,
            true,
            Some(ThumbnailStackOrigin {
                x: 420.0,
                edge: 520.0,
                anchor: ThumbnailStackAnchor::Bottom,
            }),
        );
        assert_eq!(height, 264.0);
        assert_eq!((x, y), (420.0, 256.0));
        assert_eq!(y + 52.0 + 160.0 + 52.0, 520.0);
    }

    #[test]
    fn keeps_a_dragged_collapsed_pile_on_its_origin_after_another_capture() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let origin = ThumbnailStackOrigin {
            x: 420.0,
            edge: 640.0,
            anchor: ThumbnailStackAnchor::Bottom,
        };
        let three = stack_geometry(work, 3, true, Some(origin));
        let four = stack_geometry(work, 4, true, Some(origin));
        assert_eq!(three.x, 420.0);
        assert_eq!(four.x, 420.0);
        let three_padding = collapsed_padding(3);
        let four_padding = collapsed_padding(4);
        assert_eq!(three.y + three_padding + 160.0 + 52.0, 640.0);
        assert_eq!(four.y + four_padding + 160.0 + 52.0, 640.0);

        let expanded = stack_geometry(work, 4, false, Some(origin));
        assert_eq!(expanded.y, 0.0);
        assert!(expanded.height > four.height);
        let retained_y = window_top(
            four.y,
            expanded.height,
            four.height,
            ThumbnailStackAnchor::Bottom,
        );
        assert!((retained_y + expanded.height - four_padding - 160.0 - 428.0).abs() < 1e-9);
    }

    #[test]
    fn restores_a_top_aligned_pile_without_consuming_preserved_slack() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let geometry = thumbnail_geometry(
            work,
            1,
            true,
            Some(ThumbnailStackOrigin {
                x: 420.0,
                edge: 0.0,
                anchor: ThumbnailStackAnchor::Top,
            }),
            MiniPreviewPlacement::BottomLeft,
        );
        assert_eq!(geometry.height, 264.0);
        assert_eq!((geometry.x, geometry.y), (420.0, 0.0));
        assert_eq!(geometry.anchor, ThumbnailStackAnchor::Bottom);
        let retained_y = window_top(geometry.y, 792.0, geometry.height, geometry.anchor);
        assert_eq!(retained_y, -528.0);
        assert_eq!(retained_y + 792.0 - 52.0 - 160.0, 52.0);
    }

    #[test]
    fn collapsed_physical_frame_is_independent_of_expansion_anchor() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let front_y = 420.0;
        let top = stack_geometry(
            work,
            6,
            true,
            Some(ThumbnailStackOrigin {
                x: 300.0,
                edge: front_y - 52.0,
                anchor: ThumbnailStackAnchor::Top,
            }),
        );
        let bottom = stack_geometry(
            work,
            6,
            true,
            Some(ThumbnailStackOrigin {
                x: 300.0,
                edge: front_y + 160.0 + 52.0,
                anchor: ThumbnailStackAnchor::Bottom,
            }),
        );
        assert_eq!(top, bottom);
        assert_eq!(top.anchor, ThumbnailStackAnchor::Bottom);
        assert_eq!(top.y + collapsed_padding(6), front_y);
    }

    #[test]
    fn collapsed_frame_preserves_tall_window_and_origin_round_trip() {
        let count = 100;
        let padding = collapsed_padding(count);
        let desired_height = collapsed_frame_height(count);
        assert_eq!(desired_height, 160.0 + 2.0 * padding);
        assert!(desired_height > 264.0);

        let retained_height = 1_400.0;
        let actual_y = -300.0;
        let front_y = actual_y + retained_height - padding - 160.0;
        for anchor in [ThumbnailStackAnchor::Top, ThumbnailStackAnchor::Bottom] {
            let virtual_y = collapsed_virtual_y(front_y, retained_height, anchor);
            assert_eq!(
                collapsed_front_y(virtual_y, retained_height, anchor),
                front_y
            );
            let edge = if anchor.is_top() {
                front_y - 52.0
            } else {
                front_y + 160.0 + 52.0
            };
            let recovered_front = if anchor.is_top() {
                edge + 52.0
            } else {
                edge - 160.0 - 52.0
            };
            assert_eq!(recovered_front, front_y);
        }
    }

    #[test]
    fn clamps_a_dragged_stack_to_the_work_area() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        assert_eq!(
            stack_xyh(
                work,
                1,
                true,
                Some(ThumbnailStackOrigin {
                    x: 8_000.0,
                    edge: 8_000.0,
                    anchor: ThumbnailStackAnchor::Bottom,
                }),
            ),
            (1_580.0, 764.0, 264.0)
        );
        assert_eq!(
            clamp_aligned_frame(
                -40.0,
                -20.0,
                240.0,
                240.0,
                work_area(work),
                ThumbnailStackAnchor::Bottom,
            ),
            (0.0, 0.0)
        );
    }

    #[test]
    fn lets_a_collapsed_pile_reach_the_top_when_the_window_stays_tall() {
        let work = work_area(bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0));
        assert_eq!(
            clamp_aligned_frame(
                -40.0,
                -800.0,
                792.0,
                240.0,
                work,
                ThumbnailStackAnchor::Bottom
            ),
            (0.0, -552.0)
        );
        assert_eq!(
            clamp_aligned_frame(
                420.0,
                400.0,
                792.0,
                240.0,
                work,
                ThumbnailStackAnchor::Bottom
            ),
            (420.0, 236.0)
        );
        assert_eq!(-552.0 + 792.0 - 240.0, 0.0);
        assert_eq!(
            clamp_aligned_frame(-40.0, -800.0, 792.0, 240.0, work, ThumbnailStackAnchor::Top),
            (0.0, 0.0)
        );
        assert_eq!(
            clamp_aligned_frame(
                420.0,
                2_000.0,
                792.0,
                240.0,
                work,
                ThumbnailStackAnchor::Top
            ),
            (420.0, 788.0)
        );
        assert_eq!(788.0 + 240.0, 1_040.0 - THUMBNAIL_SYSTEM_CHROME_GAP);
    }

    #[test]
    fn moves_a_collapsed_pile_through_a_retained_on_screen_frame() {
        let work = work_area(bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0));
        let frame_height = 792.0;
        let padding = 52.0;

        let top = collapsed_window_position(-40.0, -800.0, frame_height, padding, work);
        assert_eq!(top.x, 0.0);
        assert_eq!(top.frame_y, 0.0);
        assert_eq!(top.front_y, 52.0);
        assert_eq!(top.content_y, 52.0);

        let middle = collapsed_window_position(420.0, 400.0, frame_height, padding, work);
        assert_eq!(middle.frame_y, 0.0);
        assert_eq!(middle.front_y, 400.0);
        assert_eq!(middle.content_y, 400.0);

        let bottom = collapsed_window_position(8_000.0, 8_000.0, frame_height, padding, work);
        assert_eq!(bottom.x, 1_580.0);
        assert_eq!(bottom.frame_y, 236.0);
        assert_eq!(bottom.front_y, 816.0);
        assert_eq!(bottom.content_y, 580.0);
        assert_eq!(bottom.frame_y + frame_height, 1_028.0);
    }

    #[test]
    fn collapsed_content_position_is_continuous_when_the_frame_reaches_an_edge() {
        let work = work_area(bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0));
        let before = collapsed_window_position(100.0, 579.0, 792.0, 52.0, work);
        let edge = collapsed_window_position(100.0, 580.0, 792.0, 52.0, work);
        let after = collapsed_window_position(100.0, 581.0, 792.0, 52.0, work);

        assert_eq!(before.front_y + 1.0, edge.front_y);
        assert_eq!(edge.front_y + 1.0, after.front_y);
        assert_eq!(before.frame_y, 0.0);
        assert_eq!(edge.frame_y, 0.0);
        assert_eq!(after.frame_y, 1.0);
        assert_eq!(before.content_y + 1.0, edge.content_y);
        assert_eq!(edge.content_y, after.content_y);
    }

    #[test]
    fn places_the_stack_in_the_chosen_screen_corner() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let top_right = thumbnail_geometry(work, 1, false, None, MiniPreviewPlacement::TopRight);
        assert_eq!(top_right.x, 1_580.0);
        assert_eq!(top_right.y, THUMBNAIL_SYSTEM_CHROME_GAP);
        assert_eq!(top_right.height, 240.0);
        assert_eq!(top_right.anchor, ThumbnailStackAnchor::Top);

        let bottom_right =
            thumbnail_geometry(work, 1, false, None, MiniPreviewPlacement::BottomRight);
        assert_eq!(bottom_right.x, 1_580.0);
        assert_eq!(
            bottom_right.y + bottom_right.height,
            1_040.0 - THUMBNAIL_SYSTEM_CHROME_GAP
        );
        assert_eq!(bottom_right.anchor, ThumbnailStackAnchor::Bottom);
    }

    #[test]
    fn expands_a_top_anchored_pile_downward() {
        let work = bounds((0, 0, 1_920, 1_040), (0, 0, 1_920, 1_080), 1.0);
        let collapsed = thumbnail_geometry(
            work,
            3,
            true,
            Some(ThumbnailStackOrigin {
                x: 80.0,
                edge: 24.0,
                anchor: ThumbnailStackAnchor::Top,
            }),
            MiniPreviewPlacement::BottomLeft,
        );
        let expanded = thumbnail_geometry(
            work,
            3,
            false,
            Some(ThumbnailStackOrigin {
                x: 80.0,
                edge: 24.0,
                anchor: ThumbnailStackAnchor::Top,
            }),
            MiniPreviewPlacement::BottomLeft,
        );
        assert!((collapsed.y - (76.0 - collapsed_padding(3))).abs() < 1e-9);
        assert_eq!(collapsed.anchor, ThumbnailStackAnchor::Bottom);
        assert_eq!(expanded.y, 24.0);
        assert!(expanded.height > collapsed.height);
        assert_eq!(expanded.anchor, ThumbnailStackAnchor::Top);
        assert_eq!(
            window_top(24.0, 792.0, 240.0, ThumbnailStackAnchor::Top),
            24.0
        );
        assert_eq!(
            window_top(24.0, 792.0, 240.0, ThumbnailStackAnchor::Bottom),
            24.0 - (792.0 - 240.0)
        );
    }

    #[test]
    fn keeps_visible_thumbnail_window_from_shrinking_after_dismiss() {
        assert_eq!(visible_window_height(400.0, Some(584.0), true), 584.0);
        assert_eq!(visible_window_height(584.0, Some(400.0), true), 584.0);
        assert_eq!(visible_window_height(216.0, None, true), 216.0);
    }

    #[test]
    fn hides_mini_previews_when_the_preference_is_disabled() {
        assert!(stack_should_be_visible(1, false, true, false));
        assert!(!stack_should_be_visible(1, true, true, false));
        assert!(!stack_should_be_visible(1, false, false, false));
        assert!(!stack_should_be_visible(0, false, true, false));
    }

    #[test]
    fn keeps_mini_previews_visible_during_capture_when_included() {
        assert!(stack_should_be_visible(1, true, true, true));
        assert!(!stack_should_be_visible(1, true, false, true));
        assert!(!stack_should_be_visible(0, true, true, true));
    }

    #[test]
    fn keeps_the_minimized_stack_visible() {
        assert!(stack_should_be_visible(2, false, true, false));
    }

    #[test]
    fn collapsed_stack_window_fits_the_receding_pile() {
        assert_eq!(collapsed_frame_height(1), 264.0);
        assert_eq!(stack_height(1), 240.0);
        assert!(collapsed_frame_height(8) > collapsed_frame_height(4));
        assert!(collapsed_frame_height(8) < stack_height(8));
        let pose_3 = 3.0 * (24.0 + 0.55 * 3.0) / (3.0 + 24.0);
        let peek = pose_3 * 16.0;
        assert!((collapsed_frame_height(4) - (160.0 + 2.0 * (peek + 28.0))).abs() < 1e-9);
    }

    #[test]
    fn shrinks_non_macos_thumbnail_windows_to_avoid_invisible_click_blockers() {
        assert_eq!(visible_window_height(400.0, Some(584.0), false), 400.0);
    }
}
