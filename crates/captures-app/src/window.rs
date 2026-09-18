//! Native window selection owns one optional frozen desktop, its target snapshot,
//! and the original window stack. Hosts own presentation, not source selection.
use crate::{
    Artifact, Error, capture_flow, persist_screenshot,
    region::{ensure_active, validate_display},
    selection::Point,
};
use captures_capture::{
    CaptureError, CaptureMode, CaptureResult, DisplayDescriptor, DisplayFrame, PointerCursor,
    WindowDescriptor, WindowSelectionTargets, XcapBackend, classify_windows_for_display,
    overlay_pointer_cursor, overlay_pointer_cursor_in_crop, overlay_pointer_cursor_on_window,
    pointer_cursor, refine_window_chrome_from_snapshot, resolve_window_capture,
    screenshot_pointer_scale, window_display_crop_is_safe, window_physical_rect,
};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub use captures_capture::macos_window_corner_radius_for_major_version;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// Empty desktop and shell-chrome selections capture the whole display.
    Display,
    Window {
        id: String,
    },
}

/// Shipping picker policy in overlay coordinates. The frontmost shell strip or
/// an empty desktop hit means display capture (`None`). Equal z-order preserves
/// list order, with windows before shell chrome, and bounds have half-open edges.
/// This scans without allocating or sorting on pointer movement.
pub fn target_index_at_point(
    windows: &[WindowDescriptor],
    shell_chrome: &[WindowDescriptor],
    point: Point,
    origin: Point,
    scale: f64,
) -> Option<usize> {
    let scale = if scale > 0. { scale } else { 1. };
    let mut front_z = None;
    let mut target = None;
    for (index, window) in windows
        .iter()
        .enumerate()
        .map(|(i, w)| (Some(i), w))
        .chain(shell_chrome.iter().map(|w| (None, w)))
    {
        if window.width == 0 || window.height == 0 {
            continue;
        }
        let left = (f64::from(window.x) - origin.x) / scale;
        let top = (f64::from(window.y) - origin.y) / scale;
        if point.x >= left
            && point.y >= top
            && point.x < left + f64::from(window.width) / scale
            && point.y < top + f64::from(window.height) / scale
            && front_z.is_none_or(|z| window.z_order > z)
        {
            front_z = Some(window.z_order);
            target = index;
        }
    }
    target
}

pub struct WindowSession {
    generation: u64,
    display: DisplayDescriptor,
    frozen: Option<DisplayFrame>,
    cursor: Option<PointerCursor>,
    include_cursor: bool,
    targets: WindowSelectionTargets,
    // Keep non-pickable windows too: exclusion from the picker is not evidence
    // that their pixels cannot occlude another application's window.
    stack: Vec<WindowDescriptor>,
    fallback_corner_radius: f64,
}

impl WindowSession {
    /// Run off the UI thread after hiding capture windows, with a live CaptureFlow.
    /// The host supplies its OS window-radius fallback (zero outside macOS).
    /// No implicit permission prompt; unsupported window enumeration fails closed.
    pub fn prepare(
        display_id: &str,
        generation: u64,
        freeze: bool,
        include_cursor: bool,
        fallback_corner_radius: f64,
    ) -> Result<Self, Error> {
        ensure_active(generation)?;
        XcapBackend.ensure_permission(false)?;
        if !fallback_corner_radius.is_finite() || fallback_corner_radius < 0. {
            return Err(Error::InvalidWindowRadius);
        }
        captures_session::dismiss_transient_shell_ui_before_capture();
        let cursor = (freeze && include_cursor).then(pointer_cursor).flatten();
        let frozen = freeze
            .then(|| XcapBackend.capture_display(display_id))
            .transpose()?;
        let display = if let Some(frame) = &frozen {
            frame.descriptor.clone()
        } else {
            XcapBackend
                .displays()?
                .into_iter()
                .find(|d| d.id == display_id)
                .ok_or(CaptureError::TargetUnavailable)?
        };
        let stack = XcapBackend.windows()?;
        let mut targets = classify_windows_for_display(stack.clone(), &display);
        if let Some(frame) = &frozen {
            refine_window_chrome_from_snapshot(
                &mut targets.windows,
                &display,
                &frame.image,
                fallback_corner_radius,
            );
        }
        ensure_active(generation)?;
        Ok(Self {
            generation,
            display,
            frozen,
            cursor,
            include_cursor,
            targets,
            stack,
            fallback_corner_radius,
        })
    }

