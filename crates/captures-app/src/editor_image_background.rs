//! Natural-resolution image alpha edits, matching the shipping pixel tools.
//! Callers retain the original asset and publish edited pixels transactionally.

use std::collections::VecDeque;

use image::{Rgba, RgbaImage};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrushMode {
    Erase,
    Restore,
}

/// Paint a sequence of natural-image pixel points with the shipping round brush.
/// The first point is stamped once; every following segment includes both endpoints.
pub fn paint_stroke(
    working: &mut RgbaImage,
    original: Option<&RgbaImage>,
    points: &[(u32, u32)],
    radius: f64,
    hardness: f64,
    mode: BrushMode,
) -> Result<u64, String> {
    if working.width() == 0 || working.height() == 0 {
        return Err("working image dimensions must be nonzero".into());
    }
    if points.is_empty() {
        return Err("brush stroke must contain at least one point".into());
    }
    if !radius.is_finite() || radius <= 0.0 {
        return Err("brush radius must be finite and positive".into());
    }
    if !hardness.is_finite() {
        return Err("brush hardness must be finite".into());
    }
    if points
        .iter()
        .any(|&(x, y)| x >= working.width() || y >= working.height())
    {
        return Err("brush points must be within the working image".into());
    }
    let original = match mode {
        BrushMode::Erase => None,
        BrushMode::Restore => {
            let original = original.ok_or("restore brush requires an original image")?;
            if original.dimensions() != working.dimensions() {
                return Err("restore image dimensions must match the working image".into());
            }
            Some(original)
        }
    };

    let hardness = hardness.clamp(0.0, 1.0);
    let mut changed = stamp(
        working,
        original,
        (f64::from(points[0].0), f64::from(points[0].1)),
        radius,
        hardness,
        mode,
    );
    for pair in points.windows(2) {
        let from = pair[0];
        let to = pair[1];
        let dx = f64::from(to.0) - f64::from(from.0);
        let dy = f64::from(to.1) - f64::from(from.1);
        let distance = dx.hypot(dy);
        if distance < 0.001 {
            changed += stamp(
                working,
                original,
                (f64::from(to.0), f64::from(to.1)),
                radius,
                hardness,
                mode,
            );
            continue;
        }
        let steps = (distance / (radius * 0.35).max(0.5)).ceil().max(1.0) as u64;
        for index in 0..=steps {
            let t = index as f64 / steps as f64;
            changed += stamp(
                working,
                original,
                (f64::from(from.0) + dx * t, f64::from(from.1) + dy * t),
                radius,
                hardness,
                mode,
            );
        }
    }
    Ok(changed)
}

fn stamp(
    working: &mut RgbaImage,
    original: Option<&RgbaImage>,
    (center_x, center_y): (f64, f64),
    radius: f64,
    hardness: f64,
    mode: BrushMode,
) -> u64 {
    let radius = radius.max(0.5);
    let radius_squared = radius * radius;
    let soft_start = radius * hardness;
    let soft_start_squared = soft_start * soft_start;
    let min_x = (center_x - radius).floor().max(0.0) as u32;
    let max_x = (center_x + radius)
        .ceil()
        .min(f64::from(working.width() - 1)) as u32;
    let min_y = (center_y - radius).floor().max(0.0) as u32;
    let max_y = (center_y + radius)
        .ceil()
        .min(f64::from(working.height() - 1)) as u32;
    let mut changed = 0;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = f64::from(x) + 0.5 - center_x;
            let dy = f64::from(y) + 0.5 - center_y;
            let distance_squared = dx * dx + dy * dy;
            if distance_squared > radius_squared {
                continue;
            }
            let mut strength = 1.0;
            if distance_squared > soft_start_squared && radius > soft_start {
                let distance = distance_squared.sqrt();
                strength = (1.0 - (distance - soft_start) / (radius - soft_start)).clamp(0.0, 1.0);
            }
            if strength <= 0.0 {
                continue;
            }

            let pixel = working.get_pixel_mut(x, y);
            match mode {
                BrushMode::Erase => {
                    let before = pixel[3];
                    if before == 0 {
                        continue;
                    }
                    let alpha = (f64::from(before) * (1.0 - strength)).round() as u8;
                    if alpha == before {
                        continue;
                    }
                    if alpha == 0 {
                        *pixel = Rgba([0; 4]);
                    } else {
                        pixel[3] = alpha;
                    }
                }
                BrushMode::Restore => {
                    let source = original.expect("restore source validated").get_pixel(x, y);
                    let before = *pixel;
                    for channel in 0..4 {
                        pixel[channel] = (f64::from(before[channel])
                            + (f64::from(source[channel]) - f64::from(before[channel])) * strength)
                            .round() as u8;
                    }
                    if *pixel == before {
                        continue;
                    }
                }
            }
            changed += 1;
        }
    }
    changed
}

