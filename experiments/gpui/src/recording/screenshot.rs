//! Screenshot pixels and durable history, independent of the selector renderer.
use anyhow::{Context, Result};
use captures_capture::{DisplayFrame, PhysicalRect, PointerCursor, WindowDescriptor, XcapBackend};
use captures_recording::RecordingTarget;
use image::RgbaImage;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Capture {
    pub path: PathBuf,
    pub png: Vec<u8>,
}

pub fn pixels(
    frame: &DisplayFrame,
    target: &RecordingTarget,
    windows: &[WindowDescriptor],
    cursor: Option<&PointerCursor>,
) -> Result<RgbaImage> {
    let d = &frame.descriptor;
    let scale = d.overlay_to_buffer_scale(frame.image.width(), frame.image.height());
    let crop = match target {
        RecordingTarget::Display { .. } => None,
        RecordingTarget::Region { rect, .. } => {
            anyhow::ensure!(
                rect.x >= 0 && rect.y >= 0,
                "region starts outside the display"
            );
            Some(PhysicalRect {
                x: (f64::from(rect.x) * scale).round() as u32,
                y: (f64::from(rect.y) * scale).round() as u32,
                width: (f64::from(rect.width) * scale).round() as u32,
                height: (f64::from(rect.height) * scale).round() as u32,
            })
        }
        RecordingTarget::Window { window_id } => {
            let w = windows
                .iter()
                .find(|w| &w.id == window_id)
                .context("window is no longer available")?;
            let geometry_scale = if captures_capture::DisplayDescriptor::reports_physical_geometry()
            {
                d.scale_factor.max(1.)
            } else {
                1.
            };
            let x = (f64::from(w.x - d.x) / geometry_scale * scale).round();
            let y = (f64::from(w.y - d.y) / geometry_scale * scale).round();
            let right = (x + f64::from(w.width) / geometry_scale * scale)
                .min(f64::from(frame.image.width()));
            let bottom = (y + f64::from(w.height) / geometry_scale * scale)
                .min(f64::from(frame.image.height()));
            let x = x.max(0.);
            let y = y.max(0.);
            anyhow::ensure!(right > x && bottom > y, "window is outside the display");
            Some(PhysicalRect {
                x: x as u32,
                y: y as u32,
                width: (right - x) as u32,
                height: (bottom - y) as u32,
            })
        }
    };
    let mut image = match crop {
        Some(rect) => frame
            .crop(rect)
            .context("selected region is outside the display")?,
        None => frame.image.clone(),
    };
    if let Some(cursor) = cursor {
        captures_capture::overlay_pointer_cursor_in_crop(
            &mut image,
            d,
            crop.map_or(0, |r| r.x),
            crop.map_or(0, |r| r.y),
            frame.image.width(),
            frame.image.height(),
            cursor,
            captures_capture::screenshot_pointer_scale(d.scale_factor),
        );
    }
    Ok(image)
}

pub fn capture(
    profile: &Path,
    target: RecordingTarget,
    display_id: &str,
    frozen: Option<Arc<DisplayFrame>>,
    windows: &[WindowDescriptor],
    cursor: Option<&PointerCursor>,
) -> Result<Capture> {
    anyhow::ensure!(
        captures_session::capture_session_available(),
        "desktop session is unavailable"
    );
    let backend = XcapBackend;
    backend.ensure_permission(true)?;
    let image = if frozen.is_none() && matches!(target, RecordingTarget::Window { .. }) {
        let RecordingTarget::Window { window_id } = &target else {
            unreachable!()
        };
        let mut image = backend.capture_window(window_id)?;
        if let (Some(cursor), Some(window)) = (cursor, windows.iter().find(|w| &w.id == window_id))
        {
            let scale = backend
                .displays()?
                .into_iter()
                .find(|d| d.id == window.display_id)
                .map_or(1., |d| d.scale_factor);
            captures_capture::overlay_pointer_cursor_on_window(
                &mut image,
                window,
                cursor,
                captures_capture::screenshot_pointer_scale(scale),
            );
        }
        image
    } else {
        let frame = match frozen {
            Some(frame) => frame,
            None => Arc::new(backend.capture_display(display_id)?),
        };
        pixels(&frame, &target, windows, cursor)?
    };
    let png = crate::document::encoder::encode_png(&image, None).map_err(anyhow::Error::msg)?;
    let directory = profile.join("captures");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("Captures_{}.png", uuid::Uuid::new_v4()));
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    file.write_all(&png)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(&path).map_err(|e| e.error)?;
    Ok(Capture { path, png })
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_capture::DisplayDescriptor;
    use captures_recording::CaptureRect;
    fn frame() -> DisplayFrame {
        DisplayFrame {
            descriptor: DisplayDescriptor {
                id: "display".into(),
                name: "test".into(),
                x: -100,
                y: 30,
                width: 120,
                height: 80,
                scale_factor: 1.,
                is_primary: true,
            },
            image: RgbaImage::from_fn(240, 160, |x, y| image::Rgba([x as u8, y as u8, 73, 255])),
        }
    }
    #[test]
    fn frozen_region_maps_logical_coordinates_to_source_pixels() {
        let f = frame();
        let target = RecordingTarget::Region {
            display_id: "display".into(),
            rect: CaptureRect {
                x: 11,
                y: 7,
                width: 41,
                height: 19,
            },
        };
        let image = pixels(&f, &target, &[], None).unwrap();
        assert_eq!(image.dimensions(), (82, 38));
        assert_eq!(image.get_pixel(0, 0).0, [22, 14, 73, 255]);
        assert_eq!(image.get_pixel(81, 37).0, [103, 51, 73, 255]);
    }
    #[test]
    fn frozen_window_clips_negative_monitor_origin_without_shifting_pixels() {
        let f = frame();
        let w = WindowDescriptor {
            id: "window".into(),
            title: "test".into(),
            app_name: None,
            z_order: 0,
            x: -110,
            y: 43,
            width: 43,
            height: 27,
            display_id: "display".into(),
            corner_radius: None,
        };
        let image = pixels(
            &f,
            &RecordingTarget::Window {
                window_id: w.id.clone(),
            },
            &[w],
            None,
        )
        .unwrap();
        assert_eq!(image.dimensions(), (66, 54));
        assert_eq!(image.get_pixel(0, 0).0, [0, 26, 73, 255]);
        assert_eq!(image.get_pixel(65, 53).0, [65, 79, 73, 255]);
    }
    #[test]
    fn region_outside_frame_is_not_silently_clamped() {
        let target = RecordingTarget::Region {
            display_id: "display".into(),
            rect: CaptureRect {
                x: 110,
                y: 7,
                width: 41,
                height: 19,
            },
        };
        assert!(pixels(&frame(), &target, &[], None).is_err());
    }
}
