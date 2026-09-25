//! Tray/menu-bar anchored notice placement, ported from the shipping Tauri
//! `resolve_tray_notice_placement` (`apps/desktop/src-tauri/src/lib.rs`).
//!
//! All rectangles are logical desktop coordinates with a top-left origin. Hosts
//! convert OS rectangles (for example AppKit's bottom-left screen frames) before
//! calling and convert the returned window frame back. The returned window
//! includes transparent frame padding so the card shadow is not clipped, plus
//! room for the caret on the edge facing the tray icon.
use serde::{Deserialize, Serialize};

/// Transparent padding around the rounded card so `--shadow-md` is not clipped.
pub const FRAME_PAD: f64 = 28.0;
/// Extra window height reserved for the tray-pointing caret.
pub const CARET_SIZE: f64 = 8.0;
/// Width of the caret's base (`--tray-caret-span`).
pub const CARET_SPAN: f64 = 14.0;
/// The caret overlaps the card border by this much so no seam shows.
pub const CARET_OVERLAP: f64 = 1.0;
/// Keep the caret off the rounded corners of the card.
const CARET_INSET: f64 = 24.0;
/// Pull the transparent window over the tray so the caret tip sits on the icon.
const TRAY_OVERLAP: f64 = 2.0;
const SCREEN_MARGIN: f64 = 10.0;
/// Status items and tray icons live in a thin band along a display edge.
const TRAY_EDGE_BAND: f64 = 56.0;
const DISTINCT_PANEL: f64 = 16.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Caret {
    None,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Native window frame (card plus transparent padding and caret room).
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub caret: Caret,
    /// Caret tip, relative to the window's left edge.
    pub caret_x: f64,
}

impl Placement {
    /// The card rectangle inside the window, relative to the window origin.
    pub fn card(&self) -> LogicalRect {
        let top = match self.caret {
            Caret::Top => CARET_SIZE - CARET_OVERLAP,
            Caret::Bottom | Caret::None => FRAME_PAD,
        };
        let bottom = match self.caret {
            Caret::Bottom => CARET_SIZE - CARET_OVERLAP,
            Caret::Top | Caret::None => FRAME_PAD,
        };
        LogicalRect {
            x: FRAME_PAD,
            y: top,
            width: (self.width - FRAME_PAD * 2.0).max(0.0),
            height: (self.height - top - bottom).max(0.0),
        }
    }
}

/// Places a notice of `card_width` x `card_height` at the tray icon when its
/// rectangle is usable, or in the platform's tray corner otherwise.
pub fn placement(
    monitor: LogicalRect,
    work_area: LogicalRect,
    tray: Option<LogicalRect>,
    menu_bar_at_top: bool,
    card_width: f64,
    card_height: f64,
) -> Placement {
    let tray = tray.filter(|tray| tray_rect_is_usable(monitor, *tray, menu_bar_at_top));
    match tray {
        Some(tray) => place_at_tray(monitor, tray, card_width, card_height),
        None => fallback(
            monitor,
            work_area,
            if menu_bar_at_top {
                Caret::Top
            } else {
                fallback_edge(monitor, work_area)
            },
            card_width,
            card_height,
        ),
    }
}

fn window_size(card_width: f64, card_height: f64, with_caret: bool) -> (f64, f64) {
    let width = card_width + FRAME_PAD * 2.0;
    let height = if with_caret {
        card_height + FRAME_PAD + CARET_SIZE
    } else {
        card_height + FRAME_PAD * 2.0
    };
    (width, height)
}

fn caret_x(window_width: f64, window_x: f64, tray_center_x: f64) -> f64 {
    let min = FRAME_PAD + CARET_INSET;
    let max = (window_width - FRAME_PAD - CARET_INSET).max(min);
    (tray_center_x - window_x).clamp(min, max)
}

