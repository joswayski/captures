//! Window source, geometry and chrome policy shared by the shipping and native hosts.
//! Hosts own permission/session checks, snapshot acquisition, cropping and cursor
//! composition. Never invoke a composited crop unless its window stack is safe.
use image::RgbaImage;

use crate::{
    CaptureError, CaptureResult, DisplayDescriptor, LogicalRect, PhysicalRect, WindowDescriptor,
};

const WINDOW_CORNER_MASK_SAMPLES_PER_AXIS: u32 = 4;
pub const RECORDING_REGION_INDICATOR_TITLE: &str = "Captures Recording Region";

/// Standard macOS window-corner fallback in points. Hosts supply the OS version;
/// snapshot-derived radii still take precedence when available.
pub fn macos_window_corner_radius_for_major_version(major_version: i64) -> f64 {
    if major_version >= 26 { 25.0 } else { 10.0 }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowPickRole {
    Capturable,
    ShellChrome,
}

#[derive(Clone, Debug, Default)]
pub struct WindowSelectionTargets {
    pub windows: Vec<WindowDescriptor>,
    pub shell_chrome: Vec<WindowDescriptor>,
}

pub fn classify_windows_for_display(
    windows: Vec<WindowDescriptor>,
    display: &DisplayDescriptor,
) -> WindowSelectionTargets {
    let mut targets = WindowSelectionTargets::default();
    for window in windows {
        match window_pick_role(&window, display) {
            Some(WindowPickRole::Capturable) => targets.windows.push(window),
            Some(WindowPickRole::ShellChrome) => targets.shell_chrome.push(window),
            None => {}
        }
    }
    targets
}

pub fn window_pick_role(
    window: &WindowDescriptor,
    display: &DisplayDescriptor,
) -> Option<WindowPickRole> {
    if window.display_id != display.id {
        return None;
    }
    if window.width == 0 || window.height == 0 {
        return None;
    }
    if captures_window_is_internal(window) {
        return None;
    }
    #[cfg(target_os = "macos")]
    if macos_window_is_capture_overlay(window) {
        return None;
    }
    #[cfg(target_os = "windows")]
    if windows_window_is_capture_overlay(window) {
        return None;
    }
    if window_is_screen_edge_chrome(window, display) {
        return Some(WindowPickRole::ShellChrome);
    }
    if window_is_desktop_backdrop(window, display) {
        return None;
    }
    const EXCLUDED_APPS: &[&str] = &[
        "Dock",
        "Control Center",
        "Notification Centre",
        "Notification Center",
        "SystemUIServer",
        "Window Server",
        "Spotlight",
        "Wallpaper",
        "loginwindow",
    ];
    if window.app_name.as_deref().is_some_and(|name| {
        EXCLUDED_APPS
            .iter()
            .any(|excluded| name.eq_ignore_ascii_case(excluded))
    }) {
        return None;
    }
    if window.width < 48 || window.height < 48 {
        return None;
    }
    Some(WindowPickRole::Capturable)
}

pub fn window_is_capturable(window: &WindowDescriptor, display: &DisplayDescriptor) -> bool {
    matches!(
        window_pick_role(window, display),
        Some(WindowPickRole::Capturable)
    )
}

fn window_overlap_area(window: &WindowDescriptor, display: &DisplayDescriptor) -> u64 {
    let left = i64::from(window.x).max(i64::from(display.x));
    let top = i64::from(window.y).max(i64::from(display.y));
    let right = (i64::from(window.x) + i64::from(window.width))
        .min(i64::from(display.x) + i64::from(display.width));
    let bottom = (i64::from(window.y) + i64::from(window.height))
        .min(i64::from(display.y) + i64::from(display.height));
    let width = (right - left).max(0);
    let height = (bottom - top).max(0);
    u64::try_from(width * height).unwrap_or(0)
}

fn window_covers_display(window: &WindowDescriptor, display: &DisplayDescriptor) -> bool {
    let display_area = u64::from(display.width) * u64::from(display.height);
    if display_area == 0 {
        return false;
    }
    window_overlap_area(window, display) * 100 >= display_area * 95
}

/// Menu bar, taskbar, and dock/panel strips that span a display edge.
fn window_is_screen_edge_chrome(window: &WindowDescriptor, display: &DisplayDescriptor) -> bool {
    const MAX_THICKNESS: i32 = 96;
    let display_right = i64::from(display.x) + i64::from(display.width);
    let display_bottom = i64::from(display.y) + i64::from(display.height);
    let window_left = i64::from(window.x);
    let window_top = i64::from(window.y);
    let window_right = window_left + i64::from(window.width);
    let window_bottom = window_top + i64::from(window.height);
    let spans_width = window_left <= i64::from(display.x) + 8
        && window_right >= display_right - 8
        && i32::try_from(window.width).unwrap_or(i32::MAX)
            >= display.width.saturating_sub(16) as i32;
    let spans_height = window_top <= i64::from(display.y) + 8
        && window_bottom >= display_bottom - 8
        && i32::try_from(window.height).unwrap_or(i32::MAX)
            >= display.height.saturating_sub(16) as i32;
    let thickness_h = i32::try_from(window.height).unwrap_or(i32::MAX);
    let thickness_w = i32::try_from(window.width).unwrap_or(i32::MAX);
    let top_bar =
        spans_width && thickness_h <= MAX_THICKNESS && window_top <= i64::from(display.y) + 8;
    let bottom_bar =
        spans_width && thickness_h <= MAX_THICKNESS && window_bottom >= display_bottom - 8;
    let left_bar =
        spans_height && thickness_w <= MAX_THICKNESS && window_left <= i64::from(display.x) + 8;
    let right_bar =
        spans_height && thickness_w <= MAX_THICKNESS && window_right >= display_right - 8;
    top_bar || bottom_bar || left_bar || right_bar
}

/// Wallpaper / desktop windows that fill the display and steal hits under the
/// menu bar or taskbar. Named document windows from the same apps stay selectable.
fn window_is_desktop_backdrop(window: &WindowDescriptor, display: &DisplayDescriptor) -> bool {
    if !window_covers_display(window, display) {
        return false;
    }
    let title = window.title.trim();
    if title.eq_ignore_ascii_case("Desktop") || title.eq_ignore_ascii_case("Program Manager") {
        return true;
    }
    let Some(app) = window.app_name.as_deref().map(str::trim) else {
        return false;
    };
    const BACKDROP_APPS: &[&str] = &[
        "Finder",
        "explorer",
        "explorer.exe",
        "Progman",
        "WorkerW",
        "Nautilus",
        "nemo",
        "caja",
        "pcmanfm",
        "pcmanfm-qt",
        "dolphin",
        "plasmashell",
        "gnome-shell",
    ];
    if !BACKDROP_APPS
        .iter()
        .any(|excluded| app.eq_ignore_ascii_case(excluded))
    {
        return false;
    }
    title.is_empty() || title.eq_ignore_ascii_case("Desktop")
}

#[cfg(target_os = "macos")]
pub fn macos_window_is_capture_overlay(window: &WindowDescriptor) -> bool {
    window.app_name.as_deref().is_some_and(|name| {
        let name = name.trim();
        name.eq_ignore_ascii_case("Screenshot") || name.eq_ignore_ascii_case("screencaptureui")
    })
}

fn captures_window_is_internal(window: &WindowDescriptor) -> bool {
    let captures_owned = window.app_name.as_deref().is_some_and(|name| {
        let name = name.trim();
        name.eq_ignore_ascii_case("Captures")
            || name.eq_ignore_ascii_case("Captures.app")
            || name.eq_ignore_ascii_case("captures.exe")
    });
    if !captures_owned {
        return false;
    }

    const INTERNAL_WINDOW_TITLES: &[&str] = &[
        "Captures",
        "Captures is running",
        "Captures Recording Controls",
        "Captures Recording Countdown",
        RECORDING_REGION_INDICATOR_TITLE,
        "Captures Update",
        "Recording saved",
    ];
    let title = window.title.trim();
    INTERNAL_WINDOW_TITLES
        .iter()
        .any(|internal| title.eq_ignore_ascii_case(internal))
}

pub fn windows_window_is_capture_overlay(window: &WindowDescriptor) -> bool {
    window
        .app_name
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case("NVIDIA App"))
        && window
            .title
            .to_ascii_lowercase()
            .starts_with("nvidia geforce overlay")
}

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