    pub fn display(&self) -> &DisplayDescriptor {
        &self.display
    }
    pub fn windows(&self) -> &[WindowDescriptor] {
        &self.targets.windows
    }
    pub fn shell_chrome(&self) -> &[WindowDescriptor] {
        &self.targets.shell_chrome
    }

    /// Point is display-local overlay/DIP space. The result indexes `windows()`;
    /// None means the display. Frozen and live selectors use the prepared list.
    pub fn hit_test(&self, point: Point) -> Option<usize> {
        target_index_at_point(
            self.windows(),
            self.shell_chrome(),
            point,
            Point {
                x: f64::from(self.display.x),
                y: f64::from(self.display.y),
            },
            if DisplayDescriptor::reports_physical_geometry() {
                self.display.scale_factor.max(1.)
            } else {
                1.
            },
        )
    }

    /// Borrowed RGBA8; retain the session until every provider/worker is finished.
    pub fn frozen_image(&self) -> Option<&RgbaImage> {
        self.frozen.as_ref().map(|frame| &frame.image)
    }

    /// After hiding/settling the selector/countdown. Frozen safe crops preserve
    /// the original pixels; a countdown always refreshes geometry and pixels.
    /// Cancellation/session checks surround capture and precede the commit gate.
    pub fn capture(
        &self,
        root: &Path,
        target: &Target,
        after_countdown: bool,
    ) -> Result<Artifact, Error> {
        ensure_active(self.generation)?;
        XcapBackend.ensure_permission(false)?;
        let current = XcapBackend
            .displays()?
            .into_iter()
            .find(|d| d.id == self.display.id)
            .ok_or(CaptureError::TargetUnavailable)?;
        validate_display(&self.display, &current)?;
        captures_session::dismiss_transient_shell_ui_before_capture();
        let (image, mode) = self.image(target, after_countdown, &XcapBackend)?;
        ensure_active(self.generation)?;
        if !capture_flow::commit(self.generation) {
            return Err(Error::Cancelled);
        }
        persist_screenshot(root, &image, mode)
    }

    fn image(
        &self,
        target: &Target,
        after_countdown: bool,
        source: &impl Source,
    ) -> Result<(RgbaImage, CaptureMode), Error> {
        let frozen = self.frozen.as_ref().filter(|_| !after_countdown);
        let Target::Window { id } = target else {
            let (mut image, cursor) = if let Some(frame) = frozen {
                (frame.image.clone(), self.cursor.clone())
            } else {
                let cursor = self.include_cursor.then(|| source.cursor()).flatten();
                let frame = source.display(&self.display.id)?;
                validate_display(&self.display, &frame.descriptor)?;
                (frame.image, cursor)
            };
            if let Some(cursor) = cursor.as_ref().filter(|_| self.include_cursor) {
                overlay_pointer_cursor(
                    &mut image,
                    &self.display,
                    cursor,
                    screenshot_pointer_scale(self.display.scale_factor),
                );
            }
            return Ok((image, CaptureMode::Display));
        };
        let selected = self
            .windows()
            .iter()
            .find(|window| &window.id == id)
            .ok_or(CaptureError::TargetUnavailable)?;
        let image = if let Some(frame) = frozen {
            resolve_window_capture(
                window_display_crop_is_safe(selected, &self.stack),
                || self.crop(frame, selected, self.cursor.as_ref()),
                || self.native(source, selected, self.cursor.as_ref()),
            )?
        } else {
            // Unlike a frozen snapshot, a live selection must still exist on its
            // selected display. Do not guess old bounds after enumeration fails.
            let stack = source.windows()?;
            let current = stack
                .iter()
                .find(|window| window.id == *id && window.display_id == self.display.id)
                .ok_or(CaptureError::TargetUnavailable)?;
            let safe = window_display_crop_is_safe(current, &stack);
            let crop = if safe {
                let cursor = self.include_cursor.then(|| source.cursor()).flatten();
                match source.display(&self.display.id) {
                    Ok(frame) => {
                        validate_display(&self.display, &frame.descriptor)?;
                        self.crop(&frame, current, cursor.as_ref())
                    }
                    Err(_) => None, // A native window surface can still succeed.
                }
            } else {
                None
            };
            resolve_window_capture(
                safe,
                || crop,
                || {
                    let cursor = self.include_cursor.then(|| source.cursor()).flatten();
                    self.native(source, current, cursor.as_ref())
                },
            )?
        };
        Ok((image, CaptureMode::Window))
    }