fn tray_rect_is_usable(monitor: LogicalRect, tray: LogicalRect, menu_bar_at_top: bool) -> bool {
    if tray.width <= 0.0 || tray.height <= 0.0 {
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
        // macOS status items always sit in the menu bar. Reject an unlaid-out
        // item reported at the Cocoa origin (bottom-left).
        return from_top <= TRAY_EDGE_BAND;
    }
    let from_left = center_x - monitor.x;
    let from_right = monitor.x + monitor.width - center_x;
    from_top <= TRAY_EDGE_BAND
        || from_bottom <= TRAY_EDGE_BAND
        || from_left <= TRAY_EDGE_BAND
        || from_right <= TRAY_EDGE_BAND
}

/// Which edge a notice without a usable tray rectangle should hug.
pub fn fallback_edge(monitor: LogicalRect, work_area: LogicalRect) -> Caret {
    if cfg!(target_os = "macos") {
        Caret::Top
    } else if cfg!(target_os = "windows") {
        Caret::Bottom
    } else {
        fallback_edge_from_insets(monitor, work_area)
    }
}

/// Linux panels: prefer a distinct bottom panel, otherwise the top.
pub fn fallback_edge_from_insets(monitor: LogicalRect, work_area: LogicalRect) -> Caret {
    let top_inset = (work_area.y - monitor.y).max(0.0);
    let bottom_inset = (monitor.y + monitor.height - (work_area.y + work_area.height)).max(0.0);
    if bottom_inset >= DISTINCT_PANEL && bottom_inset > top_inset {
        Caret::Bottom
    } else {
        Caret::Top
    }
}

fn fallback(
    monitor: LogicalRect,
    work_area: LogicalRect,
    edge: Caret,
    card_width: f64,
    card_height: f64,
) -> Placement {
    let (width, height) = window_size(card_width, card_height, false);
    let min_x = work_area.x + SCREEN_MARGIN;
    let max_x = work_area.x + work_area.width - width - SCREEN_MARGIN;
    let x =
        (work_area.x + work_area.width - width - 18.0).clamp(min_x.min(max_x), max_x.max(min_x));
    let unclamped_y = match edge {
        Caret::Bottom => work_area.y + work_area.height - height - 18.0,
        Caret::Top | Caret::None => {
            // The work area already excludes a menu bar / top panel. Otherwise
            // keep the historical inset that clears a 24-30px menu bar.
            if work_area.y > monitor.y + 1.0 {
                work_area.y + SCREEN_MARGIN
            } else {
                monitor.y + 30.0
            }
        }
    };
    let min_y = work_area.y + SCREEN_MARGIN;
    let max_y = work_area.y + work_area.height - height - SCREEN_MARGIN;
    Placement {
        x,
        y: unclamped_y.clamp(min_y.min(max_y), max_y.max(min_y)),
        width,
        height,
        caret: Caret::None,
        caret_x: width / 2.0,
    }
}

