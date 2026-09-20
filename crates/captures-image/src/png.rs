//! PNG export packing and palette quantization shared by every native host.

use std::collections::HashMap;

use image::{
    RgbImage, RgbaImage,
    codecs::png::{CompressionType, FilterType},
};
use quantette::{ImageBuf, PaletteSize, Pipeline, dither::FloydSteinberg};

fn encode_png_with_filter(image: &RgbaImage, filter: FilterType) -> Result<Vec<u8>, String> {
    encode_png_with_quality(image, CompressionType::Fast, filter)
}

fn encode_png_with_quality(
    image: &RgbaImage,
    compression: CompressionType,
    filter: FilterType,
) -> Result<Vec<u8>, String> {
    captures_history::encode_png_with_quality(image, compression, filter)
        .map_err(|error| error.to_string())
}

/// Encode a PNG for user export.
///
/// - Preserve (`compact = false`): fast lossless packing, identical pixels.
/// - Compact without a color budget: stronger lossless packing only.
/// - Compact with `max_colors`: lossy **dithered** color quantization, then an
///   indexed PNG (1 byte/pixel) with optional `tRNS` for alpha. Window shadows
///   and transparent canvas padding stay palettized instead of falling back to
///   32-bit RGBA. Files are tagged sRGB so the compressed preview does not pick
///   up a gamma wash on color-managed displays.
pub fn encode_png_export(
    image: &RgbaImage,
    compact: bool,
    max_colors: Option<u16>,
) -> Result<Vec<u8>, String> {
    encode_png_export_dithered(image, compact, max_colors, true)
}

/// Same as [`encode_png_export`], with control over Floyd–Steinberg dithering.
/// Maximum-size search tries the undithered variant when dither noise inflates
/// the deflate stream past the lossless encode.
pub fn encode_png_export_dithered(
    image: &RgbaImage,
    compact: bool,
    max_colors: Option<u16>,
    dither: bool,
) -> Result<Vec<u8>, String> {
    if let Some(colors) = max_colors.filter(|count| *count > 0) {
        let quantized = encode_png_quantized(image, colors, dither)?;
        // Quantization is not guaranteed to shrink already-efficient images
        // (flat UI screenshots often deflate better as full RGBA than as a
        // dithered palette). Never let Compress produce a bigger file than the
        // Preserve encode of the same pixels.
        let lossless = encode_png_with_filter(image, FilterType::Sub)?;
        return Ok(if lossless.len() < quantized.len() {
            lossless
        } else {
            quantized
        });
    }
    if compact {
        encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive)
    } else {
        encode_png_with_filter(image, FilterType::Sub)
    }
}

/// Map the shared compress quality notch (also used for JPEG) to a PNG palette size.
/// Higher quality → more colors kept → larger file. `None` means compact lossless
/// packing only (Highest): same pixels, no color quantization.
pub fn png_palette_colors_for_quality(quality: u8) -> Option<u16> {
    match quality {
        0..=59 => Some(32),   // Tiny (~55)
        60..=77 => Some(64),  // Smaller (~70)
        78..=88 => Some(128), // Balanced (~85)
        89..=94 => Some(256), // High (~92)
        _ => None,            // Highest (~98): tighter packing, no palette
    }
}

/// Palette sizes tried when a hard maximum file size is requested for PNG.
pub const PNG_MAXIMUM_COLOR_STEPS: [u16; 10] = [256, 192, 128, 96, 64, 48, 32, 24, 16, 8];

fn encode_png_quantized(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<Vec<u8>, String> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive);
    }

    let colors = max_colors.clamp(2, 256);
    let (palette, indices) = match index_image(image, colors, dither) {
        Ok(indexed) => indexed,
        Err(_) => {
            return encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive);
        }
    };
    if palette.is_empty() || indices.len() != (width as usize) * (height as usize) {
        return encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive);
    }
    encode_indexed_png(width, height, &palette, &indices)
}

