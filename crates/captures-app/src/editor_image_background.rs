//! Natural-resolution image alpha edits, matching the shipping pixel tools.
//! Callers retain the original asset and publish edited pixels transactionally.

use std::collections::VecDeque;

use image::{Rgba, RgbaImage};

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
