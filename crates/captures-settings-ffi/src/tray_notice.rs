//! Native hosts borrow the shipping tray / menu-bar notice placement.
use captures_app::tray_notice::{self, Caret, FallbackEdge, LogicalRect};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturesTrayNoticeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturesTrayNoticePlacement {
    /// Window frame, logical top-left desktop coordinates.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Caret tip x, window-local.
    pub caret_x: f64,
    /// Visible card, window-local.
    pub card: CapturesTrayNoticeRect,
    /// 0 none, 1 top, 2 bottom.
    pub caret: u32,
}

impl From<CapturesTrayNoticeRect> for LogicalRect {
    fn from(rect: CapturesTrayNoticeRect) -> Self {
        LogicalRect::new(rect.x, rect.y, rect.width, rect.height)
    }
}

fn finite_area(rect: CapturesTrayNoticeRect) -> bool {
    [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|value| value.is_finite())
        && rect.width > 0.
        && rect.height > 0.
}

/// Allocation-free launch-notice placement. See the header for the coordinate
/// convention and enum values.
///
/// # Safety
/// Non-null tray/output point to aligned readable/writable storage respectively.
/// Pointers are borrowed only during this call. False leaves output unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_startup_notice_placement_v1(
    monitor: CapturesTrayNoticeRect,
    work_area: CapturesTrayNoticeRect,
    tray: *const CapturesTrayNoticeRect,
    menu_bar_at_top: bool,
    fallback_edge: u32,
    output: *mut CapturesTrayNoticePlacement,
) -> bool {
    if output.is_null() || !finite_area(monitor) || !finite_area(work_area) {
        return false;
    }
    let fallback_edge = match fallback_edge {
        0 => FallbackEdge::Top,
        1 => FallbackEdge::Bottom,
        2 => FallbackEdge::WorkAreaInsets,
        _ => return false,
    };
    // SAFETY: The caller guarantees a readable aligned tray rect or null.
    // A non-finite or empty rect is an unlaid-out icon: use the fallback.
    let tray = unsafe { tray.as_ref() }.map(|tray| LogicalRect::from(*tray));
    let placement = tray_notice::startup_notice_placement(
        monitor.into(),
        work_area.into(),
        tray,
        menu_bar_at_top,
        fallback_edge,
    );
    let card = placement.card_rect();
    // SAFETY: Checked non-null above; the caller guarantees aligned storage.
    unsafe {
        output.write(CapturesTrayNoticePlacement {
            x: placement.x,
            y: placement.y,
            width: placement.width,
            height: placement.height,
            caret_x: placement.caret_x,
            card: CapturesTrayNoticeRect {
                x: card.x,
                y: card.y,
                width: card.width,
                height: card.height,
            },
            caret: match placement.caret {
                Caret::None => 0,
                Caret::Top => 1,
                Caret::Bottom => 2,
            },
        });
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CapturesTrayNoticeRect {
        CapturesTrayNoticeRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn header_declares_the_placement_abi_with_matching_layout() {
        let header = include_str!("../include/captures_settings.h");
        assert!(header.contains("bool captures_startup_notice_placement_v1("));
        assert!(header.contains("CapturesTrayNoticeRect card;"));
        assert_eq!(size_of::<CapturesTrayNoticeRect>(), 32);
        // 5 doubles + nested rect + u32 caret, padded to 8.
        assert_eq!(size_of::<CapturesTrayNoticePlacement>(), 40 + 32 + 8);
        assert_eq!(std::mem::offset_of!(CapturesTrayNoticePlacement, card), 40);
        assert_eq!(std::mem::offset_of!(CapturesTrayNoticePlacement, caret), 72);
    }

    #[test]
    fn menu_bar_icon_places_a_top_caret_notice() {
        let monitor = rect(0., 0., 1440., 900.);
        let work = rect(0., 25., 1440., 875.);
        let tray = rect(800., 0., 28., 24.);
        let mut out = CapturesTrayNoticePlacement::default();
        // SAFETY: Both pointers refer to valid local values.
        assert!(unsafe {
            captures_startup_notice_placement_v1(monitor, work, &tray, true, 0, &mut out)
        });
        assert_eq!(out.caret, 1);
        assert_eq!((out.width, out.height), (352., 90.));
        assert_eq!(out.x, 814. - 176.);
        assert_eq!(out.y, 22.);
        assert_eq!(out.caret_x, 176.);
        assert_eq!(out.card, rect(28., 7., 296., 55.));
    }

    #[test]
    fn missing_or_unlaid_out_tray_uses_the_requested_fallback_edge() {
        let monitor = rect(0., 0., 1920., 1080.);
        let taskbar = rect(0., 0., 1920., 1040.);
        let mut out = CapturesTrayNoticePlacement::default();
        // SAFETY: Output is local writable storage; tray is null.
        assert!(unsafe {
            captures_startup_notice_placement_v1(monitor, taskbar, null(), false, 1, &mut out)
        });
        assert_eq!(out.caret, 0);
        assert_eq!((out.x, out.y), (1920. - 352. - 18., 1040. - 110. - 18.));
        assert_eq!(out.card, rect(28., 28., 296., 54.));
        // Linux insets rule picks the same bottom edge for this work area.
        let mut linux = CapturesTrayNoticePlacement::default();
        // SAFETY: As above.
        assert!(unsafe {
            captures_startup_notice_placement_v1(monitor, taskbar, null(), false, 2, &mut linux)
        });
        assert_eq!(linux, out);
        // AppKit can report the status item at the Cocoa origin before layout.
        let unplaced = rect(0., 1056., 28., 24.);
        // SAFETY: Both pointers refer to valid local values.
        assert!(unsafe {
            captures_startup_notice_placement_v1(monitor, monitor, &unplaced, true, 0, &mut out)
        });
        assert_eq!((out.caret, out.y), (0, 30.));
        let nan = rect(f64::NAN, 0., 28., 24.);
        // SAFETY: Both pointers refer to valid local values.
        assert!(unsafe {
            captures_startup_notice_placement_v1(monitor, monitor, &nan, true, 0, &mut out)
        });
        assert_eq!(out.caret, 0);
    }

    #[test]
    fn invalid_requests_leave_output_unchanged() {
        let sentinel = CapturesTrayNoticePlacement {
            x: 917.,
            ..Default::default()
        };
        let mut out = sentinel;
        let good = rect(0., 0., 1440., 900.);
        for (monitor, work, edge) in [
            (rect(0., 0., 0., 900.), good, 0),
            (good, rect(f64::NAN, 0., 1440., 900.), 0),
            (good, good, 3),
        ] {
            // SAFETY: Output is local writable storage; tray is null.
            assert!(!unsafe {
                captures_startup_notice_placement_v1(monitor, work, null(), true, edge, &mut out)
            });
            assert_eq!(out, sentinel);
        }
        // SAFETY: Null output is rejected before any write.
        assert!(!unsafe {
            captures_startup_notice_placement_v1(good, good, null(), true, 0, null_mut())
        });
    }
}