fn index_image(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), String> {
    if let Some(exact) = exact_indexed_rgba(image, max_colors) {
        return Ok(exact);
    }

    let has_transparency = image.pixels().any(|pixel| pixel[3] < 255);
    if !has_transparency {
        return quantette_rgb_indexed(image, max_colors, dither);
    }
    let has_partial_alpha = image.pixels().any(|pixel| pixel[3] > 0 && pixel[3] < 255);
    if has_partial_alpha {
        Ok(median_cut_rgba(image, max_colors, dither))
    } else {
        quantette_rgb_binary_alpha(image, max_colors, dither)
    }
}

fn exact_indexed_rgba(image: &RgbaImage, max_colors: u16) -> Option<(Vec<[u8; 4]>, Vec<u8>)> {
    let limit = usize::from(max_colors);
    let mut map = HashMap::new();
    let mut palette = Vec::new();
    let mut indices = Vec::with_capacity(rgba_pixel_count(image));
    for pixel in image.pixels() {
        // Flat screenshot regions repeat colors: reuse the previous index
        // without hashing, while retaining first-seen palette order.
        if let Some(&index) = indices.last()
            && palette[usize::from(index)] == pixel.0
        {
            indices.push(index);
            continue;
        }
        if let Some(&index) = map.get(&pixel.0) {
            indices.push(index);
            continue;
        }
        if palette.len() >= limit {
            return None;
        }
        let index = u8::try_from(palette.len()).ok()?;
        map.insert(pixel.0, index);
        palette.push(pixel.0);
        indices.push(index);
    }
    Some((palette, indices))
}

fn quantette_rgb_indexed(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), String> {
    let indexed = quantette_rgb(image, max_colors, dither)?;
    let palette = indexed
        .palette()
        .iter()
        .map(|color| [color.red, color.green, color.blue, 255])
        .collect();
    Ok((palette, indexed.indices().to_vec()))
}

fn quantette_rgb_binary_alpha(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), String> {
    let budget = max_colors.saturating_sub(1).clamp(2, 255);
    let indexed = quantette_rgb(image, budget, dither)?;
    let mut palette = Vec::with_capacity(indexed.palette().len() + 1);
    palette.push([0, 0, 0, 0]);
    palette.extend(
        indexed
            .palette()
            .iter()
            .map(|color| [color.red, color.green, color.blue, 255]),
    );
    let mut indices = Vec::with_capacity(rgba_pixel_count(image));
    for (offset, pixel) in image.pixels().enumerate() {
        if pixel[3] == 0 {
            indices.push(0);
        } else {
            indices.push(indexed.indices()[offset].saturating_add(1));
        }
    }
    Ok((palette, indices))
}

fn quantette_rgb(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<quantette::IndexedImage<quantette::deps::palette::Srgb<u8>>, String> {
    let rgb = RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y);
        image::Rgb([pixel[0], pixel[1], pixel[2]])
    });
    let quant_image = ImageBuf::try_from(rgb)
        .map_err(|error| format!("could not prepare image for PNG compression: {error}"))?;
    let pipeline = Pipeline::new()
        .palette_size(PaletteSize::from_u16_clamped(max_colors.clamp(2, 256)))
        .parallel(true);
    // Full error diffusion: optical mixing keeps hues closer to the original
    // while the leftover error reads as speckle / pixelation instead of a
    // global wash. Disable dedup — it fights dithering on busy screenshots.
    let pipeline = if dither {
        let ditherer = FloydSteinberg::with_error_diffusion(1.0).unwrap_or_default();
        pipeline.ditherer(ditherer).dedup(false)
    } else {
        pipeline.ditherer(None)
    };
    let indexed = pipeline
        .input_image(quant_image.as_ref())
        .output_srgb8_indexed_image();
    if indexed.palette().is_empty() || indexed.indices().is_empty() {
        return Err("PNG color quantization produced an empty palette".to_owned());
    }
    Ok(indexed)
}

fn rgba_pixel_count(image: &RgbaImage) -> usize {
    usize::try_from(u64::from(image.width()).saturating_mul(u64::from(image.height())))
        .unwrap_or(usize::MAX)
}

