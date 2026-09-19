//! A bounded region-selection session. Frozen pixels stay in one owned buffer;
//! nothing is persisted until confirmation wins the shared cancellation gate.
use crate::{Artifact, Error, capture_flow, persist_screenshot};
use captures_capture::{
    CaptureError, CaptureMode, DisplayDescriptor, DisplayFrame, LogicalRect, PointerCursor,
    XcapBackend, overlay_pointer_cursor_in_crop, pointer_cursor, screenshot_pointer_scale,
};
use image::RgbaImage;
use std::path::Path;

pub struct RegionSession {
    generation: u64,
    display: DisplayDescriptor,
    frozen: Option<DisplayFrame>,
    cursor: Option<PointerCursor>,
    include_cursor: bool,
}

impl RegionSession {
    /// Call off the UI thread after hiding capture windows. Start a CaptureFlow
    /// first so Escape/session lock can cancel even while preparation blocks.
    pub fn prepare(
        display_id: &str,
        generation: u64,
        freeze: bool,
        include_cursor: bool,
    ) -> Result<Self, Error> {
        ensure_active(generation)?;
        XcapBackend.ensure_permission(false)?;
        captures_session::dismiss_transient_shell_ui_before_capture();
        let cursor = (freeze && include_cursor).then(pointer_cursor).flatten();
        let frozen = if freeze {
            Some(XcapBackend.capture_display(display_id)?)
        } else {
            None
        };
        let display = if let Some(frame) = &frozen {
            frame.descriptor.clone()
        } else {
            XcapBackend
                .displays()?
                .into_iter()
                .find(|d| d.id == display_id)
                .ok_or(CaptureError::TargetUnavailable)?
        };
        ensure_active(generation)?;
        Ok(Self {
            generation,
            display,
            frozen,
            cursor,
            include_cursor,
        })
    }

    pub fn display(&self) -> &DisplayDescriptor {
        &self.display
    }

    /// Borrowed, straight-alpha RGBA8 in row-major order. Valid until this session
    /// is dropped; the host must retain the session while any image provider reads.
    pub fn frozen_image(&self) -> Option<&RgbaImage> {
        self.frozen.as_ref().map(|f| &f.image)
    }

    /// Off-main-thread final capture. Like shipping Captures, a nonzero countdown
    /// means fresh pixels/cursor even when the selection preview was frozen.
    /// The caller hides selection/countdown windows before invoking this method.
    pub fn capture(
        &self,
        root: &Path,
        rect: LogicalRect,
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
        let image = self.image(rect, after_countdown, || {
            captures_session::dismiss_transient_shell_ui_before_capture();
            let cursor = self.include_cursor.then(pointer_cursor).flatten();
            Ok((XcapBackend.capture_display(&self.display.id)?, cursor))
        })?;
        ensure_active(self.generation)?;
        if !capture_flow::commit(self.generation) {
            return Err(Error::Cancelled);
        }
        persist_screenshot(root, &image, CaptureMode::Region)
    }

    fn image(
        &self,
        rect: LogicalRect,
        after_countdown: bool,
        capture: impl FnOnce() -> Result<(DisplayFrame, Option<PointerCursor>), Error>,
    ) -> Result<RgbaImage, Error> {
        validate_rect(&self.display, rect)?;
        if let Some(frozen) = self.frozen.as_ref().filter(|_| !after_countdown) {
            return crop(
                frozen,
                rect,
                self.cursor.as_ref().filter(|_| self.include_cursor),
            );
        }
        let (fresh, cursor) = capture()?;
        validate_display(&self.display, &fresh.descriptor)?;
        crop(
            &fresh,
            rect,
            cursor.as_ref().filter(|_| self.include_cursor),
        )
    }
}

pub(super) fn ensure_active(generation: u64) -> Result<(), Error> {
    if !capture_flow::is_current(generation) {
        return Err(Error::Cancelled);
    }
    if !captures_session::capture_session_available() {
        return Err(CaptureError::SessionUnavailable.into());
    }
    Ok(())
}

pub(super) fn validate_display(
    expected: &DisplayDescriptor,
    current: &DisplayDescriptor,
) -> Result<(), Error> {
    if expected.id != current.id
        || expected.x != current.x
        || expected.y != current.y
        || expected.width != current.width
        || expected.height != current.height
        || expected.scale_factor != current.scale_factor
    {
        return Err(Error::DisplayChanged);
    }
    Ok(())
}

