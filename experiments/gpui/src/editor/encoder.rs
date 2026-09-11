//! Image export encoders adapted from Captures' native editor and kept in
//! lockstep with the production Tauri editor. Copyright Captures contributors;
//! used here under this repository's Apache-2.0 license.

use image::{Rgb, RgbImage, RgbaImage};
use quantette::{ImageBuf, PaletteSize, Pipeline, dither::FloydSteinberg};
use std::collections::HashMap;

pub fn png_palette_colors_for_quality(quality: u8) -> Option<u16> {
    match quality {
        0..=59 => Some(32),
        60..=77 => Some(64),
        78..=88 => Some(128),
        89..=94 => Some(256),
        _ => None,
    }
}

pub fn encode_png(image: &RgbaImage, quality: Option<u8>) -> Result<Vec<u8>, String> {
    let Some(quality) = quality else {
        return encode_png_rgba(image, false);
    };
    let Some(colors) = png_palette_colors_for_quality(quality) else {
        return encode_png_rgba(image, true);
    };
    let quantized = encode_png_quantized(image, colors, true)?;
    let preserve = encode_png_rgba(image, false)?;
    Ok(if preserve.len() < quantized.len() {
        preserve
    } else {
        quantized
    })
}

pub fn encode_jpeg(image: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgb = composite_onto_white(image);
    let width = u16::try_from(rgb.width()).map_err(|_| "JPEG width is too large to encode")?;
    let height = u16::try_from(rgb.height()).map_err(|_| "JPEG height is too large to encode")?;
    let mut bytes = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, quality.clamp(40, 100));
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    encoder.set_quantization_tables(
        jpeg_encoder::QuantizationTableType::ImageMagick,
        jpeg_encoder::QuantizationTableType::ImageMagick,
    );
    encoder
        .encode(rgb.as_raw(), width, height, jpeg_encoder::ColorType::Rgb)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

pub fn encode_webp(image: &RgbaImage, quality: Option<u8>) -> Result<Vec<u8>, String> {
    if image.width() == 0 || image.height() == 0 {
        return Err("cannot encode an empty WebP image".into());
    }
    let encoder = webp::Encoder::from_rgba(image.as_raw(), image.width(), image.height());
    let encoded = match quality {
        None => encoder
            .encode_simple(true, 100.)
            .map_err(|error| format!("WebP lossless encode failed: {error:?}"))?,
        Some(quality) => {
            let mut config = webp::WebPConfig::new()
                .map_err(|error| format!("WebP config failed: {error:?}"))?;
            config.lossless = 0;
            config.quality = f32::from(quality.clamp(1, 100));
            config.use_sharp_yuv = 1;
            encoder
                .encode_advanced(&config)
                .map_err(|error| format!("WebP lossy encode failed: {error:?}"))?
        }
    };
    Ok(encoded.to_vec())
}

pub fn encode_png_with_limit(image: &RgbaImage, maximum: u64) -> Result<Vec<u8>, String> {
    let preserve = encode_png(image, None)?;
    if encoded_len(&preserve) <= maximum {
        return Ok(preserve);
    }
    let mut best = None;
    'palettes: for colors in [256, 192, 128, 96, 64, 48, 32, 24, 16, 8] {
        for dither in [true, false] {
            let quantized = encode_png_quantized(image, colors, dither)?;
            let candidate = if preserve.len() < quantized.len() {
                preserve.clone()
            } else {
                quantized
            };
            let fits = encoded_len(&candidate) <= maximum;
            best = Some(candidate);
            if fits {
                break 'palettes;
            }
        }
    }
    let best = best.ok_or("PNG encoding did not produce an image")?;
    if encoded_len(&best) <= maximum {
        Ok(best)
    } else {
        Err("the PNG is larger than the requested maximum even after reducing colors; reduce the output size, raise the limit, or switch to JPEG for more aggressive size control".into())
    }
}