fn median_cut_rgba(image: &RgbaImage, max_colors: u16, dither: bool) -> (Vec<[u8; 4]>, Vec<u8>) {
    let pixel_count = rgba_pixel_count(image);
    let target = usize::from(max_colors)
        .clamp(2, 256)
        .min(pixel_count.max(1));
    let mut boxes = vec![ColorBox {
        members: (0..u32::try_from(pixel_count).unwrap_or(u32::MAX)).collect(),
        bounds: None,
    }];

    while boxes.len() < target {
        let Some((split_at, channel)) = next_split(&mut boxes, image) else {
            break;
        };
        let mut members = std::mem::take(&mut boxes[split_at].members);
        members.sort_unstable_by_key(|&index| rgba_at(image, index)[channel]);
        let mid = members.len() / 2;
        if mid == 0 || mid == members.len() {
            boxes[split_at].members = members;
            break;
        }
        let right = members.split_off(mid);
        boxes[split_at].members = members;
        boxes[split_at].bounds = None;
        boxes.push(ColorBox {
            members: right,
            bounds: None,
        });
    }

    let mut palette = Vec::with_capacity(boxes.len());
    for color_box in &boxes {
        palette.push(box_representative(image, &color_box.members));
    }
    if palette.is_empty() {
        palette.push([0, 0, 0, 255]);
    }
    let indices = if dither {
        dither_rgba_indices(image, &palette)
    } else {
        nearest_rgba_indices(image, &palette)
    };
    (palette, indices)
}

struct ColorBox {
    members: Vec<u32>,
    // Only a split changes these bounds. Compute lazily so the final split
    // (including a two-color palette) does not scan children it never selects.
    bounds: Option<([u8; 4], [u8; 4])>,
}

fn next_split(boxes: &mut [ColorBox], image: &RgbaImage) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, u8)> = None;
    for (box_index, color_box) in boxes.iter_mut().enumerate() {
        if color_box.members.len() < 2 {
            continue;
        }
        let (min, max) = *color_box
            .bounds
            .get_or_insert_with(|| box_bounds(image, &color_box.members));
        let channel = (0..4)
            .max_by_key(|&channel| max[channel].saturating_sub(min[channel]))
            .unwrap_or(0);
        let range = max[channel].saturating_sub(min[channel]);
        if range == 0 {
            continue;
        }
        if best.is_none_or(|(_, _, best_range)| range > best_range) {
            best = Some((box_index, channel, range));
        }
    }
    best.map(|(box_index, channel, _)| (box_index, channel))
}

fn box_bounds(image: &RgbaImage, members: &[u32]) -> ([u8; 4], [u8; 4]) {
    let mut min = [255_u8; 4];
    let mut max = [0_u8; 4];
    for &index in members {
        let pixel = rgba_at(image, index);
        for channel in 0..4 {
            min[channel] = min[channel].min(pixel[channel]);
            max[channel] = max[channel].max(pixel[channel]);
        }
    }
    (min, max)
}

fn box_centroid(image: &RgbaImage, members: &[u32]) -> [u8; 4] {
    if members.is_empty() {
        return [0, 0, 0, 255];
    }
    let mut sum = [0_u64; 4];
    for &index in members {
        let pixel = rgba_at(image, index);
        for channel in 0..4 {
            sum[channel] += u64::from(pixel[channel]);
        }
    }
    let count = u64::try_from(members.len()).unwrap_or(1);
    [
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
        (sum[3] / count) as u8,
    ]
}

/// Prefer a real pixel from the box over the RGB mean. Averages of saturated
/// colors drift toward gray; a medoid keeps the original hue.
fn box_representative(image: &RgbaImage, members: &[u32]) -> [u8; 4] {
    if members.is_empty() {
        return [0, 0, 0, 255];
    }
    let centroid = box_centroid(image, members);
    let mut best = members[0];
    let mut best_dist = u32::MAX;
    for &member in members {
        let dist = rgba_dist2(rgba_at(image, member), centroid);
        if dist < best_dist {
            best_dist = dist;
            best = member;
        }
    }
    rgba_at(image, best)
}