/// Map native window/display geometry onto the capture buffer.
///
/// Coordinates come from the capture backend in the same units as
/// `display.width`/`height` (logical points on macOS, physical pixels on
/// Windows). Region selections from the overlay use
/// [`DisplayDescriptor::overlay_to_buffer_scale`] instead.
pub fn capture_buffer_scale(display: &DisplayDescriptor, image: &RgbaImage) -> f64 {
    let logical_w = f64::from(display.width.max(1));
    let logical_h = f64::from(display.height.max(1));
    let scale_x = f64::from(image.width()) / logical_w;
    let scale_y = f64::from(image.height()) / logical_h;
    let derived = ((scale_x + scale_y) * 0.5).max(1.0);
    // If the platform scale disagrees badly, trust the buffer dimensions.
    if (derived - display.scale_factor.max(1.0)).abs() > 0.25 {
        return derived;
    }
    display.scale_factor.max(1.0).max(derived)
}

pub fn window_physical_rect(
    display: &DisplayDescriptor,
    image: &RgbaImage,
    window: &WindowDescriptor,
) -> Option<PhysicalRect> {
    let scale = capture_buffer_scale(display, image);
    let rect = LogicalRect {
        x: f64::from(window.x - display.x),
        y: f64::from(window.y - display.y),
        width: f64::from(window.width),
        height: f64::from(window.height),
    };
    let physical = rect.to_physical(scale, image.width(), image.height());
    if physical.width == 0 || physical.height == 0 {
        None
    } else {
        Some(physical)
    }
}