/// Clear pixels within an inclusive maximum RGB-channel distance of the seed.
/// Alpha does not affect color distance, but transparent pixels are barriers.
/// Contiguous mode uses four-connected neighbors, not diagonals. Fully cleared
/// pixels have zero RGB as well as zero alpha, matching canvas PNG output.
pub fn remove_color(
    image: &mut RgbaImage,
    start: (u32, u32),
    tolerance: u8,
    contiguous: bool,
) -> u64 {
    let (x, y) = start;
    if x >= image.width() || y >= image.height() {
        return 0;
    }
    let target = *image.get_pixel(x, y);
    if target[3] == 0 {
        return 0;
    }
    let matches = |pixel: &Rgba<u8>| {
        pixel[3] != 0 && (0..3).all(|channel| pixel[channel].abs_diff(target[channel]) <= tolerance)
    };
    if !contiguous {
        let mut changed = 0;
        for pixel in image.pixels_mut().filter(|pixel| matches(pixel)) {
            *pixel = Rgba([0; 4]);
            changed += 1;
        }
        return changed;
    }

    // Clear at enqueue time so each matching pixel is queued once. The zero
    // alpha doubles as a visited marker without a second full-frame bitmap.
    image.put_pixel(x, y, Rgba([0; 4]));
    let mut queue = VecDeque::from([start]);
    let mut changed = 1;
    while let Some((x, y)) = queue.pop_front() {
        for neighbor in [
            x.checked_sub(1).map(|x| (x, y)),
            (x + 1 < image.width()).then_some((x + 1, y)),
            y.checked_sub(1).map(|y| (x, y)),
            (y + 1 < image.height()).then_some((x, y + 1)),
        ]
        .into_iter()
        .flatten()
        {
            let pixel = image.get_pixel_mut(neighbor.0, neighbor.1);
            if matches(pixel) {
                *pixel = Rgba([0; 4]);
                queue.push_back(neighbor);
                changed += 1;
            }
        }
    }
    changed
}

/// Shipping `WAND_LOUPE_SAMPLE_EXTENT`: the odd natural-pixel neighborhood the
/// remove-background wand loupe magnifies, so a true center pixel exists.
pub const WAND_LOUPE_SAMPLE_EXTENT: u32 = 11;
/// `WAND_LOUPE_SIZE_PX`: the loupe circle's diameter in points.
pub const WAND_LOUPE_SIZE: f64 = 84.;
/// `WAND_LOUPE_OFFSET_PX`: gap from the crosshair so the loupe never covers
/// the sampled pixel.
pub const WAND_LOUPE_OFFSET: f64 = 18.;
/// `wandLoupeScreenPosition`'s viewport margin.
pub const WAND_LOUPE_MARGIN: f64 = 8.;
/// `paintWandColorLoupe`'s checkerboard (dark, light, cell) behind
/// transparent samples; shipping hard-codes these literals.
pub const WAND_LOUPE_CHECKER: ([u8; 3], [u8; 3], f64) =
    ([0xc4, 0xc4, 0xc8], [0xec, 0xec, 0xee], 6.);
/// Alpha of the black grid between magnified source pixels.
pub const WAND_LOUPE_GRID_ALPHA: f64 = 0.18;
/// `.screenshot-wand-loupe-meta`: the swatch + hex pill sits this far below
/// the circle.
pub const WAND_LOUPE_META_GAP: f64 = 6.;

/// The remove-background wand's colour loupe at one pointer position.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct WandLoupe {
    /// Natural pixel the wand keys on (the highlighted center tile).
    pub pixel: (u32, u32),
    /// Straight-alpha RGBA of that pixel.
    pub color: [u8; 4],
    /// `extent × extent` tiles row by row around `pixel`; `None` outside the
    /// image, where the checkerboard shows through.
    pub tiles: Vec<Option<[u8; 4]>>,
    pub extent: u32,
    /// `.screenshot-wand-loupe-hex`: `#rrggbb`, or `empty` when transparent.
    pub text: String,
    pub transparent: bool,
    /// The loupe's `aria-label`.
    pub accessible_label: String,
}

/// `rgbaToHex`: `#rrggbb`, ignoring alpha.
#[must_use]
pub fn rgba_hex([r, g, b, _]: [u8; 4]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The frontmost visible image under `point` and its natural pixel: the layer
/// and sample the wand click edits and the loupe magnifies. Locked images
/// count, as in the shipping wand.
#[must_use]
pub fn wand_target(
    document: &crate::editor::Document,
    point: crate::editor::Point,
) -> Option<(usize, &crate::editor::ImageElement, (u32, u32))> {
    document
        .elements
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, element)| match element {
            crate::editor::Element::Image(image) if image.base.visible => image
                .natural_pixel_at(point)
                .map(|pixel| (index, image, pixel)),
            _ => None,
        })
}