pub fn encode_jpeg_with_limit(image: &RgbaImage, maximum: u64) -> Result<Vec<u8>, String> {
    let preserve = encode_jpeg(image, 100)?;
    if encoded_len(&preserve) <= maximum {
        return Ok(preserve);
    }
    let minimum = encode_jpeg(image, 40)?;
    if encoded_len(&minimum) > maximum {
        return Err("JPEG cannot meet the requested maximum at the supported quality range; reduce the output size or raise the limit".into());
    }
    let mut best = minimum;
    let mut low = 41_u8;
    let mut high = 100_u8;
    while low <= high {
        let quality = low + (high - low) / 2;
        let candidate = encode_jpeg(image, quality)?;
        if encoded_len(&candidate) <= maximum {
            best = candidate;
            low = quality.saturating_add(1);
        } else {
            high = quality - 1;
        }
    }
    Ok(best)
}

pub fn encode_webp_with_limit(image: &RgbaImage, maximum: u64) -> Result<Vec<u8>, String> {
    let preserve = encode_webp(image, None)?;
    if encoded_len(&preserve) <= maximum {
        return Ok(preserve);
    }
    let minimum = encode_webp(image, Some(1))?;
    if encoded_len(&minimum) > maximum {
        return Err("WebP cannot meet the requested maximum at the supported quality range; reduce the output size or raise the limit".into());
    }
    let mut best = minimum;
    let mut low = 2_u8;
    let mut high = 100_u8;
    while low <= high {
        let quality = low + (high - low) / 2;
        let candidate = encode_webp(image, Some(quality))?;
        if encoded_len(&candidate) <= maximum {
            best = candidate;
            low = quality.saturating_add(1);
        } else {
            high = quality - 1;
        }
    }
    Ok(best)
}

fn encoded_len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

fn composite_onto_white(image: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y);
        let alpha = u16::from(pixel[3]);
        let inverse = 255 - alpha;
        Rgb([
            ((u16::from(pixel[0]) * alpha + 255 * inverse) / 255) as u8,
            ((u16::from(pixel[1]) * alpha + 255 * inverse) / 255) as u8,
            ((u16::from(pixel[2]) * alpha + 255 * inverse) / 255) as u8,
        ])
    })
}

fn encode_png_quantized(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<Vec<u8>, String> {
    if image.width() == 0 || image.height() == 0 {
        return encode_png_rgba(image, true);
    }
    let (palette, indices) = index_image(image, max_colors.clamp(2, 256), dither)?;
    if palette.is_empty() || indices.len() != pixel_count(image) {
        return encode_png_rgba(image, true);
    }
    encode_indexed_png(image.width(), image.height(), &palette, &indices)
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
    if image.pixels().any(|pixel| pixel[3] > 0 && pixel[3] < 255) {
        Ok(median_cut_rgba(image, max_colors, dither))
    } else {
        quantette_rgb_binary_alpha(image, max_colors, dither)
    }
}

fn exact_indexed_rgba(image: &RgbaImage, max_colors: u16) -> Option<(Vec<[u8; 4]>, Vec<u8>)> {
    let mut map = HashMap::new();
    let mut palette = Vec::new();
    let mut indices = Vec::with_capacity(pixel_count(image));
    for pixel in image.pixels() {
        if let Some(&index) = map.get(&pixel.0) {
            indices.push(index);
        } else {
            if palette.len() >= usize::from(max_colors) {
                return None;
            }
            let index = u8::try_from(palette.len()).ok()?;
            map.insert(pixel.0, index);
            palette.push(pixel.0);
            indices.push(index);
        }
    }
    Some((palette, indices))
}

fn quantette_rgb_indexed(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), String> {
    let indexed = quantette_rgb(image, max_colors, dither)?;
    Ok((
        indexed
            .palette()
            .iter()
            .map(|color| [color.red, color.green, color.blue, 255])
            .collect(),
        indexed.indices().to_vec(),
    ))
}

fn quantette_rgb_binary_alpha(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), String> {
    let indexed = quantette_rgb(image, max_colors.saturating_sub(1).clamp(2, 255), dither)?;
    let mut palette = vec![[0, 0, 0, 0]];
    palette.extend(
        indexed
            .palette()
            .iter()
            .map(|color| [color.red, color.green, color.blue, 255]),
    );
    let indices = image
        .pixels()
        .enumerate()
        .map(|(offset, pixel)| {
            if pixel[3] == 0 {
                0
            } else {
                indexed.indices()[offset].saturating_add(1)
            }
        })
        .collect();
    Ok((palette, indices))
}

fn quantette_rgb(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<quantette::IndexedImage<quantette::deps::palette::Srgb<u8>>, String> {
    let rgb = RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y);
        Rgb([pixel[0], pixel[1], pixel[2]])
    });
    let quant_image = ImageBuf::try_from(rgb).map_err(|error| error.to_string())?;
    let pipeline = Pipeline::new()
        .palette_size(PaletteSize::from_u16_clamped(max_colors.clamp(2, 256)))
        .parallel(true);
    let pipeline = if dither {
        pipeline
            .ditherer(FloydSteinberg::with_error_diffusion(1.).unwrap_or_default())
            .dedup(false)
    } else {
        pipeline.ditherer(None)
    };
    let indexed = pipeline
        .input_image(quant_image.as_ref())
        .output_srgb8_indexed_image();
    if indexed.palette().is_empty() || indexed.indices().is_empty() {
        return Err("PNG color quantization produced an empty palette".into());
    }
    Ok(indexed)
}