/// Measure each window's visible corner radius from the freeze-frame so the
/// selector ring, dim cutout, and PNG mask share one shape.
///
/// A single OS-default radius is wrong for panels, terminals, and other apps
/// that keep tighter chrome than the current system window style. Sampling the
/// already-captured display image avoids a second per-window capture pass.
pub fn refine_window_chrome_from_snapshot(
    windows: &mut [WindowDescriptor],
    display: &DisplayDescriptor,
    image: &RgbaImage,
    fallback_radius: f64,
) {
    let scale = capture_buffer_scale(display, image);
    for window in windows.iter_mut() {
        if let Some(radius) = estimate_window_corner_radius_from_snapshot(
            window,
            display,
            image,
            scale,
            fallback_radius,
        ) {
            window.corner_radius = Some(radius);
        }
    }
}

fn estimate_window_corner_radius_from_snapshot(
    window: &WindowDescriptor,
    display: &DisplayDescriptor,
    image: &RgbaImage,
    scale: f64,
    fallback_radius: f64,
) -> Option<f64> {
    let scale = scale.max(1.0);
    let left = ((f64::from(window.x - display.x) * scale).round() as i64).max(0);
    let top = ((f64::from(window.y - display.y) * scale).round() as i64).max(0);
    let width = ((f64::from(window.width) * scale).round() as i64).max(1);
    let height = ((f64::from(window.height) * scale).round() as i64).max(1);
    let right = left + width;
    let bottom = top + height;
    if right > i64::from(image.width()) || bottom > i64::from(image.height()) {
        return None;
    }

    // Fullscreen-ish targets keep square display edges.
    if window.x <= display.x
        && window.y <= display.y
        && window.x + window.width as i32 >= display.x + display.width as i32
        && window.y + window.height as i32 >= display.y + display.height as i32
    {
        return Some(0.0);
    }

    let max_radius_px = ((fallback_radius * scale)
        .min(width as f64 / 2.0)
        .min(height as f64 / 2.0)
        .floor() as i64)
        .max(0);
    if max_radius_px < 2 {
        return Some(0.0);
    }

    let mut samples = Vec::with_capacity(4);
    for (corner_x, corner_y, dir_x, dir_y) in [
        (left, top, 1_i64, 1_i64),
        (right - 1, top, -1, 1),
        (left, bottom - 1, 1, -1),
        (right - 1, bottom - 1, -1, -1),
    ] {
        if let Some(radius_px) = estimate_corner_radius_px(
            image,
            corner_x,
            corner_y,
            dir_x,
            dir_y,
            max_radius_px,
            width,
            height,
        ) {
            samples.push(radius_px);
        }
    }
    if samples.is_empty() {
        return None;
    }
    // Inclusive pixel bounds make the trailing edge of a corner one pixel short
    // of the true radius. Prefer the strongest readable corner instead of the
    // median, which systematically under-reads rounded chrome.
    let best_px = *samples.iter().max().unwrap_or(&0) as f64;
    let radius_points = (best_px / scale).clamp(0.0, fallback_radius.max(0.0));
    // Prefer half-point steps so CSS border-radius stays stable on Retina.
    Some((radius_points * 2.0).round() / 2.0)
}