fn rgba_dist2(left: [u8; 4], right: [u8; 4]) -> u32 {
    (0..4).fold(0_u32, |sum, channel| {
        let delta = i32::from(left[channel]) - i32::from(right[channel]);
        sum.saturating_add(u32::try_from(delta.saturating_mul(delta)).unwrap_or(u32::MAX))
    })
}

/// Nearest palette lookup. Linear scan is enough for tiny palettes; a 4D k-d
/// tree keeps High (256 colors) from doing a full scan on every pixel.
struct PaletteIndex<'a> {
    colors: &'a [[u8; 4]],
    nodes: Vec<KdNode>,
}

struct KdNode {
    color_index: u8,
    axis: u8,
    left: Option<u16>,
    right: Option<u16>,
}

impl<'a> PaletteIndex<'a> {
    fn new(colors: &'a [[u8; 4]]) -> Self {
        let mut nodes = Vec::with_capacity(colors.len());
        if !colors.is_empty() {
            let mut order: Vec<u8> = (0..colors.len())
                .filter_map(|index| u8::try_from(index).ok())
                .collect();
            build_kd_node(colors, &mut order, 0, &mut nodes);
        }
        Self { colors, nodes }
    }

    fn nearest(&self, query: [u8; 4]) -> u8 {
        if self.colors.is_empty() {
            return 0;
        }
        if self.colors.len() <= 16 || self.nodes.is_empty() {
            return nearest_linear(self.colors, query);
        }
        let mut best_index = self.nodes[0].color_index;
        let mut best_dist = rgba_dist2(query, self.colors[usize::from(best_index)]);
        search_kd(self, 0, query, &mut best_index, &mut best_dist);
        best_index
    }
}

fn nearest_linear(colors: &[[u8; 4]], query: [u8; 4]) -> u8 {
    let mut best_index = 0_u8;
    let mut best_dist = u32::MAX;
    for (palette_index, candidate) in colors.iter().enumerate() {
        let dist = rgba_dist2(query, *candidate);
        if dist < best_dist {
            best_dist = dist;
            best_index = u8::try_from(palette_index).unwrap_or(255);
        }
    }
    best_index
}

fn build_kd_node(
    colors: &[[u8; 4]],
    order: &mut [u8],
    depth: usize,
    nodes: &mut Vec<KdNode>,
) -> Option<u16> {
    if order.is_empty() {
        return None;
    }
    let axis = u8::try_from(depth % 4).unwrap_or(0);
    order.sort_unstable_by_key(|&index| colors[usize::from(index)][usize::from(axis)]);
    let mid = order.len() / 2;
    let node_id = u16::try_from(nodes.len()).ok()?;
    nodes.push(KdNode {
        color_index: order[mid],
        axis,
        left: None,
        right: None,
    });
    let left = build_kd_node(colors, &mut order[..mid], depth.saturating_add(1), nodes);
    let right = build_kd_node(
        colors,
        &mut order[mid.saturating_add(1)..],
        depth.saturating_add(1),
        nodes,
    );
    if let Some(node) = nodes.get_mut(usize::from(node_id)) {
        node.left = left;
        node.right = right;
    }
    Some(node_id)
}

fn search_kd(
    index: &PaletteIndex<'_>,
    node_id: usize,
    query: [u8; 4],
    best_index: &mut u8,
    best_dist: &mut u32,
) {
    let Some(node) = index.nodes.get(node_id) else {
        return;
    };
    let color = index.colors[usize::from(node.color_index)];
    let dist = rgba_dist2(query, color);
    if dist < *best_dist {
        *best_dist = dist;
        *best_index = node.color_index;
    }
    let axis = usize::from(node.axis);
    let delta = i32::from(query[axis]) - i32::from(color[axis]);
    let (near, far) = if delta <= 0 {
        (node.left, node.right)
    } else {
        (node.right, node.left)
    };
    if let Some(child) = near {
        search_kd(index, usize::from(child), query, best_index, best_dist);
    }
    let plane = u32::try_from(delta.saturating_mul(delta)).unwrap_or(u32::MAX);
    if let Some(child) = far
        && plane < *best_dist
    {
        search_kd(index, usize::from(child), query, best_index, best_dist);
    }
}