#[derive(Debug)]
struct ColorBox {
    members: Vec<u32>,
}

fn median_cut_rgba(image: &RgbaImage, max_colors: u16, dither: bool) -> (Vec<[u8; 4]>, Vec<u8>) {
    let target = usize::from(max_colors)
        .clamp(2, 256)
        .min(pixel_count(image).max(1));
    let mut boxes = vec![ColorBox {
        members: (0..u32::try_from(pixel_count(image)).unwrap_or(u32::MAX)).collect(),
    }];
    while boxes.len() < target {
        let Some((split_at, channel)) = next_split(&boxes, image) else {
            break;
        };
        let mut members = std::mem::take(&mut boxes[split_at].members);
        members.sort_unstable_by_key(|&index| rgba_at(image, index)[channel]);
        let right = members.split_off(members.len() / 2);
        if members.is_empty() || right.is_empty() {
            boxes[split_at].members.extend(right);
            break;
        }
        boxes[split_at].members = members;
        boxes.push(ColorBox { members: right });
    }
    let mut palette: Vec<_> = boxes
        .iter()
        .map(|color_box| box_representative(image, &color_box.members))
        .collect();
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

fn next_split(boxes: &[ColorBox], image: &RgbaImage) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, u8)> = None;
    for (box_index, color_box) in boxes
        .iter()
        .enumerate()
        .filter(|(_, b)| b.members.len() > 1)
    {
        let (min, max) = box_bounds(image, &color_box.members);
        let channel = (0..4)
            .max_by_key(|&channel| max[channel].saturating_sub(min[channel]))
            .unwrap_or(0);
        let range = max[channel].saturating_sub(min[channel]);
        if range > 0 && best.is_none_or(|(_, _, previous)| range > previous) {
            best = Some((box_index, channel, range));
        }
    }
    best.map(|(index, channel, _)| (index, channel))
}

fn box_bounds(image: &RgbaImage, members: &[u32]) -> ([u8; 4], [u8; 4]) {
    let mut min = [255; 4];
    let mut max = [0; 4];
    for &index in members {
        let pixel = rgba_at(image, index);
        for channel in 0..4 {
            min[channel] = min[channel].min(pixel[channel]);
            max[channel] = max[channel].max(pixel[channel]);
        }
    }
    (min, max)
}

fn box_representative(image: &RgbaImage, members: &[u32]) -> [u8; 4] {
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
    let centroid = [
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
        (sum[3] / count) as u8,
    ];
    members
        .iter()
        .copied()
        .min_by_key(|&index| rgba_dist2(rgba_at(image, index), centroid))
        .map(|index| rgba_at(image, index))
        .unwrap_or([0, 0, 0, 255])
}

