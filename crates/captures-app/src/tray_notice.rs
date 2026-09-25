//! Placement for the shipping tray / menu-bar anchored notices, such as the
//! "Captures is ready to use" launch notice. Pure logical-point geometry in a
//! top-left desktop coordinate space: hosts convert their monitor, work area
//! and tray icon rects, then position a transparent window at the result.
//!
//! Ported from the Tauri host (`apps/desktop/src-tauri/src/lib.rs`) so every
//! native host shares one policy and its tests.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Card size of the launch pill, excluding the transparent shadow frame.
pub const STARTUP_NOTICE_WIDTH: f64 = 296.0;
pub const STARTUP_NOTICE_HEIGHT: f64 = 54.0;
/// Transparent padding around the rounded card so `--shadow-md` is not clipped.
/// Dark `--shadow-md` is `0 8px 20px`, so the blur plus Y offset needs 28px.
pub const TRAY_NOTICE_FRAME_PAD: f64 = 28.0;
/// Extra window height reserved for the tray-pointing caret.
pub const TRAY_NOTICE_CARET_SIZE: f64 = 8.0;
/// Width of the launch notice's triangle caret (`--tray-caret-span`).
pub const STARTUP_NOTICE_CARET_SPAN: f64 = 12.0;
/// The caret overlaps the card by this much so no seam shows between them
/// (`--tray-caret-overlap`).
pub const TRAY_NOTICE_CARET_OVERLAP: f64 = 1.0;
/// Keep the caret off the rounded ends of the notice. The launch pill is 36px
/// tall, so its end-caps are 18px; half the 12px caret span is 6px more.
pub const TRAY_NOTICE_CARET_INSET: f64 = 24.0;
/// Pull the transparent window over the tray so the caret tip sits on the icon.
pub const TRAY_NOTICE_TRAY_OVERLAP: f64 = 2.0;
pub const TRAY_NOTICE_SCREEN_MARGIN: f64 = 10.0;
/// Status items and tray icons live in a thin band along a display edge.
/// After an update restart, AppKit can report the status item at the Cocoa
/// origin (bottom-left) before it has been placed in the menu bar.
pub const STARTUP_NOTICE_TRAY_EDGE_BAND: f64 = 56.0;
/// macOS and Windows report a tray rect once the icon is laid out; retry this
/// many times before using the fallback placement. Linux never reports one.
pub const STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS: u32 = 20;
pub const STARTUP_NOTICE_TRAY_RETRY_DELAY: Duration = Duration::from_millis(50);
/// Quiet (login/autostart) launches show the notice briefly.
pub const STARTUP_NOTICE_AUTOSTART_VISIBLE: Duration = Duration::from_secs(5);
/// After first-run setup, keep the tray hint up long enough to read.
pub const STARTUP_NOTICE_AFTER_SETUP_VISIBLE: Duration = Duration::from_secs(15);

/// Native window title of the launch notice, matching the shipping app.
pub const STARTUP_NOTICE_WINDOW_TITLE: &str = "Captures is running";
pub const STARTUP_NOTICE_TITLE: &str = "Captures is ready to use";
/// Followed by one key chip per token of the saved New Capture shortcut.
pub const STARTUP_NOTICE_HINT: &str = "Open New Capture with";

/// A rect in logical points, top-left origin, y growing downward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn is_finite(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Caret {
    None,
    Top,
    Bottom,
}

/// Where a notice goes when no usable tray rect exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FallbackEdge {
    /// macOS menu bar.
    Top,
    /// Windows taskbar.
    Bottom,
    /// Linux panels: follow whichever work-area inset looks like a panel.
    WorkAreaInsets,
}

impl FallbackEdge {
    pub const fn for_current_platform() -> Self {
        if cfg!(target_os = "macos") {
            Self::Top
        } else if cfg!(target_os = "windows") {
            Self::Bottom
        } else {
            Self::WorkAreaInsets
        }
    }

    pub fn resolve(self, monitor: LogicalRect, work_area: LogicalRect) -> Caret {
        match self {
            Self::Top => Caret::Top,
            Self::Bottom => Caret::Bottom,
            Self::WorkAreaInsets => fallback_edge_from_insets(monitor, work_area),
        }
    }
}