/// Shipping `WandColorLoupe` + `paintWandColorLoupe`: the magnified natural
/// pixels around the wand's sample, clamped so edge pixels still fill the
/// loupe, plus the sampled colour's label. `None` hides the loupe (the
/// pointer is off every visible image, or its asset is unavailable).
pub fn wand_loupe<'a>(
    document: &crate::editor::Document,
    asset: impl Fn(&str) -> Option<&'a RgbaImage>,
    point: crate::editor::Point,
) -> Option<WandLoupe> {
    let (_, image, pixel) = wand_target(document, point)?;
    let source = asset(&image.src)?;
    if source.width() == 0 || source.height() == 0 {
        return None;
    }
    let (x, y) = (
        pixel.0.min(source.width() - 1),
        pixel.1.min(source.height() - 1),
    );
    let extent = WAND_LOUPE_SAMPLE_EXTENT | 1;
    let half = i64::from(extent / 2);
    let mut tiles = Vec::with_capacity((extent * extent) as usize);
    for row in 0..i64::from(extent) {
        for column in 0..i64::from(extent) {
            let sx = i64::from(x) - half + column;
            let sy = i64::from(y) - half + row;
            tiles.push(
                (sx >= 0
                    && sy >= 0
                    && sx < i64::from(source.width())
                    && sy < i64::from(source.height()))
                .then(|| source.get_pixel(sx as u32, sy as u32).0),
            );
        }
    }
    let color = source.get_pixel(x, y).0;
    let transparent = color[3] == 0;
    let hex = rgba_hex(color);
    Some(WandLoupe {
        pixel: (x, y),
        color,
        tiles,
        extent,
        text: if transparent {
            "empty".into()
        } else {
            hex.clone()
        },
        transparent,
        accessible_label: if transparent {
            "Sample color: transparent".into()
        } else {
            format!("Sample color {hex}")
        },
    })
}

/// Shipping `wandLoupeScreenPosition`: the loupe's top-left beside the
/// cursor, flipped left/up near the viewport's right/bottom edges and kept
/// [`WAND_LOUPE_MARGIN`] inside it. Coordinates are y-down points.
#[must_use]
pub fn wand_loupe_position(
    cursor: crate::editor::Point,
    viewport_width: f64,
    viewport_height: f64,
) -> crate::editor::Point {
    let (size, offset, margin) = (WAND_LOUPE_SIZE, WAND_LOUPE_OFFSET, WAND_LOUPE_MARGIN);
    let mut left = cursor.x + offset;
    let mut top = cursor.y + offset;
    if left + size > viewport_width - margin {
        left = cursor.x - offset - size;
    }
    if top + size > viewport_height - margin {
        top = cursor.y - offset - size;
    }
    crate::editor::Point {
        x: left.min(viewport_width - size - margin).max(margin),
        y: top.min(viewport_height - size - margin).max(margin),
    }
}

#[cfg(test)]
mod loupe_tests {
    use super::*;
    use crate::editor::{Document, Point};

    #[test]
    fn loupe_position_flips_near_the_viewport_edges() {
        // Shipping `imageBackground.test.ts` cases.
        let open = wand_loupe_position(Point { x: 40., y: 50. }, 800., 600.);
        assert_eq!(open, Point { x: 58., y: 68. });
        let corner = wand_loupe_position(Point { x: 790., y: 590. }, 800., 600.);
        assert!(corner.x + WAND_LOUPE_SIZE <= 792. && corner.y + WAND_LOUPE_SIZE <= 592.);
        assert_eq!(corner, Point { x: 688., y: 488. });
        let tiny = wand_loupe_position(Point { x: 5., y: 5. }, 60., 60.);
        assert_eq!(tiny, Point { x: 8., y: 8. }, "never leaves the margin");
    }

    #[test]
    fn loupe_magnifies_the_wand_target_and_labels_its_colour() {
        let document = Document::new_capture("fixture:base", 20., 10., None);
        let mut pixels = RgbaImage::from_pixel(20, 10, Rgba([0x12, 0x34, 0x56, 255]));
        pixels.put_pixel(0, 0, Rgba([255, 0, 0, 0]));
        let asset = |src: &str| (src == "fixture:base").then_some(&pixels);

        let loupe = wand_loupe(&document, asset, Point { x: 3.5, y: 4.5 }).unwrap();
        assert_eq!(loupe.pixel, (3, 4));
        assert_eq!(loupe.text, "#123456");
        assert_eq!(loupe.accessible_label, "Sample color #123456");
        assert_eq!(loupe.tiles.len(), 121);
        // Column 0 of the window is x = -2: outside, so the checker shows.
        assert_eq!(loupe.tiles[0], None);
        assert_eq!(loupe.tiles[60], Some([0x12, 0x34, 0x56, 255]));

        let empty = wand_loupe(&document, asset, Point { x: 0.2, y: 0.2 }).unwrap();
        assert!(empty.transparent);
        assert_eq!(empty.text, "empty");
        assert_eq!(empty.accessible_label, "Sample color: transparent");

        assert_eq!(wand_loupe(&document, asset, Point { x: 25., y: 4. }), None);
        assert_eq!(
            wand_loupe(&document, |_| None, Point { x: 3., y: 4. }),
            None
        );
        assert_eq!(rgba_hex([1, 2, 255, 0]), "#0102ff");
    }
}
