/// AppKit geometry stays in global points until the host positions the notice.
/// In particular, do not scale a global origin using one display's backing scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoticeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NoticeRect {
    fn contains(self, other: Self) -> bool {
        other.width > 0.0
            && other.height > 0.0
            && other.x >= self.x
            && other.y >= self.y
            && other.x + other.width <= self.x + self.width
            && other.y + other.height <= self.y + self.height
    }

    fn flip(self, primary_top: f64) -> Self {
        Self {
            y: primary_top - self.y - self.height,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrayNoticeGeometry {
    pub monitor: NoticeRect,
    pub work_area: NoticeRect,
    pub tray: Option<NoticeRect>,
}

fn resolve_geometry(
    primary_top: f64,
    monitor: NoticeRect,
    work_area: NoticeRect,
    tray: Option<NoticeRect>,
    safe_top: f64,
    auxiliary_areas: [NoticeRect; 2],
) -> TrayNoticeGeometry {
    let tray = tray.filter(|tray| {
        monitor.contains(*tray)
            && (safe_top <= 0.0 || auxiliary_areas.iter().any(|area| area.contains(*tray)))
    });
    let monitor = monitor.flip(primary_top);
    let mut work_area = work_area.flip(primary_top);
    // visibleFrame can include the camera housing when the menu bar auto-hides.
    let safe_y = monitor.y + safe_top;
    if work_area.y < safe_y {
        work_area.height = (work_area.height - (safe_y - work_area.y)).max(0.0);
        work_area.y = safe_y;
    }
    TrayNoticeGeometry {
        monitor,
        work_area,
        tray: tray.map(|tray| tray.flip(primary_top)),
    }
}

/// Read the live status-item window and its screen on the AppKit main thread.
/// A rectangle under/across the camera housing isn't an accessible anchor.
#[cfg(target_os = "macos")]
pub fn tray_notice_geometry(
    status_item: Option<&objc2_app_kit::NSStatusItem>,
) -> Option<TrayNoticeGeometry> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    use objc2_foundation::NSRect;

    fn rect(frame: NSRect) -> NoticeRect {
        NoticeRect {
            x: frame.origin.x,
            y: frame.origin.y,
            width: frame.size.width,
            height: frame.size.height,
        }
    }

    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let primary = screens.firstObject()?;
    let window = status_item
        .and_then(|item| item.button(mtm))
        .and_then(|button| button.window());
    let screen = window
        .as_ref()
        .and_then(|window| window.screen())
        .unwrap_or_else(|| primary.clone());
    let primary_frame = primary.frame();
    Some(resolve_geometry(
        primary_frame.origin.y + primary_frame.size.height,
        rect(screen.frame()),
        rect(screen.visibleFrame()),
        window.as_ref().map(|window| rect(window.frame())),
        screen.safeAreaInsets().top,
        [
            rect(screen.auxiliaryTopLeftArea()),
            rect(screen.auxiliaryTopRightArea()),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NoticeRect {
        NoticeRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn notched_menu_bar_only_accepts_fully_visible_items_on_either_side() {
        let monitor = rect(0.0, 0.0, 1512.0, 982.0);
        let areas = [
            rect(0.0, 950.0, 680.0, 32.0),
            rect(832.0, 950.0, 680.0, 32.0),
        ];
        for (x, visible) in [
            (652.0, true),
            (653.0, false),
            (740.0, false),
            (831.0, false),
            (832.0, true),
        ] {
            let geometry = resolve_geometry(
                982.0,
                monitor,
                monitor,
                Some(rect(x, 950.0, 28.0, 32.0)),
                32.0,
                areas,
            );
            assert_eq!(geometry.tray.is_some(), visible, "status item x={x}");
            assert_eq!(geometry.work_area, rect(0.0, 32.0, 1512.0, 950.0));
        }
    }

    #[test]
    fn unnotched_secondary_screen_uses_global_points_without_retina_scaling() {
        // A display above and left of the primary, with a different scale.
        let monitor = rect(-1920.0, 400.0, 1920.0, 1080.0);
        let work = rect(-1920.0, 440.0, 1920.0, 1016.0);
        let geometry = resolve_geometry(
            982.0,
            monitor,
            work,
            Some(rect(-310.0, 1456.0, 26.0, 24.0)),
            0.0,
            [rect(0.0, 0.0, 0.0, 0.0); 2],
        );
        assert_eq!(geometry.monitor, rect(-1920.0, -498.0, 1920.0, 1080.0));
        assert_eq!(geometry.work_area, rect(-1920.0, -474.0, 1920.0, 1016.0));
        assert_eq!(geometry.tray, Some(rect(-310.0, -498.0, 26.0, 24.0)));
    }

    #[test]
    fn no_item_and_offscreen_item_preserve_safe_fallback_on_selected_screen() {
        let monitor = rect(1512.0, -100.0, 1728.0, 1117.0);
        let work = rect(1512.0, -50.0, 1728.0, 1035.0);
        for tray in [None, Some(rect(100.0, 980.0, 28.0, 32.0))] {
            let geometry = resolve_geometry(
                982.0,
                monitor,
                work,
                tray,
                32.0,
                [rect(0.0, 0.0, 0.0, 0.0); 2],
            );
            assert_eq!(geometry.tray, None);
            assert_eq!(geometry.work_area, rect(1512.0, -3.0, 1728.0, 1035.0));
        }
    }
}
