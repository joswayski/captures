use image::RgbaImage;

use crate::model::{DisplayDescriptor, WindowDescriptor};

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

/// Draw a pointer glyph onto a full-display screenshot when `pointer` lands on it.
pub fn overlay_pointer_cursor(
    image: &mut RgbaImage,
    display: &DisplayDescriptor,
    pointer: (i32, i32),
    pointer_scale: f64,
) {
    let Some(position) = map_pointer_to_buffer(
        display.x,
        display.y,
        display.width,
        display.height,
        image.width(),
        image.height(),
        pointer,
        pointer_scale,
    ) else {
        return;
    };
    draw_cursor(image, position);
}

/// Draw a pointer glyph onto a window screenshot when `pointer` lands on it.
pub fn overlay_pointer_cursor_on_window(
    image: &mut RgbaImage,
    window: &WindowDescriptor,
    pointer: (i32, i32),
    pointer_scale: f64,
) {
    let Some(position) = map_pointer_to_buffer(
        window.x,
        window.y,
        window.width,
        window.height,
        image.width(),
        image.height(),
        pointer,
        pointer_scale,
    ) else {
        return;
    };
    draw_cursor(image, position);
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
fn draw_cursor(image: &mut RgbaImage, position: (i32, i32)) {
    let scale = (f64::from(image.height()) / 1_080.0)
        .round()
        .clamp(1.0, 2.0) as i32;
    draw_polygon(image, position, &CURSOR_OUTLINE, scale, [24, 24, 24]);
    draw_polygon(image, position, &CURSOR_FILL, scale, [248, 248, 248]);
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
        map_pointer_to_buffer, overlay_pointer_cursor, overlay_pointer_cursor_on_window,
        screenshot_pointer_scale,
    };
    use crate::model::{DisplayDescriptor, WindowDescriptor};

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
        overlay_pointer_cursor(&mut image, &display, (10, 12), 1.0);
        assert!(image.pixels().any(|pixel| pixel.0 == [24, 24, 24, 255]));
        assert!(image.pixels().any(|pixel| pixel.0 == [248, 248, 248, 255]));
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
        overlay_pointer_cursor_on_window(&mut image, &window, (50, 42), 1.0);
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
}