fn nearest_rgba_indices(image: &RgbaImage, palette: &[[u8; 4]]) -> Vec<u8> {
    let lookup = PaletteIndex::new(palette);
    image
        .pixels()
        .map(|pixel| lookup.nearest(pixel.0))
        .collect()
}

/// Floyd–Steinberg remap so partial-alpha screenshots get speckle instead of a
/// flat, desaturated nearest-color assignment. Only the current and next rows
/// of diffusion error are kept, so a 4K window capture does not allocate an
/// 800 MB error plane.
fn dither_rgba_indices(image: &RgbaImage, palette: &[[u8; 4]]) -> Vec<u8> {
    let width = usize::try_from(image.width()).unwrap_or(0);
    let height = usize::try_from(image.height()).unwrap_or(0);
    let pixel_count = width.saturating_mul(height);
    let mut indices = vec![0_u8; pixel_count];
    if palette.is_empty() || width == 0 || height == 0 {
        return indices;
    }
    let lookup = PaletteIndex::new(palette);
    let mut current_error = vec![[0_i16; 4]; width];
    let mut next_error = vec![[0_i16; 4]; width];
    for y in 0..height {
        for x in 0..width {
            let offset = y * width + x;
            let pixel = image.get_pixel(x as u32, y as u32).0;
            let mut color = [0_i16; 4];
            for channel in 0..4 {
                color[channel] =
                    (i16::from(pixel[channel]) + current_error[x][channel]).clamp(0, 255);
            }
            let query = [
                color[0] as u8,
                color[1] as u8,
                color[2] as u8,
                color[3] as u8,
            ];
            let best_index = lookup.nearest(query);
            indices[offset] = best_index;
            let chosen = palette[usize::from(best_index)];
            let mut quant_error = [0_i16; 4];
            for channel in 0..4 {
                quant_error[channel] = color[channel] - i16::from(chosen[channel]);
            }
            add_row_error(&mut current_error, x.saturating_add(1), quant_error, 7);
            if x > 0 {
                add_row_error(&mut next_error, x - 1, quant_error, 3);
            }
            add_row_error(&mut next_error, x, quant_error, 5);
            add_row_error(&mut next_error, x.saturating_add(1), quant_error, 1);
        }
        std::mem::swap(&mut current_error, &mut next_error);
        next_error.fill([0; 4]);
    }
    indices
}

fn add_row_error(row: &mut [[i16; 4]], x: usize, quant_error: [i16; 4], numerator: i16) {
    let Some(slot) = row.get_mut(x) else {
        return;
    };
    for channel in 0..4 {
        slot[channel] = slot[channel].saturating_add((quant_error[channel] * numerator) / 16);
    }
}

fn rgba_at(image: &RgbaImage, index: u32) -> [u8; 4] {
    // Quantizer members are already row-major indices into packed RGBA8 pixels.
    let (pixels, _) = image.as_raw().as_chunks::<4>();
    pixels[index as usize]
}

/// Tag PNG output as sRGB with matching gAMA/cHRM. Untagged PNGs are treated as
/// generic RGB (gamma 1.8) on macOS ColorSync, so the compressed `<img>` preview
/// looks washed out next to the sRGB canvas even when the pixels did not change.
fn mark_png_as_srgb(encoder: &mut png::Encoder<&mut Vec<u8>>) {
    captures_history::mark_png_as_srgb(encoder);
}