#[allow(clippy::too_many_arguments)]
fn estimate_corner_radius_px(
    image: &RgbaImage,
    corner_x: i64,
    corner_y: i64,
    dir_x: i64,
    dir_y: i64,
    max_radius_px: i64,
    window_width_px: i64,
    window_height_px: i64,
) -> Option<i64> {
    let outside = sample_image(image, corner_x, corner_y)?;
    // Deep interior of this corner — should land on window chrome/content.
    let inset = (max_radius_px.max(8) + 4)
        .min(window_width_px / 3)
        .min(window_height_px / 3);
    if inset < 4 {
        return None;
    }
    let inside = sample_image(image, corner_x + dir_x * inset, corner_y + dir_y * inset)?;
    // If the corner already looks like the interior, this corner is square or
    // the freeze-frame has no readable edge (e.g. same-colored neighbor).
    if pixels_similar(outside, inside, 18) {
        return Some(0);
    }

    let mut along_x = 0_i64;
    while along_x < max_radius_px {
        let x = corner_x + dir_x * along_x;
        let Some(pixel) = sample_image(image, x, corner_y) else {
            break;
        };
        if !pixels_similar(pixel, outside, 18) {
            break;
        }
        along_x += 1;
    }

    let mut along_y = 0_i64;
    while along_y < max_radius_px {
        let y = corner_y + dir_y * along_y;
        let Some(pixel) = sample_image(image, corner_x, y) else {
            break;
        };
        if !pixels_similar(pixel, outside, 18) {
            break;
        }
        along_y += 1;
    }

    // At an inclusive trailing edge the arc is one pixel short of R, so the two
    // runs can disagree. Keep the longer readable edge for this corner.
    let radius = along_x.max(along_y).clamp(0, max_radius_px);
    // Tiny runs are usually anti-alias or 1px framing, not real window chrome.
    if radius <= 1 {
        return Some(0);
    }
    Some(radius)
}

fn sample_image(image: &RgbaImage, x: i64, y: i64) -> Option<[u8; 4]> {
    if x < 0 || y < 0 {
        return None;
    }
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    if x >= image.width() || y >= image.height() {
        return None;
    }
    Some(image.get_pixel(x, y).0)
}

fn pixels_similar(left: [u8; 4], right: [u8; 4], max_channel_delta: u8) -> bool {
    left.iter()
        .zip(right.iter())
        .all(|(a, b)| a.abs_diff(*b) <= max_channel_delta)
}

