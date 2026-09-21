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