/// Whether the host should poll the tray rect for a short while before using
/// the fallback. Linux AppIndicator never reports one.
pub const fn should_retry_tray_rect() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// Transparent notice window frame plus where its caret points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub caret: Caret,
    /// Caret tip, in window-local x.
    pub caret_x: f64,
}

impl Placement {
    /// The visible card in window-local coordinates. Mirrors `.tray-notice`
    /// padding: the frame pad on every side except the caret side, where the
    /// caret (minus its overlap) takes the space.
    pub fn card_rect(&self) -> LogicalRect {
        let caret_side = TRAY_NOTICE_CARET_SIZE - TRAY_NOTICE_CARET_OVERLAP;
        let top = if self.caret == Caret::Top {
            caret_side
        } else {
            TRAY_NOTICE_FRAME_PAD
        };
        let bottom = if self.caret == Caret::Bottom {
            caret_side
        } else {
            TRAY_NOTICE_FRAME_PAD
        };
        LogicalRect {
            x: TRAY_NOTICE_FRAME_PAD,
            y: top,
            width: (self.width - TRAY_NOTICE_FRAME_PAD * 2.0).max(0.0),
            height: (self.height - top - bottom).max(0.0),
        }
    }

    /// Triangle `[tip, base_left, base_right]` in window-local coordinates,
    /// or `None` for a fallback placement without a tray icon.
    pub fn caret_triangle(&self, span: f64) -> Option<[(f64, f64); 3]> {
        let half = span / 2.0;
        match self.caret {
            Caret::None => None,
            Caret::Top => Some([
                (self.caret_x, 0.0),
                (self.caret_x - half, TRAY_NOTICE_CARET_SIZE),
                (self.caret_x + half, TRAY_NOTICE_CARET_SIZE),
            ]),
            Caret::Bottom => Some([
                (self.caret_x, self.height),
                (self.caret_x - half, self.height - TRAY_NOTICE_CARET_SIZE),
                (self.caret_x + half, self.height - TRAY_NOTICE_CARET_SIZE),
            ]),
        }
    }
}

/// Launch notice placement: anchor to a usable tray rect, else fall back.
pub fn startup_notice_placement(
    monitor: LogicalRect,
    work_area: LogicalRect,
    tray: Option<LogicalRect>,
    menu_bar_at_top: bool,
    fallback_edge: FallbackEdge,
) -> Placement {
    resolve_tray_notice_placement(
        monitor,
        work_area,
        tray,
        menu_bar_at_top,
        fallback_edge,
        STARTUP_NOTICE_WIDTH,
        STARTUP_NOTICE_HEIGHT,
    )
}

pub fn resolve_tray_notice_placement(
    monitor: LogicalRect,
    work_area: LogicalRect,
    tray: Option<LogicalRect>,
    menu_bar_at_top: bool,
    fallback_edge: FallbackEdge,
    card_width: f64,
    card_height: f64,
) -> Placement {
    let tray = tray.filter(|tray| tray_icon_rect_is_usable(monitor, *tray, menu_bar_at_top));
    match tray {
        Some(tray) => place_tray_notice(monitor, tray, card_width, card_height),
        None => fallback_tray_notice(
            monitor,
            work_area,
            if menu_bar_at_top {
                Caret::Top
            } else {
                fallback_edge.resolve(monitor, work_area)
            },
            card_width,
            card_height,
        ),
    }
}

pub fn tray_notice_window_size(card_width: f64, card_height: f64, with_caret: bool) -> (f64, f64) {
    let width = card_width + TRAY_NOTICE_FRAME_PAD * 2.0;
    let height = if with_caret {
        card_height + TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_SIZE
    } else {
        card_height + TRAY_NOTICE_FRAME_PAD * 2.0
    };
    (width, height)
}

fn tray_notice_caret_x(window_width: f64, window_x: f64, tray_center_x: f64) -> f64 {
    let min = TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_INSET;
    let max = (window_width - TRAY_NOTICE_FRAME_PAD - TRAY_NOTICE_CARET_INSET).max(min);
    (tray_center_x - window_x).clamp(min, max)
}