/// Apply the shipping macOS window-corner alpha mask to a display crop.
///
/// Hosts decide whether this macOS-specific policy applies; this geometry
/// helper performs no platform detection or fallback-radius discovery.
pub fn mask_macos_window_corners(
    image: &mut RgbaImage,
    window: &WindowDescriptor,
    display: &DisplayDescriptor,
    scale: f64,
    corner_radius_points: f64,
) {
    let window_left = i64::from(window.x);
    let window_top = i64::from(window.y);
    let window_right = window_left + i64::from(window.width);
    let window_bottom = window_top + i64::from(window.height);
    let display_left = i64::from(display.x);
    let display_top = i64::from(display.y);
    let display_right = display_left + i64::from(display.width);
    let display_bottom = display_top + i64::from(display.height);

    // A fullscreen window has square display edges. A larger, clipped window
    // also has no visible window corners within this display crop.
    if window_left <= display_left
        && window_top <= display_top
        && window_right >= display_right
        && window_bottom >= display_bottom
    {
        return;
    }

    let scale = scale.max(1.0);
    let full_width = f64::from(window.width) * scale;
    let full_height = f64::from(window.height) * scale;
    let radius = (corner_radius_points * scale)
        .min(full_width / 2.0)
        .min(full_height / 2.0);
    if radius <= 0.0 {
        return;
    }

    // Crops are clipped to the selected display. Keep coordinates relative to
    // the full window so a partially offscreen rounded corner is masked only
    // where that corner is still visible.
    let crop_offset_x = ((display_left - window_left).max(0) as f64) * scale;
    let crop_offset_y = ((display_top - window_top).max(0) as f64) * scale;
    let samples = WINDOW_CORNER_MASK_SAMPLES_PER_AXIS;
    let sample_count = samples * samples;

    for y in 0..image.height() {
        let window_y = crop_offset_y + f64::from(y);
        let near_vertical_corner = window_y < radius || window_y + 1.0 > full_height - radius;
        if !near_vertical_corner {
            continue;
        }

        for x in 0..image.width() {
            let window_x = crop_offset_x + f64::from(x);
            let near_horizontal_corner = window_x < radius || window_x + 1.0 > full_width - radius;
            if !near_horizontal_corner {
                continue;
            }

            let mut inside_samples = 0;
            for sample_y in 0..samples {
                for sample_x in 0..samples {
                    let sample_x = window_x + (f64::from(sample_x) + 0.5) / f64::from(samples);
                    let sample_y = window_y + (f64::from(sample_y) + 0.5) / f64::from(samples);
                    let center_x = sample_x.clamp(radius, full_width - radius);
                    let center_y = sample_y.clamp(radius, full_height - radius);
                    let distance_x = sample_x - center_x;
                    let distance_y = sample_y - center_y;
                    if distance_x.mul_add(distance_x, distance_y * distance_y) <= radius * radius {
                        inside_samples += 1;
                    }
                }
            }

            let mask_alpha = u8::try_from((inside_samples * 255 + sample_count / 2) / sample_count)
                .expect("corner coverage stays within one byte");
            let pixel = image.get_pixel_mut(x, y);
            if mask_alpha == 0 {
                // Do not leave pixels from windows behind the target hidden in
                // fully transparent PNG data.
                pixel.0 = [0, 0, 0, 0];
            } else {
                pixel.0[3] = pixel.0[3].min(mask_alpha);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn macos_window_radius_changes_at_liquid_glass_boundary() {
        for (version, radius) in [(15, 10.), (25, 10.), (26, 25.), (27, 25.)] {
            assert_eq!(
                macos_window_corner_radius_for_major_version(version),
                radius
            );
        }
    }

    fn display(x: i32, y: i32, width: u32, height: u32, scale_factor: f64) -> DisplayDescriptor {
        DisplayDescriptor {
            id: "display".into(),
            name: "Display".into(),
            x,
            y,
            width,
            height,
            scale_factor,
            is_primary: true,
        }
    }

    fn window(display: &DisplayDescriptor) -> WindowDescriptor {
        WindowDescriptor {
            id: "window".into(),
            title: "Document".into(),
            app_name: Some("App".into()),
            z_order: 1,
            x: display.x + 100,
            y: display.y + 100,
            width: 640,
            height: 480,
            display_id: display.id.clone(),
            corner_radius: None,
        }
    }

    #[test]
    fn target_roles_keep_shell_strips_before_the_minimum_window_size() {
        let display = display(-1440, 0, 1440, 900, 1.0);
        let mut candidate = window(&display);
        assert_eq!(
            window_pick_role(&candidate, &display),
            Some(WindowPickRole::Capturable)
        );

        candidate.height = 47;
        assert_eq!(window_pick_role(&candidate, &display), None);

        candidate.x = display.x;
        candidate.y = display.y;
        candidate.width = display.width;
        candidate.height = 24;
        assert_eq!(
            window_pick_role(&candidate, &display),
            Some(WindowPickRole::ShellChrome),
            "thin edge chrome remains a display target despite the 48px window minimum"
        );

        candidate.display_id = "other".into();
        assert_eq!(window_pick_role(&candidate, &display), None);
        candidate.display_id = display.id.clone();
        candidate.width = 0;
        assert_eq!(window_pick_role(&candidate, &display), None);
    }

    #[test]
    fn platform_capture_overlay_exclusions_remain_platform_gated() {
        let display = display(0, 0, 1440, 900, 1.0);
        let mut overlay = window(&display);
        overlay.app_name = Some("NVIDIA App".into());
        overlay.title = "NVIDIA GeForce Overlay DT".into();

        #[cfg(target_os = "windows")]
        assert_eq!(window_pick_role(&overlay, &display), None);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            window_pick_role(&overlay, &display),
            Some(WindowPickRole::Capturable)
        );

        overlay.app_name = Some("Screenshot".into());
        overlay.title.clear();
        #[cfg(target_os = "macos")]
        assert_eq!(window_pick_role(&overlay, &display), None);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            window_pick_role(&overlay, &display),
            Some(WindowPickRole::Capturable)
        );
    }

    #[test]
    fn native_window_scale_does_not_use_overlay_coordinate_scaling() {
        let display = display(0, 0, 1920, 1080, 1.5);
        let image = RgbaImage::new(1920, 1080);

        assert!((capture_buffer_scale(&display, &image) - 1.0).abs() < f64::EPSILON);
        assert!(
            (DisplayDescriptor::overlay_to_buffer_scale_for(
                display.width,
                display.height,
                display.scale_factor,
                image.width(),
                image.height(),
                true,
            ) - 1.5)
                .abs()
                < f64::EPSILON,
            "Windows overlay DIPs require a different scale from native window pixels"
        );
    }

    #[test]
    fn window_rect_clips_against_a_negative_origin_display() {
        let display = display(-1920, -100, 1920, 1080, 1.0);
        let image = RgbaImage::new(1920, 1080);
        let window = WindowDescriptor {
            id: "window".into(),
            title: "Clipped".into(),
            app_name: Some("App".into()),
            z_order: 1,
            x: -1950,
            y: -120,
            width: 100,
            height: 80,
            display_id: display.id.clone(),
            corner_radius: None,
        };

        assert_eq!(
            window_physical_rect(&display, &image, &window),
            Some(PhysicalRect {
                x: 0,
                y: 0,
                width: 70,
                height: 60,
            })
        );
    }

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