fn place_at_tray(
    monitor: LogicalRect,
    tray: LogicalRect,
    card_width: f64,
    card_height: f64,
) -> Placement {
    let tray_center_x = tray.x + tray.width / 2.0;
    let tray_center_y = tray.y + tray.height / 2.0;
    let caret = if tray_center_y <= monitor.y + monitor.height / 2.0 {
        Caret::Top
    } else {
        Caret::Bottom
    };
    let (width, height) = window_size(card_width, card_height, true);
    let min_x = monitor.x + SCREEN_MARGIN;
    let max_x = monitor.x + monitor.width - width - SCREEN_MARGIN;
    let x = (tray_center_x - width / 2.0).clamp(min_x.min(max_x), max_x.max(min_x));
    let unclamped_y = match caret {
        Caret::Top => tray.y + tray.height - TRAY_OVERLAP,
        Caret::Bottom | Caret::None => tray.y - height + TRAY_OVERLAP,
    };
    let min_y = monitor.y + SCREEN_MARGIN;
    let max_y = monitor.y + monitor.height - height - SCREEN_MARGIN;
    Placement {
        x,
        y: unclamped_y.clamp(min_y.min(max_y), max_y.max(min_y)),
        width,
        height,
        caret,
        caret_x: caret_x(width, x, tray_center_x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: LogicalRect = LogicalRect {
        x: 0.0,
        y: 0.0,
        width: 1440.0,
        height: 900.0,
    };

    fn rect(x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
        LogicalRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn top_tray_points_the_caret_up_at_the_icon() {
        let tray = rect(1200.0, 0.0, 24.0, 24.0);
        let placed = placement(MONITOR, MONITOR, Some(tray), true, 400.0, 168.0);
        assert_eq!(placed.caret, Caret::Top);
        assert_eq!(placed.width, 456.0);
        assert_eq!(placed.height, 168.0 + FRAME_PAD + CARET_SIZE);
        assert_eq!(placed.y, 22.0);
        // The window clamps to the screen margin; the caret still aims at the icon.
        assert_eq!(placed.x, 1440.0 - 456.0 - 10.0);
        assert_eq!(placed.x + placed.caret_x, 1212.0);
        assert_eq!(placed.card(), rect(28.0, 7.0, 400.0, 168.0 + CARET_OVERLAP));
    }

    #[test]
    fn bottom_taskbar_points_the_caret_down() {
        let tray = rect(700.0, 868.0, 24.0, 32.0);
        let placed = placement(MONITOR, MONITOR, Some(tray), false, 400.0, 224.0);
        assert_eq!(placed.caret, Caret::Bottom);
        assert_eq!(placed.y + placed.height, 868.0 + 2.0);
        assert_eq!(placed.x + placed.caret_x, 712.0);
        let card = placed.card();
        assert_eq!(card.y, FRAME_PAD);
        // As in the shipping CSS, the card extends under the caret overlap.
        assert_eq!(card.height, 224.0 + CARET_OVERLAP);
    }

    #[test]
    fn caret_stays_off_rounded_corners() {
        let tray = rect(1420.0, 0.0, 20.0, 24.0);
        let placed = placement(MONITOR, MONITOR, Some(tray), false, 400.0, 168.0);
        assert_eq!(placed.caret_x, placed.width - FRAME_PAD - CARET_INSET);
    }

    #[test]
    fn unusable_trays_fall_back_to_the_platform_corner() {
        // macOS status item reported at the Cocoa origin before layout.
        let unlaid = rect(0.0, 876.0, 24.0, 24.0);
        let placed = placement(MONITOR, MONITOR, Some(unlaid), true, 400.0, 168.0);
        assert_eq!(placed.caret, Caret::None);
        assert_eq!(placed.y, 30.0);
        assert_eq!(placed.x, 1440.0 - 456.0 - 18.0);
        assert_eq!(placed.card().height, 168.0);
        // Center of the screen is not a tray.
        let center = rect(700.0, 400.0, 24.0, 24.0);
        assert_eq!(
            placement(MONITOR, MONITOR, Some(center), false, 400.0, 168.0).caret,
            Caret::None
        );
    }

    #[test]
    fn linux_fallback_prefers_a_distinct_bottom_panel() {
        assert_eq!(
            fallback_edge_from_insets(MONITOR, rect(0.0, 0.0, 1440.0, 860.0)),
            Caret::Bottom
        );
        assert_eq!(
            fallback_edge_from_insets(MONITOR, rect(0.0, 28.0, 1440.0, 872.0)),
            Caret::Top
        );
        assert_eq!(fallback_edge_from_insets(MONITOR, MONITOR), Caret::Top);
        let placed = fallback(
            MONITOR,
            rect(0.0, 0.0, 1440.0, 860.0),
            Caret::Bottom,
            400.0,
            168.0,
        );
        assert_eq!(placed.y, 860.0 - 224.0 - 18.0);
    }
}