pub fn tray_icon_rect_is_usable(
    monitor: LogicalRect,
    tray: LogicalRect,
    menu_bar_at_top: bool,
) -> bool {
    if !tray.is_finite() || !monitor.is_finite() || tray.width <= 0.0 || tray.height <= 0.0 {
        return false;
    }
    let center_x = tray.x + tray.width / 2.0;
    let center_y = tray.y + tray.height / 2.0;
    if center_x < monitor.x
        || center_x > monitor.x + monitor.width
        || center_y < monitor.y
        || center_y > monitor.y + monitor.height
    {
        return false;
    }
    let from_top = center_y - monitor.y;
    let from_bottom = monitor.y + monitor.height - center_y;
    if menu_bar_at_top {
        // macOS extras always sit in the menu bar. Reject the unlaid-out
        // status item at the Cocoa origin, which maps to the bottom-left.
        return from_top <= STARTUP_NOTICE_TRAY_EDGE_BAND;
    }
    let from_left = center_x - monitor.x;
    let from_right = monitor.x + monitor.width - center_x;
    from_top <= STARTUP_NOTICE_TRAY_EDGE_BAND
        || from_bottom <= STARTUP_NOTICE_TRAY_EDGE_BAND
        || from_left <= STARTUP_NOTICE_TRAY_EDGE_BAND
        || from_right <= STARTUP_NOTICE_TRAY_EDGE_BAND
}

/// Linux: a panel shows up as a work-area inset. Prefer the bottom only when
/// it is clearly a panel and larger than any top inset.
pub fn fallback_edge_from_insets(monitor: LogicalRect, work_area: LogicalRect) -> Caret {
    let top_inset = (work_area.y - monitor.y).max(0.0);
    let bottom_inset = (monitor.y + monitor.height - (work_area.y + work_area.height)).max(0.0);
    const DISTINCT_PANEL: f64 = 16.0;
    if bottom_inset >= DISTINCT_PANEL && bottom_inset > top_inset {
        Caret::Bottom
    } else {
        Caret::Top
    }
}

pub fn fallback_tray_notice(
    monitor: LogicalRect,
    work_area: LogicalRect,
    edge: Caret,
    card_width: f64,
    card_height: f64,
) -> Placement {
    let (width, height) = tray_notice_window_size(card_width, card_height, false);
    let min_x = work_area.x + TRAY_NOTICE_SCREEN_MARGIN;
    let max_x = work_area.x + work_area.width - width - TRAY_NOTICE_SCREEN_MARGIN;
    let x =
        (work_area.x + work_area.width - width - 18.0).clamp(min_x.min(max_x), max_x.max(min_x));
    let unclamped_y = match edge {
        Caret::Bottom => work_area.y + work_area.height - height - 18.0,
        Caret::Top | Caret::None => {
            // Work area already excludes a menu bar / top panel. Otherwise keep
            // the historical inset that clears a 24–30px menu bar.
            if work_area.y > monitor.y + 1.0 {
                work_area.y + TRAY_NOTICE_SCREEN_MARGIN
            } else {
                monitor.y + 30.0
            }
        }
    };
    let min_y = work_area.y + TRAY_NOTICE_SCREEN_MARGIN;
    let max_y = work_area.y + work_area.height - height - TRAY_NOTICE_SCREEN_MARGIN;
    Placement {
        x,
        y: unclamped_y.clamp(min_y.min(max_y), max_y.max(min_y)),
        width,
        height,
        caret: Caret::None,
        caret_x: width / 2.0,
    }
}

