use image::RgbaImage;

use crate::model::{DisplayDescriptor, WindowDescriptor};

#[derive(Clone, Debug)]
pub struct CursorImage {
    pub pixels: RgbaImage,
    pub logical_width: f64,
    pub logical_height: f64,
    pub hot_spot_x: f64,
    pub hot_spot_y: f64,
}

#[derive(Clone, Debug)]
pub struct PointerCursor {
    pub position: (i32, i32),
    pub image: Option<CursorImage>,
}

/// Snapshot before changing the host's cursor or opening a selector. macOS keeps
/// the system cursor pixels; Windows/X11 retain the shipping synthetic arrow.
pub fn pointer_cursor() -> Option<PointerCursor> {
    Some(PointerCursor {
        position: pointer_position()?,
        image: native_cursor_image(),
    })
}

#[cfg(not(target_os = "linux"))]
pub fn pointer_position() -> Option<(i32, i32)> {
    use mouse_position::mouse_position::Mouse;
    match Mouse::get_mouse_position() {
        Mouse::Position { x, y } => Some((x, y)),
        Mouse::Error => None,
    }
}

#[cfg(target_os = "linux")]
pub fn pointer_position() -> Option<(i32, i32)> {
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};
    // A Wayland-only or disconnected X11 session must not enter mouse_position's
    // unchecked XOpenDisplay path. Coordinates are physical X11 root pixels.
    std::env::var_os("DISPLAY")?;
    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let pointer = connection.query_pointer(root).ok()?.reply().ok()?;
    pointer
        .same_screen
        .then_some((i32::from(pointer.root_x), i32::from(pointer.root_y)))
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn native_cursor_image() -> Option<CursorImage> {
    // currentCursor describes only this app. The system-wide public accessor
    // preserves another application's cursor and hotspot for still captures.
    let cursor = objc2_app_kit::NSCursor::currentSystemCursor()?;
    let image = cursor.image();
    let size = image.size();
    let hot_spot = cursor.hotSpot();
    let tiff = image.TIFFRepresentation()?.to_vec();
    if tiff.is_empty() || size.width <= 0.0 || size.height <= 0.0 {
        return None;
    }
    Some(CursorImage {
        pixels: image::load_from_memory(&tiff).ok()?.to_rgba8(),
        logical_width: size.width,
        logical_height: size.height,
        hot_spot_x: hot_spot.x,
        hot_spot_y: hot_spot.y,
    })
}

#[cfg(not(target_os = "macos"))]
const fn native_cursor_image() -> Option<CursorImage> {
    None
}

const CURSOR_OUTLINE: [(i32, i32); 7] = [
    (0, 0),
    (0, 22),
    (6, 16),
    (11, 27),
    (16, 25),
    (11, 15),
    (21, 15),
];
const CURSOR_FILL: [(i32, i32); 7] = [
    (2, 4),
    (2, 18),
    (6, 13),
    (12, 23),
    (13, 22),
    (8, 12),
    (17, 12),
];

/// Scale that maps a platform pointer sample into display descriptor units.
///
/// Windows `GetCursorPos` and display geometry are physical. Linux X11 pointer
/// samples are physical while xcap reports logical monitor size. macOS
/// `CGEvent` locations already match the logical display origin.
#[must_use]
pub const fn screenshot_pointer_scale(display_scale_factor: f64) -> f64 {
    if cfg!(target_os = "linux") {
        display_scale_factor.max(1.0)
    } else {
        1.0
    }
}

/// Draw a captured pointer onto a full-display screenshot when its hotspot lands on it.
pub fn overlay_pointer_cursor(
    image: &mut RgbaImage,
    display: &DisplayDescriptor,
    cursor: &PointerCursor,
    pointer_scale: f64,
) {
    overlay_pointer_cursor_in_crop(
        image,
        display,
        0,
        0,
        image.width(),
        image.height(),
        cursor,
        pointer_scale,
    );
}