fn encode_indexed_png(
    width: u32,
    height: u32,
    palette_colors: &[[u8; 4]],
    indices: &[u8],
) -> Result<Vec<u8>, String> {
    let mut palette = Vec::with_capacity(palette_colors.len().max(1) * 3);
    let mut trns = Vec::with_capacity(palette_colors.len());
    let mut has_transparency = false;
    for color in palette_colors {
        palette.push(color[0]);
        palette.push(color[1]);
        palette.push(color[2]);
        trns.push(color[3]);
        has_transparency |= color[3] < 255;
    }
    // png crate requires a non-empty palette for indexed images.
    if palette.is_empty() {
        palette.extend_from_slice(&[0, 0, 0]);
        trns.push(255);
    }
    let depth = match palette_colors.len() {
        0..=2 => png::BitDepth::One,
        3..=4 => png::BitDepth::Two,
        5..=16 => png::BitDepth::Four,
        _ => png::BitDepth::Eight,
    };

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(depth);
        encoder.set_compression(png::Compression::Best);
        // Palette indices are categorical, not intensities: byte-difference
        // filters like Paeth add noise and inflate the deflate stream, which
        // could make a "compressed" PNG larger than the original.
        encoder.set_filter(png::FilterType::NoFilter);
        mark_png_as_srgb(&mut encoder);
        encoder.set_palette(palette);
        if has_transparency {
            encoder.set_trns(trns);
        }
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        // PNG stores sub-byte samples most-significant first, padding each row
        // independently. Keep the palette (including alpha) and indices exact.
        let packed;
        let data = if depth == png::BitDepth::Eight {
            indices
        } else {
            let bits = depth as usize;
            let per_byte = 8 / bits;
            packed = indices
                .chunks(width as usize)
                .flat_map(|row| {
                    row.chunks(per_byte).map(|chunk| {
                        chunk.iter().enumerate().fold(0, |byte, (offset, index)| {
                            byte | (index << (8 - bits * (offset + 1)))
                        })
                    })
                })
                .collect::<Vec<u8>>();
            packed.as_slice()
        };
        writer
            .write_image_data(data)
            .map_err(|error| error.to_string())?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;

    #[test]
    fn indexed_png_packing_preserves_alpha_and_odd_row_boundaries() {
        for (colors, depth) in [
            (2, png::BitDepth::One),
            (3, png::BitDepth::Two),
            (5, png::BitDepth::Four),
            (17, png::BitDepth::Eight),
        ] {
            let palette: Vec<[u8; 4]> = (0..colors)
                .map(|index| {
                    let value = index as u8;
                    [value, 255 - value, value / 2, value]
                })
                .collect();
            let width = 17;
            let height = 19;
            let indices: Vec<u8> = (0..width * height)
                .map(|index| (index % colors) as u8)
                .collect();
            let bytes = encode_indexed_png(width, height, &palette, &indices).unwrap();
            let reader = png::Decoder::new(std::io::Cursor::new(&bytes))
                .read_info()
                .unwrap();
            assert_eq!(reader.info().bit_depth, depth);
            assert_eq!(reader.info().color_type, png::ColorType::Indexed);
            assert_eq!(
                reader.info().srgb,
                Some(png::SrgbRenderingIntent::Perceptual)
            );
            let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
            let expected: Vec<u8> = indices
                .iter()
                .flat_map(|&index| palette[usize::from(index)])
                .collect();
            assert_eq!(decoded.as_raw(), &expected);
        }
    }

    #[test]
    fn exact_palette_keeps_first_seen_order_revisited_colors_and_alpha() {
        let palette = vec![[0, 0, 0, 0], [0, 0, 0, 255], [7, 13, 19, 127]];
        let indices = vec![0, 0, 1, 1, 0, 0, 2, 2, 0, 1, 2, 2];
        let image = RgbaImage::from_fn(6, 2, |x, y| {
            Rgba(palette[usize::from(indices[(y * 6 + x) as usize])])
        });
        assert_eq!(exact_indexed_rgba(&image, 2), None);
        assert_eq!(exact_indexed_rgba(&image, 3), Some((palette, indices)));
    }

    // Regression oracle for the bounds-cache optimization: deliberately
    // recompute every box bound independently from the implementation.
    fn uncached_median_cut(
        image: &RgbaImage,
        colors: u16,
        dither: bool,
    ) -> (Vec<[u8; 4]>, Vec<u8>) {
        let count = rgba_pixel_count(image);
        let target = usize::from(colors).clamp(2, 256).min(count.max(1));
        let mut boxes: Vec<Vec<u32>> = vec![(0..count as u32).collect()];
        while boxes.len() < target {
            let mut best: Option<(usize, usize, u8)> = None;
            for (index, members) in boxes.iter().enumerate() {
                if members.len() < 2 {
                    continue;
                }
                let (min, max) = box_bounds(image, members);
                let channel = (0..4)
                    .max_by_key(|&channel| max[channel] - min[channel])
                    .unwrap();
                let range = max[channel] - min[channel];
                if range > 0 && best.is_none_or(|(_, _, previous)| range > previous) {
                    best = Some((index, channel, range));
                }
            }
            let Some((index, channel, _)) = best else {
                break;
            };
            boxes[index].sort_unstable_by_key(|&pixel| rgba_at(image, pixel)[channel]);
            let mid = boxes[index].len() / 2;
            let right = boxes[index].split_off(mid);
            boxes.push(right);
        }
        let palette: Vec<_> = boxes
            .iter()
            .map(|members| box_representative(image, members))
            .collect();
        let indices = if dither {
            dither_rgba_indices(image, &palette)
        } else {
            nearest_rgba_indices(image, &palette)
        };
        (palette, indices)
    }

    #[test]
    fn median_cut_matches_uncached_palette_order_and_indices() {
        let fixtures = [
            RgbaImage::new(0, 0),
            RgbaImage::from_pixel(1, 1, Rgba([13, 47, 99, 128])),
            RgbaImage::from_fn(9, 9, |x, y| {
                Rgba([
                    (x * 17) as u8,
                    (y * 17) as u8,
                    (255 - x * 17) as u8,
                    (255 - y * 17) as u8,
                ])
            }),
            RgbaImage::from_fn(23, 17, |x, y| {
                Rgba([
                    (x * 11) as u8,
                    (y * 13) as u8,
                    (x * 37 + y * 71) as u8,
                    (x * 7 + y * 3) as u8,
                ])
            }),
        ];
        for image in &fixtures {
            for colors in [0, 2, 3, 16, 64, 256, 300] {
                for dither in [false, true] {
                    assert_eq!(
                        median_cut_rgba(image, colors, dither),
                        uncached_median_cut(image, colors, dither),
                        "{:?}, {colors} colors, dither={dither}",
                        image.dimensions()
                    );
                }
            }
        }
    }

    #[test]
    fn palette_kd_tree_finds_the_same_nearest_distance_as_linear_search() {
        let palette: Vec<[u8; 4]> = (0..32_u8)
            .map(|index| {
                [
                    index.wrapping_mul(7),
                    index.wrapping_mul(11),
                    index.wrapping_mul(3),
                    255 - index,
                ]
            })
            .collect();
        let lookup = PaletteIndex::new(&palette);
        for red in (0..=255).step_by(19) {
            for green in (0..=255).step_by(23) {
                for blue in (0..=255).step_by(29) {
                    let query = [red, green, blue, 200];
                    let kd = lookup.nearest(query);
                    let linear = nearest_linear(&palette, query);
                    assert_eq!(
                        rgba_dist2(query, palette[usize::from(kd)]),
                        rgba_dist2(query, palette[usize::from(linear)])
                    );
                }
            }
        }
    }

    #[test]
    fn partial_alpha_palette_output_is_deterministic_and_srgb() {
        let image = RgbaImage::from_fn(23, 17, |x, y| {
            Rgba([
                (x * 11) as u8,
                (y * 13) as u8,
                (x * 37 + y * 71) as u8,
                (x * 7 + y * 3) as u8,
            ])
        });
        let first = encode_png_export_dithered(&image, true, Some(16), true).unwrap();
        let second = encode_png_export_dithered(&image, true, Some(16), true).unwrap();
        assert_eq!(first, second);
        let reader = png::Decoder::new(std::io::Cursor::new(&first))
            .read_info()
            .unwrap();
        assert_eq!(reader.info().color_type, png::ColorType::Indexed);
        assert_eq!(
            reader.info().srgb,
            Some(png::SrgbRenderingIntent::Perceptual)
        );
    }
}