    fn crop(
        &self,
        frame: &DisplayFrame,
        window: &WindowDescriptor,
        cursor: Option<&PointerCursor>,
    ) -> Option<RgbaImage> {
        let physical = window_physical_rect(&frame.descriptor, &frame.image, window)?;
        let mut image = frame.crop(physical)?;
        #[cfg(target_os = "macos")]
        captures_capture::mask_macos_window_corners(
            &mut image,
            window,
            &frame.descriptor,
            captures_capture::capture_buffer_scale(&frame.descriptor, &frame.image),
            window
                .corner_radius
                .filter(|r| r.is_finite() && *r >= 0.)
                .unwrap_or(self.fallback_corner_radius),
        );
        #[cfg(not(target_os = "macos"))]
        let _ = self.fallback_corner_radius;
        if let Some(cursor) = cursor.filter(|_| self.include_cursor) {
            overlay_pointer_cursor_in_crop(
                &mut image,
                &frame.descriptor,
                physical.x,
                physical.y,
                frame.image.width(),
                frame.image.height(),
                cursor,
                screenshot_pointer_scale(frame.descriptor.scale_factor),
            );
        }
        Some(image)
    }

    fn native(
        &self,
        source: &impl Source,
        window: &WindowDescriptor,
        cursor: Option<&PointerCursor>,
    ) -> CaptureResult<RgbaImage> {
        let mut image = source.window(&window.id)?;
        if let Some(cursor) = cursor.filter(|_| self.include_cursor) {
            overlay_pointer_cursor_on_window(
                &mut image,
                window,
                cursor,
                screenshot_pointer_scale(self.display.scale_factor),
            );
        }
        Ok(image)
    }
}

// The source boundary tests orchestration with deterministic pixels/geometry,
// without a desktop, permission prompts or global hotkey registration.
trait Source {
    fn windows(&self) -> CaptureResult<Vec<WindowDescriptor>>;
    fn display(&self, id: &str) -> CaptureResult<DisplayFrame>;
    fn window(&self, id: &str) -> CaptureResult<RgbaImage>;
    fn cursor(&self) -> Option<PointerCursor>;
}