/// Draw a captured pointer onto a cropped screenshot when the hotspot is inside the crop.
///
/// `source_width` / `source_height` are the full display buffer that `crop_x` /
/// `crop_y` were taken from. The hotspot is tested against that crop, so a
/// pointer just outside the selected region does not leave a clipped arrow.
#[allow(clippy::too_many_arguments)]
pub fn overlay_pointer_cursor_in_crop(
    image: &mut RgbaImage,
    display: &DisplayDescriptor,
    crop_x: u32,
    crop_y: u32,
    source_width: u32,
    source_height: u32,
    cursor: &PointerCursor,
    pointer_scale: f64,
) {
    let Some((x, y)) = map_pointer_to_buffer(
        display.x,
        display.y,
        display.width,
        display.height,
        source_width,
        source_height,
        cursor.position,
        pointer_scale,
    ) else {
        return;
    };
    let Ok(crop_x) = i32::try_from(crop_x) else {
        return;
    };
    let Ok(crop_y) = i32::try_from(crop_y) else {
        return;
    };
    let local_x = x - crop_x;
    let local_y = y - crop_y;
    let Ok(image_width) = i32::try_from(image.width()) else {
        return;
    };
    let Ok(image_height) = i32::try_from(image.height()) else {
        return;
    };
    if local_x < 0 || local_y < 0 || local_x >= image_width || local_y >= image_height {
        return;
    }
    draw_cursor(
        image,
        (local_x, local_y),
        cursor.image.as_ref(),
        f64::from(source_width) / f64::from(display.width.max(1)),
        f64::from(source_height) / f64::from(display.height.max(1)),
        source_height,
    );
}

/// Draw a captured pointer onto a window screenshot when its hotspot lands on it.
pub fn overlay_pointer_cursor_on_window(
    image: &mut RgbaImage,
    window: &WindowDescriptor,
    cursor: &PointerCursor,
    pointer_scale: f64,
) {
    let Some(position) = map_pointer_to_buffer(
        window.x,
        window.y,
        window.width,
        window.height,
        image.width(),
        image.height(),
        cursor.position,
        pointer_scale,
    ) else {
        return;
    };
    draw_cursor(
        image,
        position,
        cursor.image.as_ref(),
        f64::from(image.width()) / f64::from(window.width.max(1)),
        f64::from(image.height()) / f64::from(window.height.max(1)),
        image.height(),
    );
}

#[allow(clippy::cast_possible_truncation, clippy::too_many_arguments)]
fn map_pointer_to_buffer(
    origin_x: i32,
    origin_y: i32,
    source_width: u32,
    source_height: u32,
    buffer_width: u32,
    buffer_height: u32,
    pointer: (i32, i32),
    pointer_scale: f64,
) -> Option<(i32, i32)> {
    let scale = pointer_scale.max(1.0);
    let x = f64::from(pointer.0) / scale - f64::from(origin_x);
    let y = f64::from(pointer.1) / scale - f64::from(origin_y);
    if x < 0.0 || y < 0.0 || x >= f64::from(source_width) || y >= f64::from(source_height) {
        return None;
    }
    Some((
        (x * f64::from(buffer_width) / f64::from(source_width.max(1))).round() as i32,
        (y * f64::from(buffer_height) / f64::from(source_height.max(1))).round() as i32,
    ))
}