pub(super) fn validate_rect(display: &DisplayDescriptor, rect: LogicalRect) -> Result<(), Error> {
    let (width, height) = display.overlay_size();
    if ![rect.x, rect.y, rect.width, rect.height]
        .into_iter()
        .all(f64::is_finite)
        || rect.x < 0.
        || rect.y < 0.
        || rect.width < 2.
        || rect.height < 2.
        || rect.x + rect.width > width
        || rect.y + rect.height > height
    {
        return Err(Error::InvalidRegion);
    }
    Ok(())
}

pub(super) fn crop(
    frame: &DisplayFrame,
    rect: LogicalRect,
    cursor: Option<&PointerCursor>,
) -> Result<RgbaImage, Error> {
    validate_rect(&frame.descriptor, rect)?;
    let scale = frame
        .descriptor
        .overlay_to_buffer_scale(frame.image.width(), frame.image.height());
    let physical = rect.to_physical(scale, frame.image.width(), frame.image.height());
    let mut image = frame.crop(physical).ok_or(Error::InvalidRegion)?;
    if let Some(cursor) = cursor {
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
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> DisplayFrame {
        DisplayFrame {
            descriptor: DisplayDescriptor {
                id: "left".into(),
                name: "Fixture".into(),
                x: -20,
                y: 7,
                width: 20,
                height: 12,
                scale_factor: 1.,
                is_primary: false,
            },
            image: RgbaImage::from_fn(40, 24, |x, y| image::Rgba([x as u8, y as u8 * 3, 17, 255])),
        }
    }
    #[test]
    fn countdown_always_uses_fresh_pixels_and_cursor() {
        let rect = LogicalRect {
            x: 3.25,
            y: 2.25,
            width: 7.5,
            height: 4.5,
        };
        let frozen_pointer = PointerCursor {
            position: (-16, 10),
            image: Some(captures_capture::CursorImage {
                pixels: RgbaImage::from_pixel(2, 2, image::Rgba([211, 19, 79, 255])),
                logical_width: 1.,
                logical_height: 1.,
                hot_spot_x: 0.,
                hot_spot_y: 0.,
            }),
        };
        for freeze in [false, true] {
            for countdown in [false, true] {
                for include_cursor in [false, true] {
                    let session = RegionSession {
                        generation: 0,
                        display: frame().descriptor,
                        frozen: freeze.then(frame),
                        cursor: Some(frozen_pointer.clone()),
                        include_cursor,
                    };
                    let mut fresh_calls = 0;
                    let pixels = session
                        .image(rect, countdown, || {
                            fresh_calls += 1;
                            let mut fresh = frame();
                            fresh.image.fill(101);
                            let cursor = PointerCursor {
                                position: (-17, 10),
                                ..frozen_pointer.clone()
                            };
                            Ok((fresh, Some(cursor)))
                        })
                        .unwrap();
                    if freeze && !countdown {
                        assert_eq!(fresh_calls, 0);
                        assert_eq!(
                            pixels.get_pixel(1, 1).0,
                            if include_cursor {
                                [211, 19, 79, 255]
                            } else {
                                [8, 18, 17, 255]
                            }
                        );
                        assert_eq!(
                            session.frozen_image().unwrap().get_pixel(8, 6).0,
                            [8, 18, 17, 255]
                        );
                    } else {
                        assert_eq!(fresh_calls, 1);
                        // Fresh cursor's hotspot is just outside the crop. No arrow fragment.
                        assert!(pixels.pixels().all(|p| p.0 == [101; 4]));
                    }
                }
            }
        }
    }
    #[test]
    fn fresh_cursor_is_drawn_at_its_new_crop_local_position() {
        let session = RegionSession {
            generation: 0,
            display: frame().descriptor,
            frozen: Some(frame()),
            cursor: None,
            include_cursor: true,
        };
        let pixels = session
            .image(
                LogicalRect {
                    x: 3.25,
                    y: 2.25,
                    width: 7.5,
                    height: 4.5,
                },
                true,
                || {
                    let mut fresh = frame();
                    fresh.image = RgbaImage::from_pixel(40, 24, image::Rgba([91, 64, 83, 255]));
                    Ok((
                        fresh,
                        Some(PointerCursor {
                            position: (-14, 11),
                            image: Some(captures_capture::CursorImage {
                                pixels: RgbaImage::from_pixel(
                                    2,
                                    2,
                                    image::Rgba([211, 19, 79, 255]),
                                ),
                                logical_width: 1.,
                                logical_height: 1.,
                                hot_spot_x: 0.,
                                hot_spot_y: 0.,
                            }),
                        }),
                    ))
                },
            )
            .unwrap();
        // Global (-14,11) maps to source (12,8), minus rounded crop origin (7,5).
        assert_eq!(pixels.get_pixel(5, 3).0, [211, 19, 79, 255]);
        assert_eq!(pixels.get_pixel(1, 1).0, [91, 64, 83, 255]);
    }
    #[test]
    fn rejects_invalid_rect_before_fresh_capture_and_revalidates_fresh_frame() {
        let session = RegionSession {
            generation: 0,
            display: frame().descriptor,
            frozen: None,
            cursor: None,
            include_cursor: false,
        };
        let rect = LogicalRect {
            x: 4.,
            y: 3.,
            width: 7.,
            height: 4.,
        };
        assert!(matches!(
            session.image(LogicalRect { x: -1., ..rect }, false, || panic!(
                "invalid region must not capture"
            )),
            Err(Error::InvalidRegion)
        ));
        assert!(matches!(
            session.image(rect, false, || {
                let mut fresh = frame();
                fresh.descriptor.scale_factor = 2.;
                Ok((fresh, None))
            }),
            Err(Error::DisplayChanged)
        ));
    }
    #[test]
    fn fractional_crop_uses_actual_buffer_edges_and_preserves_region_metadata() {
        let source = frame();
        let region = LogicalRect {
            x: 3.25,
            y: 2.25,
            width: 7.5,
            height: 4.5,
        };
        let pixels = crop(&source, region, None).unwrap();
        // Independent edge rounding: [6.5,4.5]→[7,5], [21.5,13.5]→[22,14].
        assert_eq!(pixels.dimensions(), (15, 9));
        assert_eq!(pixels.get_pixel(0, 0).0, [7, 15, 17, 255]);
        assert_eq!(pixels.get_pixel(14, 8).0, [21, 39, 17, 255]);
        let data = tempfile::tempdir().unwrap();
        let saved = persist_screenshot(data.path(), &pixels, CaptureMode::Region).unwrap();
        assert_eq!(saved.entry.mode, Some(CaptureMode::Region));
        assert_eq!(image::open(saved.image_path).unwrap().into_rgba8(), pixels);
        assert_eq!(source.image.dimensions(), (40, 24));
    }
    #[test]
    fn invalid_or_outside_selection_is_not_silently_clipped() {
        let source = frame();
        for rect in [
            LogicalRect {
                x: -0.1,
                y: 2.,
                width: 4.,
                height: 3.,
            },
            LogicalRect {
                x: 18.,
                y: 2.,
                width: 2.01,
                height: 3.,
            },
            LogicalRect {
                x: 3.,
                y: 10.,
                width: 2.,
                height: 2.01,
            },
            LogicalRect {
                x: 3.,
                y: 2.,
                width: 1.99,
                height: 3.,
            },
            LogicalRect {
                x: f64::NAN,
                y: 2.,
                width: 4.,
                height: 3.,
            },
        ] {
            assert!(matches!(
                crop(&source, rect, None),
                Err(Error::InvalidRegion)
            ));
        }
        assert!(
            crop(
                &source,
                LogicalRect {
                    x: 18.,
                    y: 10.,
                    width: 2.,
                    height: 2.
                },
                None
            )
            .is_ok()
        );
    }
    #[test]
    fn changed_geometry_or_scale_rejects_a_stale_selection_but_rename_does_not() {
        let expected = frame().descriptor;
        for kind in 0..6 {
            let mut current = expected.clone();
            match kind {
                0 => current.id = "another".into(),
                1 => current.x += 1,
                2 => current.y += 1,
                3 => current.width += 1,
                4 => current.height += 1,
                _ => current.scale_factor = 1.25,
            }
            assert!(matches!(
                validate_display(&expected, &current),
                Err(Error::DisplayChanged)
            ));
        }
        let mut renamed = expected.clone();
        renamed.name = "Renamed".into();
        assert!(validate_display(&expected, &renamed).is_ok());
    }
    #[test]
    fn stale_generation_does_not_capture_or_access_the_desktop() {
        assert!(matches!(
            RegionSession::prepare("not-a-display", 0, true, true),
            Err(Error::Cancelled)
        ));
    }
}