pub fn place_tray_notice(
    monitor: LogicalRect,
    tray: LogicalRect,
    card_width: f64,
    card_height: f64,
) -> Placement {
    let fallback = fallback_tray_notice(monitor, monitor, Caret::Top, card_width, card_height);
    if tray.width <= 0.0 || tray.height <= 0.0 {
        return fallback;
    }

    let tray_center_x = tray.x + tray.width / 2.0;
    let tray_center_y = tray.y + tray.height / 2.0;
    let caret = if tray_center_y <= monitor.y + monitor.height / 2.0 {
        Caret::Top
    } else {
        Caret::Bottom
    };
    let (width, height) = tray_notice_window_size(card_width, card_height, true);
    let min_x = monitor.x + TRAY_NOTICE_SCREEN_MARGIN;
    let max_x = monitor.x + monitor.width - width - TRAY_NOTICE_SCREEN_MARGIN;
    let x = (tray_center_x - width / 2.0).clamp(min_x.min(max_x), max_x.max(min_x));
    let unclamped_y = match caret {
        Caret::Top => tray.y + tray.height - TRAY_NOTICE_TRAY_OVERLAP,
        Caret::Bottom => tray.y - height + TRAY_NOTICE_TRAY_OVERLAP,
        Caret::None => fallback.y,
    };
    let min_y = monitor.y + TRAY_NOTICE_SCREEN_MARGIN;
    let max_y = monitor.y + monitor.height - height - TRAY_NOTICE_SCREEN_MARGIN;
    let y = unclamped_y.clamp(min_y.min(max_y), max_y.max(min_y));

    Placement {
        x,
        y,
        width,
        height,
        caret,
        caret_x: tray_notice_caret_x(width, x, tray_center_x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice_monitor(width: f64, height: f64) -> LogicalRect {
        LogicalRect::new(0.0, 0.0, width, height)
    }

    fn notice_tray(x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
        LogicalRect::new(x, y, width, height)
    }

    fn place_startup_notice(monitor: LogicalRect, tray: Option<LogicalRect>) -> Placement {
        match tray {
            Some(tray) => {
                place_tray_notice(monitor, tray, STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT)
            }
            None => fallback_startup_notice(monitor, monitor, Caret::Top),
        }
    }

    fn fallback_startup_notice(
        monitor: LogicalRect,
        work_area: LogicalRect,
        edge: Caret,
    ) -> Placement {
        fallback_tray_notice(
            monitor,
            work_area,
            edge,
            STARTUP_NOTICE_WIDTH,
            STARTUP_NOTICE_HEIGHT,
        )
    }

    fn resolve_startup_notice_placement(
        monitor: LogicalRect,
        work_area: LogicalRect,
        tray: Option<LogicalRect>,
        menu_bar_at_top: bool,
    ) -> Placement {
        startup_notice_placement(
            monitor,
            work_area,
            tray,
            menu_bar_at_top,
            FallbackEdge::WorkAreaInsets,
        )
    }

    #[test]
    fn startup_notice_after_setup_lasts_three_times_as_long_as_autostart() {
        assert_eq!(
            STARTUP_NOTICE_AFTER_SETUP_VISIBLE,
            STARTUP_NOTICE_AUTOSTART_VISIBLE * 3
        );
    }

    #[test]
    fn startup_notice_falls_back_to_the_top_right_without_a_tray_rect() {
        let placement = place_startup_notice(notice_monitor(1440.0, 900.0), None);
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(placement.x, 1440.0 - window_width - 18.0);
        assert_eq!(placement.y, 30.0);
        assert_eq!(placement.width, window_width);
        assert_eq!(placement.height, window_height);
        assert_eq!(placement.caret, Caret::None);
        assert_eq!(placement.caret_triangle(STARTUP_NOTICE_CARET_SPAN), None);
    }

    #[test]
    fn startup_notice_centers_under_a_macos_menu_bar_icon() {
        let placement = place_startup_notice(
            notice_monitor(1440.0, 900.0),
            Some(notice_tray(800.0, 0.0, 28.0, 24.0)),
        );
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        assert_eq!(placement.caret, Caret::Top);
        assert_eq!(placement.width, window_width);
        assert_eq!(placement.height, window_height);
        assert_eq!(placement.x, 814.0 - window_width / 2.0);
        assert_eq!(placement.y, 24.0 - TRAY_NOTICE_TRAY_OVERLAP);
        assert_eq!(placement.caret_x, window_width / 2.0);
    }

    #[test]
    fn startup_notice_keeps_a_right_edge_icon_caret_on_the_card() {
        let placement = place_startup_notice(
            notice_monitor(1440.0, 900.0),
            Some(notice_tray(1410.0, 0.0, 28.0, 24.0)),
        );
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        let caret_max = window_width - TRAY_NOTICE_FRAME_PAD - TRAY_NOTICE_CARET_INSET;
        assert_eq!(placement.caret, Caret::Top);
        assert_eq!(
            placement.x,
            1440.0 - window_width - TRAY_NOTICE_SCREEN_MARGIN
        );
        assert_eq!(placement.caret_x, caret_max);
    }

    #[test]
    fn startup_notice_sits_above_a_windows_taskbar_tray_icon() {
        let placement = place_startup_notice(
            notice_monitor(1920.0, 1080.0),
            Some(notice_tray(1860.0, 1048.0, 24.0, 24.0)),
        );
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        let caret_max = window_width - TRAY_NOTICE_FRAME_PAD - TRAY_NOTICE_CARET_INSET;
        assert_eq!(placement.caret, Caret::Bottom);
        assert_eq!(
            placement.x,
            1920.0 - window_width - TRAY_NOTICE_SCREEN_MARGIN
        );
        assert_eq!(
            placement.y,
            1048.0 - window_height + TRAY_NOTICE_TRAY_OVERLAP
        );
        assert_eq!(placement.caret_x, caret_max);
    }

    #[test]
    fn macos_rejects_an_unlaid_out_status_item_at_the_bottom_left() {
        let monitor = notice_monitor(1440.0, 900.0);
        assert!(!tray_icon_rect_is_usable(
            monitor,
            notice_tray(0.0, 876.0, 28.0, 24.0),
            true
        ));
        assert!(tray_icon_rect_is_usable(
            monitor,
            notice_tray(800.0, 0.0, 28.0, 24.0),
            true
        ));
        let placement = resolve_startup_notice_placement(
            monitor,
            monitor,
            Some(notice_tray(0.0, 876.0, 28.0, 24.0)),
            true,
        );
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(placement.caret, Caret::None);
        assert_eq!(placement.x, 1440.0 - window_width - 18.0);
        assert_eq!(placement.y, 30.0);
    }

    #[test]
    fn macos_still_anchors_to_a_laid_out_menu_bar_icon() {
        let monitor = notice_monitor(1440.0, 900.0);
        let placement = resolve_startup_notice_placement(
            monitor,
            monitor,
            Some(notice_tray(800.0, 0.0, 28.0, 24.0)),
            true,
        );
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, true);
        assert_eq!(placement.caret, Caret::Top);
        assert_eq!(placement.x, 814.0 - window_width / 2.0);
        assert_eq!(placement.y, 24.0 - TRAY_NOTICE_TRAY_OVERLAP);
    }

    #[test]
    fn windows_fallback_without_a_tray_rect_sits_above_the_taskbar() {
        let monitor = notice_monitor(1920.0, 1080.0);
        let work_area = LogicalRect::new(0.0, 0.0, 1920.0, 1040.0);
        let placement = fallback_startup_notice(monitor, work_area, Caret::Bottom);
        let (window_width, window_height) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(placement.caret, Caret::None);
        assert_eq!(placement.x, 1920.0 - window_width - 18.0);
        assert_eq!(placement.y, 1040.0 - window_height - 18.0);
        assert!(tray_icon_rect_is_usable(
            monitor,
            notice_tray(1860.0, 1048.0, 24.0, 24.0),
            false
        ));
        // The platform rule is a parameter, so the Windows path is testable
        // on every host.
        assert_eq!(
            startup_notice_placement(monitor, work_area, None, false, FallbackEdge::Bottom),
            placement
        );
    }

    #[test]
    fn linux_fallback_follows_the_panel_edge_of_the_work_area() {
        let monitor = notice_monitor(1920.0, 1080.0);
        let top_panel = LogicalRect::new(0.0, 28.0, 1920.0, 1052.0);
        let bottom_panel = LogicalRect::new(0.0, 0.0, 1920.0, 1040.0);
        assert_eq!(fallback_edge_from_insets(monitor, top_panel), Caret::Top);
        assert_eq!(
            fallback_edge_from_insets(monitor, bottom_panel),
            Caret::Bottom
        );
        let below_panel = fallback_startup_notice(monitor, top_panel, Caret::Top);
        let (window_width, _) =
            tray_notice_window_size(STARTUP_NOTICE_WIDTH, STARTUP_NOTICE_HEIGHT, false);
        assert_eq!(below_panel.y, 38.0);
        assert_eq!(below_panel.x, 1920.0 - window_width - 18.0);
        assert_eq!(
            resolve_startup_notice_placement(monitor, bottom_panel, None, false),
            fallback_startup_notice(monitor, bottom_panel, Caret::Bottom)
        );
    }

    #[test]
    fn menu_bar_at_top_always_falls_back_to_the_top_edge() {
        let monitor = notice_monitor(1440.0, 900.0);
        let dock_bottom = LogicalRect::new(0.0, 25.0, 1440.0, 800.0);
        assert_eq!(
            startup_notice_placement(monitor, dock_bottom, None, true, FallbackEdge::Bottom),
            fallback_startup_notice(monitor, dock_bottom, Caret::Top)
        );
    }

    #[test]
    fn tray_notice_window_reserves_padding_and_caret_space() {
        assert_eq!(
            tray_notice_window_size(400.0, 118.0, false),
            (
                400.0 + TRAY_NOTICE_FRAME_PAD * 2.0,
                118.0 + TRAY_NOTICE_FRAME_PAD * 2.0
            )
        );
        assert_eq!(
            tray_notice_window_size(400.0, 118.0, true),
            (
                400.0 + TRAY_NOTICE_FRAME_PAD * 2.0,
                118.0 + TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_SIZE
            )
        );
        assert_eq!(
            tray_notice_window_size(440.0, 290.0, true),
            (
                440.0 + TRAY_NOTICE_FRAME_PAD * 2.0,
                290.0 + TRAY_NOTICE_FRAME_PAD + TRAY_NOTICE_CARET_SIZE
            )
        );
    }

    #[test]
    fn card_and_caret_geometry_follow_the_css_frame() {
        let top = place_startup_notice(
            notice_monitor(1440.0, 900.0),
            Some(notice_tray(800.0, 0.0, 28.0, 24.0)),
        );
        let card = top.card_rect();
        assert_eq!(card.x, TRAY_NOTICE_FRAME_PAD);
        assert_eq!(card.width, STARTUP_NOTICE_WIDTH);
        assert_eq!(card.y, TRAY_NOTICE_CARET_SIZE - TRAY_NOTICE_CARET_OVERLAP);
        assert_eq!(card.y + card.height, top.height - TRAY_NOTICE_FRAME_PAD);
        let [tip, left, right] = top.caret_triangle(STARTUP_NOTICE_CARET_SPAN).unwrap();
        assert_eq!(tip, (top.caret_x, 0.0));
        assert_eq!(left.1, TRAY_NOTICE_CARET_SIZE);
        assert_eq!(right.0 - left.0, STARTUP_NOTICE_CARET_SPAN);
        // The base sits inside the card so no seam shows.
        assert!(left.1 > card.y);

        let bottom = place_startup_notice(
            notice_monitor(1920.0, 1080.0),
            Some(notice_tray(1860.0, 1048.0, 24.0, 24.0)),
        );
        let card = bottom.card_rect();
        assert_eq!(card.y, TRAY_NOTICE_FRAME_PAD);
        assert_eq!(
            card.y + card.height,
            bottom.height - TRAY_NOTICE_CARET_SIZE + TRAY_NOTICE_CARET_OVERLAP
        );
        let [tip, left, _] = bottom.caret_triangle(STARTUP_NOTICE_CARET_SPAN).unwrap();
        assert_eq!(tip, (bottom.caret_x, bottom.height));
        assert!(left.1 < card.y + card.height);

        let fallback = place_startup_notice(notice_monitor(1440.0, 900.0), None);
        let card = fallback.card_rect();
        assert_eq!(
            (card.y, card.height),
            (TRAY_NOTICE_FRAME_PAD, STARTUP_NOTICE_HEIGHT)
        );
    }

    #[test]
    fn non_finite_tray_rects_are_rejected() {
        let monitor = notice_monitor(1440.0, 900.0);
        for tray in [
            notice_tray(f64::NAN, 0.0, 28.0, 24.0),
            notice_tray(800.0, 0.0, f64::INFINITY, 24.0),
            notice_tray(800.0, 0.0, 0.0, 24.0),
        ] {
            assert!(!tray_icon_rect_is_usable(monitor, tray, true));
            assert!(!tray_icon_rect_is_usable(monitor, tray, false));
        }
    }

    #[test]
    fn fallback_edge_rules_map_to_platform_behavior() {
        let monitor = notice_monitor(1920.0, 1080.0);
        let bottom_panel = LogicalRect::new(0.0, 0.0, 1920.0, 1040.0);
        assert_eq!(FallbackEdge::Top.resolve(monitor, bottom_panel), Caret::Top);
        assert_eq!(
            FallbackEdge::Bottom.resolve(monitor, monitor),
            Caret::Bottom
        );
        assert_eq!(
            FallbackEdge::WorkAreaInsets.resolve(monitor, bottom_panel),
            Caret::Bottom
        );
        let expected = if cfg!(target_os = "macos") {
            FallbackEdge::Top
        } else if cfg!(target_os = "windows") {
            FallbackEdge::Bottom
        } else {
            FallbackEdge::WorkAreaInsets
        };
        assert_eq!(FallbackEdge::for_current_platform(), expected);
    }
}