#[allow(clippy::cast_possible_truncation)]
fn draw_cursor(
    image: &mut RgbaImage,
    position: (i32, i32),
    cursor: Option<&CursorImage>,
    scale_x: f64,
    scale_y: f64,
    source_height: u32,
) {
    if let Some(cursor) = cursor {
        draw_native_cursor(image, position, cursor, scale_x, scale_y);
        return;
    }

    let scale = (f64::from(source_height) / 1_080.0).round().clamp(1.0, 2.0) as i32;
    let (outline, fill) = cursor_colors(cfg!(target_os = "macos"));
    draw_polygon(image, position, &CURSOR_OUTLINE, scale, outline);
    draw_polygon(image, position, &CURSOR_FILL, scale, fill);
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn draw_native_cursor(
    image: &mut RgbaImage,
    position: (i32, i32),
    cursor: &CursorImage,
    scale_x: f64,
    scale_y: f64,
) {
    let width = (cursor.logical_width * scale_x).round().max(1.0) as u32;
    let height = (cursor.logical_height * scale_y).round().max(1.0) as u32;
    let pixels = if cursor.pixels.width() == width && cursor.pixels.height() == height {
        cursor.pixels.clone()
    } else {
        image::imageops::resize(
            &cursor.pixels,
            width,
            height,
            image::imageops::FilterType::Lanczos3,
        )
    };
    let x = position.0 - (cursor.hot_spot_x * scale_x).round() as i32;
    let y = position.1 - (cursor.hot_spot_y * scale_y).round() as i32;
    image::imageops::overlay(image, &pixels, i64::from(x), i64::from(y));
}

const fn cursor_colors(macos: bool) -> ([u8; 3], [u8; 3]) {
    if macos {
        // Match the standard macOS pointer. The inverse treatment disappears
        // against light content when a region becomes undimmed.
        ([248, 248, 248], [24, 24, 24])
    } else {
        ([24, 24, 24], [248, 248, 248])
    }
}

fn draw_polygon(
    image: &mut RgbaImage,
    origin: (i32, i32),
    polygon: &[(i32, i32)],
    scale: i32,
    color: [u8; 3],
) {
    let scaled = polygon
        .iter()
        .map(|(x, y)| (x * scale, y * scale))
        .collect::<Vec<_>>();
    let max_x = scaled.iter().map(|(x, _)| *x).max().unwrap_or_default();
    let max_y = scaled.iter().map(|(_, y)| *y).max().unwrap_or_default();
    for y in 0..=max_y {
        for x in 0..=max_x {
            if point_in_polygon(x, y, &scaled) {
                put_pixel(image, origin.0 + x, origin.1 + y, color);
            }
        }
    }
}

fn point_in_polygon(x: i32, y: i32, polygon: &[(i32, i32)]) -> bool {
    let mut inside = false;
    let mut previous = polygon.last().copied().unwrap_or_default();
    for &current in polygon {
        let crosses = (current.1 > y) != (previous.1 > y)
            && f64::from(x)
                < f64::from(previous.0 - current.0) * f64::from(y - current.1)
                    / f64::from(previous.1 - current.1)
                    + f64::from(current.0);
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn put_pixel(image: &mut RgbaImage, x: i32, y: i32, color: [u8; 3]) {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return;
    };
    if x >= image.width() || y >= image.height() {
        return;
    }
    image.put_pixel(x, y, image::Rgba([color[0], color[1], color[2], 255]));
}

#[cfg(test)]
mod tests {
    use super::{
        CursorImage, PointerCursor, cursor_colors, map_pointer_to_buffer, overlay_pointer_cursor,
        overlay_pointer_cursor_in_crop, overlay_pointer_cursor_on_window, screenshot_pointer_scale,
    };
    use crate::model::{DisplayDescriptor, WindowDescriptor};

    #[cfg(target_os = "linux")]
    #[test]
    fn unavailable_x11_pointer_is_none_without_crashing() {
        if std::env::var_os("CAPTURES_POINTER_CHILD").is_some() {
            assert!(super::pointer_position().is_none());
            assert!(super::pointer_cursor().is_none());
            return;
        }
        // Subprocesses avoid mutating the environment of parallel Rust tests.
        for display in [None, Some(":4999")] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "cursor::tests::unavailable_x11_pointer_is_none_without_crashing",
                ])
                .env("CAPTURES_POINTER_CHILD", "1")
                .env_remove("DISPLAY");
            if let Some(display) = display {
                child.env("DISPLAY", display);
            }
            assert!(child.status().unwrap().success());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires X11 with pointer positioned at physical (423, 317)"]
    fn live_x11_pointer_preserves_physical_coordinates() {
        assert_eq!(super::pointer_position(), Some((423, 317)));
        let cursor = super::pointer_cursor().unwrap();
        assert_eq!(cursor.position, (423, 317));
        assert!(
            cursor.image.is_none(),
            "X11 retains the shipping synthetic arrow"
        );
    }

    fn display(x: i32, y: i32, width: u32, height: u32, scale_factor: f64) -> DisplayDescriptor {
        DisplayDescriptor {
            id: "1".into(),
            name: "Test".into(),
            x,
            y,
            width,
            height,
            scale_factor,
            is_primary: true,
        }
    }

    fn cursor(position: (i32, i32)) -> PointerCursor {
        PointerCursor {
            position,
            image: None,
        }
    }

    #[test]
    fn linux_physical_pointer_maps_onto_logical_display_buffer() {
        assert_eq!(
            map_pointer_to_buffer(100, 50, 800, 450, 1_600, 900, (800, 500), 2.0),
            Some((600, 400))
        );
    }

    #[test]
    fn windows_physical_pointer_stays_aligned_with_physical_display() {
        assert_eq!(
            map_pointer_to_buffer(100, 50, 3_840, 2_160, 1_920, 1_080, (900, 550), 1.0),
            Some((400, 250))
        );
    }

    #[test]
    fn macos_logical_pointer_scales_onto_retina_buffer() {
        assert_eq!(
            map_pointer_to_buffer(0, 0, 1_512, 982, 3_024, 1_964, (400, 200), 1.0),
            Some((800, 400))
        );
    }

    #[test]
    fn ignores_a_pointer_outside_the_source() {
        assert_eq!(
            map_pointer_to_buffer(0, 0, 800, 600, 800, 600, (-10, 20), 1.0),
            None
        );
    }

    #[test]
    fn paints_the_cursor_hotspot_on_a_display_capture() {
        let display = display(0, 0, 80, 60, 1.0);
        let mut image = image::RgbaImage::from_pixel(80, 60, image::Rgba([0, 0, 0, 255]));
        overlay_pointer_cursor(&mut image, &display, &cursor((10, 12)), 1.0);
        assert!(image.pixels().any(|pixel| pixel.0 == [24, 24, 24, 255]));
        assert!(image.pixels().any(|pixel| pixel.0 == [248, 248, 248, 255]));
    }

    #[test]
    fn macos_cursor_has_a_dark_fill_with_a_light_outline() {
        assert_eq!(cursor_colors(true), ([248, 248, 248], [24, 24, 24]));
        assert_eq!(cursor_colors(false), ([24, 24, 24], [248, 248, 248]));
    }

    #[test]
    fn paints_the_cursor_hotspot_on_a_window_capture() {
        let window = WindowDescriptor {
            id: "w".into(),
            title: "Window".into(),
            app_name: None,
            z_order: 1,
            x: 40,
            y: 30,
            width: 80,
            height: 60,
            display_id: "1".into(),
            corner_radius: None,
        };
        let mut image = image::RgbaImage::from_pixel(80, 60, image::Rgba([0, 0, 0, 255]));
        overlay_pointer_cursor_on_window(&mut image, &window, &cursor((50, 42)), 1.0);
        assert!(image.pixels().any(|pixel| pixel.0 == [24, 24, 24, 255]));
        assert!(image.pixels().any(|pixel| pixel.0 == [248, 248, 248, 255]));
    }

    #[test]
    fn linux_pointer_scale_uses_the_display_factor() {
        if cfg!(target_os = "linux") {
            assert!((screenshot_pointer_scale(2.0) - 2.0).abs() < f64::EPSILON);
        } else {
            assert!((screenshot_pointer_scale(2.0) - 1.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn paints_the_cursor_when_the_hotspot_is_inside_a_crop() {
        let display = display(0, 0, 80, 60, 1.0);
        let mut image = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        overlay_pointer_cursor_in_crop(
            &mut image,
            &display,
            10,
            10,
            80,
            60,
            &cursor((14, 16)),
            1.0,
        );
        assert!(image.pixels().any(|pixel| pixel.0 == [24, 24, 24, 255]));
    }

    #[test]
    fn skips_the_cursor_when_the_hotspot_is_outside_the_crop() {
        let display = display(0, 0, 80, 60, 1.0);
        let mut image = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        overlay_pointer_cursor_in_crop(&mut image, &display, 10, 10, 80, 60, &cursor((8, 16)), 1.0);
        assert!(image.pixels().all(|pixel| pixel.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn composites_native_cursor_pixels_relative_to_the_hotspot() {
        let display = display(0, 0, 20, 20, 1.0);
        let mut image = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        let cursor = PointerCursor {
            position: (10, 10),
            image: Some(CursorImage {
                pixels: image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 128])),
                logical_width: 4.0,
                logical_height: 4.0,
                hot_spot_x: 1.0,
                hot_spot_y: 2.0,
            }),
        };

        overlay_pointer_cursor(&mut image, &display, &cursor, 1.0);

        assert_eq!(image.get_pixel(8, 8).0, [0, 0, 0, 255]);
        assert_eq!(&image.get_pixel(9, 8).0[..3], &[128, 0, 0]);
        assert_eq!(&image.get_pixel(12, 11).0[..3], &[128, 0, 0]);
        assert_eq!(image.get_pixel(13, 12).0, [0, 0, 0, 255]);
    }
}
