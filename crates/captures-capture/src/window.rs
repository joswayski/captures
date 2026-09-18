//! Window pixel-source policy shared by the shipping and native hosts.
//! Hosts own permission/session checks, snapshot geometry, cropping and cursor
//! composition. Never invoke a composited crop unless its window stack is safe.
use image::RgbaImage;

use crate::{CaptureError, CaptureResult, WindowDescriptor};

pub fn resolve_window_capture(
    display_crop_is_safe: bool,
    display_crop: impl FnOnce() -> Option<RgbaImage>,
    native_capture: impl FnOnce() -> CaptureResult<RgbaImage>,
) -> CaptureResult<RgbaImage> {
    if display_crop_is_safe
        && let Some(image) = display_crop()
        && !image_is_effectively_blank(&image)
    {
        return Ok(image);
    }

    let native_error = match native_capture() {
        Ok(image) if !image_is_effectively_blank(&image) => return Ok(image),
        Ok(_) => None,
        Err(error) => Some(error),
    };

    if !display_crop_is_safe {
        return Err(CaptureError::WindowOccluded);
    }

    match native_error {
        None => Err(CaptureError::WindowEmpty),
        Some(error) => Err(error),
    }
}

pub fn window_display_crop_is_safe(
    selected: &WindowDescriptor,
    windows: &[WindowDescriptor],
) -> bool {
    windows.iter().any(|candidate| candidate.id == selected.id)
        && !windows.iter().any(|candidate| {
            candidate.id != selected.id
                && candidate.display_id == selected.display_id
                && candidate.z_order > selected.z_order
                && window_rects_overlap(selected, candidate)
                && !window_is_associated_transient(selected, candidate)
        })
}

fn window_is_associated_transient(
    selected: &WindowDescriptor,
    candidate: &WindowDescriptor,
) -> bool {
    // xcap exposes app-owned menus, popovers, and similar transient surfaces as
    // separate untitled windows. Keeping those in an otherwise safe display crop
    // preserves frozen/countdown states without admitting another document window.
    candidate.title.trim().is_empty()
        && selected
            .app_name
            .as_deref()
            .zip(candidate.app_name.as_deref())
            .is_some_and(|(selected_app, candidate_app)| {
                !selected_app.trim().is_empty()
                    && selected_app
                        .trim()
                        .eq_ignore_ascii_case(candidate_app.trim())
            })
}

fn window_rects_overlap(left: &WindowDescriptor, right: &WindowDescriptor) -> bool {
    let left_x = i64::from(left.x);
    let left_y = i64::from(left.y);
    let left_right = left_x + i64::from(left.width);
    let left_bottom = left_y + i64::from(left.height);
    let right_x = i64::from(right.x);
    let right_y = i64::from(right.y);
    let right_right = right_x + i64::from(right.width);
    let right_bottom = right_y + i64::from(right.height);

    left_x < right_right && right_x < left_right && left_y < right_bottom && right_y < left_bottom
}

pub fn image_is_effectively_blank(image: &RgbaImage) -> bool {
    // Solid / near-solid frames from failed CGWindow captures (common black full-screen).
    let mut samples = 0u32;
    let mut matching = 0u32;
    let first = image.get_pixel(0, 0).0;
    let step_x = (image.width() / 16).max(1);
    let step_y = (image.height() / 16).max(1);
    for y in (0..image.height()).step_by(step_y as usize) {
        for x in (0..image.width()).step_by(step_x as usize) {
            samples += 1;
            let pixel = image.get_pixel(x, y).0;
            let close = pixel
                .iter()
                .zip(first.iter())
                .all(|(a, b)| a.abs_diff(*b) <= 2);
            if close {
                matching += 1;
            }
        }
    }
    samples > 0 && matching * 100 / samples >= 98
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn crop_safety_requires_a_target_and_respects_stack_edges_and_app_identity() {
        let selected = WindowDescriptor {
            id: "target".into(),
            title: "Document".into(),
            app_name: Some(" Editor ".into()),
            z_order: 5,
            x: -200,
            y: 30,
            width: 100,
            height: 80,
            display_id: "left".into(),
            corner_radius: None,
        };
        assert!(!window_display_crop_is_safe(&selected, &[]));
        let mut other = WindowDescriptor {
            id: "other".into(),
            app_name: Some("Browser".into()),
            z_order: 6,
            x: -100,
            ..selected.clone()
        };
        let safe = |other: &WindowDescriptor| {
            window_display_crop_is_safe(&selected, &[selected.clone(), other.clone()])
        };
        assert!(safe(&other), "touching edges do not overlap");
        other.x -= 1;
        assert!(
            !safe(&other),
            "one pixel of a higher foreign window is unsafe"
        );
        other.z_order = 4;
        assert!(safe(&other), "a lower window cannot cover the target");
        other.z_order = 6;
        other.display_id = "right".into();
        assert!(safe(&other));
        other.display_id = "left".into();
        other.title = "  ".into();
        other.app_name = Some("editor".into());
        assert!(safe(&other), "known same-app untitled transient is allowed");
        other.title = "Other document".into();
        assert!(!safe(&other), "same app does not imply same document");
        other.title.clear();
        other.app_name = None;
        assert!(
            !safe(&other),
            "unknown ownership is not a same-app transient"
        );
    }

    #[test]
    fn unsafe_source_is_never_read_even_when_native_capture_fails() {
        let error = resolve_window_capture(
            false,
            || panic!("must not read pixels from an occluding window"),
            || Err(CaptureError::TargetUnavailable),
        )
        .unwrap_err();
        assert!(matches!(error, CaptureError::WindowOccluded));
        let error = resolve_window_capture(true, || None, || Err(CaptureError::TargetUnavailable))
            .unwrap_err();
        assert!(matches!(error, CaptureError::TargetUnavailable));
        let error = resolve_window_capture(
            true,
            || None,
            || Ok(RgbaImage::from_pixel(10, 10, Rgba([8, 17, 23, 255]))),
        )
        .unwrap_err();
        assert!(matches!(error, CaptureError::WindowEmpty));
    }

    #[test]
    fn blank_frame_heuristic_keeps_shipping_channel_and_sample_thresholds() {
        let mut image = RgbaImage::from_pixel(10, 10, Rgba([10, 20, 30, 255]));
        for pixel in image.pixels_mut().skip(1) {
            *pixel = Rgba([12, 18, 31, 253]);
        }
        assert!(image_is_effectively_blank(&image), "channel delta <= 2");
        image.put_pixel(3, 1, Rgba([13, 20, 30, 255]));
        image.put_pixel(4, 2, Rgba([10, 20, 30, 252]));
        assert!(
            image_is_effectively_blank(&image),
            "98 of 100 samples match"
        );
        image.put_pixel(7, 6, Rgba([10, 24, 30, 255]));
        assert!(
            !image_is_effectively_blank(&image),
            "97 of 100 samples match"
        );
    }
}