fn rgba_dist2(left: [u8; 4], right: [u8; 4]) -> u32 {
    (0..4).fold(0, |sum, channel| {
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

fn dither_rgba_indices(image: &RgbaImage, palette: &[[u8; 4]]) -> Vec<u8> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let mut indices = vec![0; width.saturating_mul(height)];
    let lookup = PaletteIndex::new(palette);
    let mut current_error = vec![[0_i16; 4]; width];
    let mut next_error = vec![[0_i16; 4]; width];
    for y in 0..height {
        for x in 0..width {
            let pixel = image.get_pixel(x as u32, y as u32).0;
            let mut adjusted = [0_u8; 4];
            for channel in 0..4 {
                adjusted[channel] =
                    (i16::from(pixel[channel]) + current_error[x][channel]).clamp(0, 255) as u8;
            }
            let palette_index = lookup.nearest(adjusted);
            indices[y * width + x] = palette_index;
            let chosen = palette[usize::from(palette_index)];
            let mut error = [0_i16; 4];
            for channel in 0..4 {
                error[channel] = i16::from(adjusted[channel]) - i16::from(chosen[channel]);
            }
            add_error(&mut current_error, x + 1, error, 7);
            if x > 0 {
                add_error(&mut next_error, x - 1, error, 3);
            }
            add_error(&mut next_error, x, error, 5);
            add_error(&mut next_error, x + 1, error, 1);
        }
        std::mem::swap(&mut current_error, &mut next_error);
        next_error.fill([0; 4]);
    }
    indices
}

fn add_error(row: &mut [[i16; 4]], x: usize, error: [i16; 4], numerator: i16) {
    if let Some(slot) = row.get_mut(x) {
        for channel in 0..4 {
            slot[channel] = slot[channel].saturating_add((error[channel] * numerator) / 16);
        }
    }
}

fn rgba_at(image: &RgbaImage, index: u32) -> [u8; 4] {
    let width = image.width().max(1);
    image.get_pixel(index % width, index / width).0
}

fn pixel_count(image: &RgbaImage) -> usize {
    usize::try_from(u64::from(image.width()) * u64::from(image.height())).unwrap_or(usize::MAX)
}

fn mark_png_as_srgb(encoder: &mut png::Encoder<&mut Vec<u8>>) {
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder.set_source_gamma(png::ScaledFloat::from_scaled(45_455));
    encoder.set_source_chromaticities(png::SourceChromaticities::new(
        (0.3127, 0.3290),
        (0.6400, 0.3300),
        (0.3000, 0.6000),
        (0.1500, 0.0600),
    ));
}

fn encode_png_rgba(image: &RgbaImage, compact: bool) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(if compact {
        png::Compression::Best
    } else {
        png::Compression::Fast
    });
    if compact {
        encoder.set_adaptive_filter(png::AdaptiveFilterType::Adaptive);
        encoder.set_filter(png::FilterType::Paeth);
    } else {
        encoder.set_filter(png::FilterType::Sub);
    }
    mark_png_as_srgb(&mut encoder);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(image.as_raw())
        .map_err(|error| error.to_string())?;
    drop(writer);
    Ok(bytes)
}

fn encode_indexed_png(
    width: u32,
    height: u32,
    palette_colors: &[[u8; 4]],
    indices: &[u8],
) -> Result<Vec<u8>, String> {
    let mut palette = Vec::with_capacity(palette_colors.len().max(1) * 3);
    let mut alpha = Vec::with_capacity(palette_colors.len());
    for color in palette_colors {
        palette.extend_from_slice(&color[..3]);
        alpha.push(color[3]);
    }
    if palette.is_empty() {
        palette.extend_from_slice(&[0, 0, 0]);
        alpha.push(255);
    }
    let has_transparency = alpha.iter().any(|value| *value < 255);
    let depth = match palette_colors.len() {
        0..=2 => png::BitDepth::One,
        3..=4 => png::BitDepth::Two,
        5..=16 => png::BitDepth::Four,
        _ => png::BitDepth::Eight,
    };
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(depth);
    encoder.set_compression(png::Compression::Best);
    encoder.set_filter(png::FilterType::NoFilter);
    mark_png_as_srgb(&mut encoder);
    encoder.set_palette(palette);
    if has_transparency {
        encoder.set_trns(alpha);
    }
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
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
            .collect::<Vec<_>>();
        &packed
    };
    writer
        .write_image_data(data)
        .map_err(|error| error.to_string())?;
    drop(writer);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn detailed(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                (x * 17 + y * 3) as u8,
                (x * 5 + y * 11) as u8,
                (x * y + 40) as u8,
                if x < width / 5 {
                    (x * 255 / (width / 5).max(1)) as u8
                } else {
                    255
                },
            ])
        })
    }

    #[test]
    fn png_quality_ladder_is_indexed_with_alpha_and_never_larger() {
        let image = detailed(220, 150);
        let preserve = encode_png(&image, None).unwrap();
        let high = encode_png(&image, Some(92)).unwrap();
        let tiny = encode_png(&image, Some(55)).unwrap();
        assert!(high.len() <= preserve.len());
        assert!(tiny.len() <= preserve.len());
        assert!(tiny.len() < high.len());
        let info = png::Decoder::new(std::io::Cursor::new(&tiny))
            .read_info()
            .unwrap()
            .info()
            .clone();
        assert_eq!(info.color_type, png::ColorType::Indexed);
        assert!(info.trns.is_some());
        let decoded = image::load_from_memory(&tiny).unwrap().to_rgba8();
        assert!(decoded.pixels().any(|pixel| pixel[3] < 255));
        assert!(decoded.pixels().any(|pixel| pixel[3] > 0 && pixel[3] < 255));
    }

    #[test]
    fn lossy_webp_quality_changes_pixels_and_size_but_keeps_alpha() {
        let image = detailed(240, 160);
        let lossless = encode_webp(&image, None).unwrap();
        let high = encode_webp(&image, Some(92)).unwrap();
        let tiny = encode_webp(&image, Some(55)).unwrap();
        assert!(tiny.len() < high.len());
        assert!(lossless.windows(4).any(|chunk| chunk == b"VP8L"));
        assert!(high.windows(4).any(|chunk| chunk == b"VP8 "));
        assert!(!high.windows(4).any(|chunk| chunk == b"VP8L"));
        let decoded = image::load_from_memory(&tiny).unwrap().to_rgba8();
        assert_ne!(decoded, image);
        assert_eq!(decoded.get_pixel(0, 20).0[3], 0);
        assert!(decoded.pixels().any(|pixel| pixel[3] > 0 && pixel[3] < 255));
    }

    #[test]
    fn jpeg_uses_full_resolution_chroma_and_white_alpha_composite() {
        let mut image = RgbaImage::from_fn(31, 17, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([255, 0, 40, 255])
            } else {
                Rgba([0, 220, 255, 255])
            }
        });
        image.put_pixel(30, 16, Rgba([0, 0, 0, 0]));
        let bytes = encode_jpeg(&image, 98).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let corner = decoded.get_pixel(30, 16).0;
        assert!(corner.iter().all(|channel| *channel > 225));
        let red = decoded.get_pixel(10, 6).0;
        let cyan = decoded.get_pixel(11, 6).0;
        assert!(red[0] > red[1] + 80, "red chroma was blurred: {red:?}");
        assert!(cyan[2] > cyan[0] + 80, "cyan chroma was blurred: {cyan:?}");
    }

    #[test]
    fn maximum_size_search_preserves_when_possible_and_steps_down_when_needed() {
        type LimitedEncoder = fn(&RgbaImage, u64) -> Result<Vec<u8>, String>;
        let image = detailed(240, 160);
        let cases: [(Vec<u8>, Vec<u8>, LimitedEncoder); 3] = [
            (
                encode_png(&image, None).unwrap(),
                encode_png(&image, Some(55)).unwrap(),
                encode_png_with_limit,
            ),
            (
                encode_jpeg(&image, 100).unwrap(),
                encode_jpeg(&image, 60).unwrap(),
                encode_jpeg_with_limit,
            ),
            (
                encode_webp(&image, None).unwrap(),
                encode_webp(&image, Some(60)).unwrap(),
                encode_webp_with_limit,
            ),
        ];
        for (preserve, compressed, limited) in cases {
            assert_eq!(limited(&image, preserve.len() as u64).unwrap(), preserve);
            let cap = compressed.len() as u64;
            let result = limited(&image, cap).unwrap();
            assert!(result.len() as u64 <= cap);
            assert_ne!(result, preserve);
        }
    }
}