impl Source for XcapBackend {
    fn windows(&self) -> CaptureResult<Vec<WindowDescriptor>> {
        XcapBackend::windows(self)
    }
    fn display(&self, id: &str) -> CaptureResult<DisplayFrame> {
        self.capture_display(id)
    }
    fn window(&self, id: &str) -> CaptureResult<RgbaImage> {
        self.capture_window(id)
    }
    fn cursor(&self) -> Option<PointerCursor> {
        pointer_cursor()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn frame(blue: u8) -> DisplayFrame {
        DisplayFrame {
            descriptor: DisplayDescriptor {
                id: "left".into(),
                name: "Left".into(),
                x: -30,
                y: 10,
                width: 200,
                height: 140,
                scale_factor: 1.,
                is_primary: false,
            },
            image: RgbaImage::from_fn(200, 140, |x, y| image::Rgba([x as u8, y as u8, blue, 255])),
        }
    }
    fn window() -> WindowDescriptor {
        WindowDescriptor {
            id: "editor".into(),
            title: "Document".into(),
            app_name: Some("Editor".into()),
            z_order: 2,
            x: -10,
            y: 30,
            width: 70,
            height: 50,
            display_id: "left".into(),
            corner_radius: Some(0.),
        }
    }
    fn cursor(position: (i32, i32)) -> PointerCursor {
        PointerCursor {
            position,
            image: Some(captures_capture::CursorImage {
                pixels: RgbaImage::from_pixel(2, 2, image::Rgba([233, 17, 201, 255])),
                logical_width: 2.,
                logical_height: 2.,
                hot_spot_x: 0.,
                hot_spot_y: 0.,
            }),
        }
    }
    fn session(freeze: bool) -> WindowSession {
        WindowSession {
            generation: 0,
            display: frame(11).descriptor,
            frozen: freeze.then(|| frame(11)),
            cursor: Some(cursor((-4, 37))),
            include_cursor: false,
            targets: WindowSelectionTargets {
                windows: vec![window()],
                shell_chrome: vec![],
            },
            stack: vec![window()],
            fallback_corner_radius: 0.,
        }
    }
    fn target() -> Target {
        Target::Window {
            id: "editor".into(),
        }
    }
    struct Fake {
        windows: Vec<WindowDescriptor>,
        frame: DisplayFrame,
        native: Option<RgbaImage>,
        enumerate_error: bool,
        display_error: bool,
        calls: RefCell<Vec<&'static str>>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                windows: vec![WindowDescriptor {
                    x: -1,
                    y: 34,
                    width: 61,
                    height: 43,
                    ..window()
                }],
                frame: frame(97),
                native: Some(RgbaImage::from_fn(61, 43, |x, y| {
                    image::Rgba([x as u8, y as u8, 177, 255])
                })),
                enumerate_error: false,
                display_error: false,
                calls: RefCell::new(vec![]),
            }
        }
    }
    impl Source for Fake {
        fn windows(&self) -> CaptureResult<Vec<WindowDescriptor>> {
            self.calls.borrow_mut().push("windows");
            if self.enumerate_error {
                Err(CaptureError::Unsupported)
            } else {
                Ok(self.windows.clone())
            }
        }
        fn display(&self, id: &str) -> CaptureResult<DisplayFrame> {
            assert_eq!(id, "left");
            self.calls.borrow_mut().push("display");
            if self.display_error {
                Err(CaptureError::Backend("display failed".into()))
            } else {
                Ok(DisplayFrame {
                    descriptor: self.frame.descriptor.clone(),
                    image: self.frame.image.clone(),
                })
            }
        }
        fn window(&self, id: &str) -> CaptureResult<RgbaImage> {
            assert_eq!(id, "editor");
            self.calls.borrow_mut().push("native");
            self.native.clone().ok_or(CaptureError::TargetUnavailable)
        }
        fn cursor(&self) -> Option<PointerCursor> {
            self.calls.borrow_mut().push("cursor");
            Some(cursor((5, 41)))
        }
    }

    #[test]
    fn frozen_and_countdown_use_different_geometry_pixels_and_cursor() {
        for freeze in [false, true] {
            for countdown in [false, true] {
                for include_cursor in [false, true] {
                    let mut session = session(freeze);
                    session.include_cursor = include_cursor;
                    let source = Fake::default();
                    let (image, mode) = session.image(&target(), countdown, &source).unwrap();
                    let frozen = freeze && !countdown;
                    assert_eq!(mode, CaptureMode::Window);
                    assert_eq!(image.dimensions(), if frozen { (70, 50) } else { (61, 43) });
                    assert_eq!(
                        image.get_pixel(0, 0).0,
                        if frozen {
                            [20, 20, 11, 255]
                        } else {
                            [29, 24, 97, 255]
                        }
                    );
                    assert_eq!(
                        image.get_pixel(6, 7).0,
                        if include_cursor {
                            [233, 17, 201, 255]
                        } else if frozen {
                            [26, 27, 11, 255]
                        } else {
                            [35, 31, 97, 255]
                        }
                    );
                    assert!(!source.calls.borrow().contains(&"native"));
                    if frozen {
                        assert!(source.calls.borrow().is_empty());
                    }
                    if let Some(frozen) = session.frozen_image() {
                        assert_eq!(frozen.get_pixel(26, 27).0, [26, 27, 11, 255]);
                    }
                }
            }
        }
    }

    #[test]
    fn excluded_small_occluder_forces_native_without_reading_desktop() {
        for freeze in [false, true] {
            let mut session = session(freeze);
            let mut source = Fake::default();
            let occluder = WindowDescriptor {
                id: "other".into(),
                app_name: Some("Other".into()),
                z_order: 3,
                width: 20,
                height: 20,
                ..window()
            };
            assert!(
                classify_windows_for_display(vec![occluder.clone()], &session.display)
                    .windows
                    .is_empty()
            );
            session.stack.push(occluder.clone());
            source.windows.push(occluder);
            let (image, _) = session.image(&target(), false, &source).unwrap();
            assert_eq!(image.get_pixel(3, 7).0, [3, 7, 177, 255]);
            assert!(!source.calls.borrow().contains(&"display"));
            source.native = None;
            assert!(matches!(
                session.image(&target(), false, &source),
                Err(Error::Capture(CaptureError::WindowOccluded))
            ));
        }
    }

    #[test]
    fn blank_or_failed_display_uses_native_and_blank_native_is_rejected() {
        for failure in [false, true] {
            let session = session(false);
            let mut source = Fake {
                display_error: failure,
                ..Fake::default()
            };
            source.frame.image.fill(0);
            let (image, _) = session.image(&target(), false, &source).unwrap();
            assert_eq!(image.get_pixel(3, 7).0, [3, 7, 177, 255]);
            assert_eq!(*source.calls.borrow(), ["windows", "display", "native"]);
            source.native.as_mut().unwrap().fill(0);
            assert!(matches!(
                session.image(&target(), false, &source),
                Err(Error::Capture(CaptureError::WindowEmpty))
            ));
        }
    }

    #[test]
    fn live_targets_and_display_are_revalidated_before_using_pixels() {
        for case in 0..4 {
            let session = session(true);
            let mut source = Fake::default();
            match case {
                0 => source.windows.clear(),
                1 => source.windows[0].display_id = "right".into(),
                2 => source.enumerate_error = true,
                _ => source.frame.descriptor.scale_factor = 1.25,
            }
            let error = session.image(&target(), true, &source).unwrap_err();
            match case {
                0 | 1 => assert!(matches!(
                    error,
                    Error::Capture(CaptureError::TargetUnavailable)
                )),
                2 => assert!(matches!(error, Error::Capture(CaptureError::Unsupported))),
                _ => assert!(matches!(error, Error::DisplayChanged)),
            }
            assert!(!source.calls.borrow().contains(&"native"));
            if case < 3 {
                assert!(!source.calls.borrow().contains(&"display"));
            }
        }
        let source = Fake::default();
        assert!(
            session(true)
                .image(
                    &Target::Window {
                        id: "unlisted".into()
                    },
                    false,
                    &source
                )
                .is_err()
        );
        assert!(source.calls.borrow().is_empty());
    }

    #[test]
    fn display_fallback_preserves_mode_and_uses_fresh_pixels_after_countdown() {
        for countdown in [false, true] {
            let source = Fake::default();
            let (image, mode) = session(true)
                .image(&Target::Display, countdown, &source)
                .unwrap();
            assert_eq!(mode, CaptureMode::Display);
            assert_eq!(image.dimensions(), (200, 140));
            assert_eq!(
                image.get_pixel(127, 81).0,
                [127, 81, if countdown { 97 } else { 11 }, 255]
            );
            assert!(!source.calls.borrow().contains(&"windows"));
            let root = tempfile::tempdir().unwrap();
            let artifact = persist_screenshot(root.path(), &image, mode).unwrap();
            assert_eq!(artifact.entry.mode, Some(CaptureMode::Display));
            assert_eq!(
                image::open(artifact.image_path).unwrap().into_rgba8(),
                image
            );
        }
    }

    #[test]
    fn native_fallback_uses_window_local_cursor_and_does_not_mask_the_surface() {
        for freeze in [false, true] {
            let mut session = session(freeze);
            session.include_cursor = true;
            session.targets.windows[0].corner_radius = Some(12.);
            let mut source = Fake::default();
            let occluder = WindowDescriptor {
                id: "cover".into(),
                z_order: 3,
                ..window()
            };
            session.stack.push(occluder.clone());
            source.windows.push(occluder);
            let (image, mode) = session.image(&target(), false, &source).unwrap();
            // Frozen target 70x50 maps its (6,7) pointer to the native 61x43
            // surface at rounded (5,6). Fresh geometry already matches 61x43.
            let position = if freeze { (5, 6) } else { (6, 7) };
            assert_eq!(
                image.get_pixel(position.0, position.1).0,
                [233, 17, 201, 255]
            );
            assert_eq!(image.get_pixel(0, 0).0, [0, 0, 177, 255]);
            let root = tempfile::tempdir().unwrap();
            let artifact = persist_screenshot(root.path(), &image, mode).unwrap();
            assert_eq!(artifact.entry.mode, Some(CaptureMode::Window));
            assert_eq!(
                image::open(artifact.image_path).unwrap().into_rgba8(),
                image
            );
        }
    }

    #[test]
    fn only_macos_display_crops_apply_the_window_corner_mask() {
        let mut session = session(true);
        session.targets.windows[0].corner_radius = None;
        session.fallback_corner_radius = 8.;
        let (image, _) = session.image(&target(), false, &Fake::default()).unwrap();
        assert_eq!(
            image.get_pixel(0, 0).0[3],
            if cfg!(target_os = "macos") { 0 } else { 255 }
        );
        assert_eq!(image.get_pixel(20, 20).0, [40, 40, 11, 255]);
        // The shared borrowed preview remains unmasked and cursor-free.
        assert_eq!(
            session.frozen_image().unwrap().get_pixel(20, 20).0,
            [20, 20, 11, 255]
        );
    }

    #[test]
    fn cancelled_session_cannot_prepare_or_persist() {
        assert!(matches!(
            WindowSession::prepare("missing", 0, true, true, 0.),
            Err(Error::Cancelled)
        ));
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            session(true).capture(root.path(), &target(), false),
            Err(Error::Cancelled)
        ));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn hit_testing_matches_shipping_window_and_shell_vectors() {
        #[derive(Deserialize)]
        struct Case {
            windows: Vec<WindowDescriptor>,
            shell: Vec<WindowDescriptor>,
            point: Point,
            origin: Point,
            scale: f64,
            expected: Option<usize>,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/window-hit-golden.json")).unwrap();
        for (index, case) in cases.iter().enumerate() {
            assert_eq!(
                target_index_at_point(
                    &case.windows,
                    &case.shell,
                    case.point,
                    case.origin,
                    case.scale
                ),
                case.expected,
                "shipping hit vector {index}"
            );
        }
    }

    #[test]
    fn session_hit_coordinates_follow_the_platform_display_contract() {
        let mut session = session(false);
        session.display.scale_factor = 2.;
        // The window starts 20 native units from the display's negative origin.
        let factor = if cfg!(target_os = "windows") { 2. } else { 1. };
        assert_eq!(
            session.hit_test(Point {
                x: 20. / factor,
                y: 20. / factor
            }),
            Some(0)
        );
        assert_eq!(
            session.hit_test(Point {
                x: 90. / factor,
                y: 20. / factor
            }),
            None
        );
        assert_eq!(
            session.hit_test(Point {
                x: 19.99 / factor,
                y: 20. / factor
            }),
            None
        );
    }
}
