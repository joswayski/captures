//! Shipping CSS effects egui has no primitive for: `filter: blur()` as a real
//! separable Gaussian (standard deviation = the CSS radius, transparent past
//! the element's edges), `box-shadow` layers as cached Gaussian-blurred masks
//! (standard deviation = half the CSS blur radius, clipped outside the
//! element like CSS), and shapes drawn through a 2D projective map for the
//! preview pile's 3D pose. Everything is computed once per size and cached,
//! so a settled frame repaints nothing new.

use std::collections::HashMap;

use eframe::egui::{self, Color32, ColorImage, Mesh, Pos2, Rect, Vec2};
use serde::Deserialize;

/// One CSS `box-shadow` layer, from the generated `shadows` tokens.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct BoxShadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    #[serde(default)]
    pub spread: f32,
    /// Unmultiplied sRGB, 0…1.
    pub color: [f32; 4],
}

impl BoxShadow {
    /// A centred `0 0 <blur> <color>` glow.
    pub fn glow(blur: f32, color: Color32) -> Self {
        let [r, g, b, a] = color.to_srgba_unmultiplied().map(|c| f32::from(c) / 255.);
        Self {
            x: 0.,
            y: 0.,
            blur,
            spread: 0.,
            color: [r, g, b, a],
        }
    }

    fn color32(&self, opacity: f32) -> Color32 {
        let [r, g, b, a] = self.color.map(|c| (c.clamp(0., 1.) * 255.).round() as u8);
        Color32::from_rgba_unmultiplied(r, g, b, a).gamma_multiply(opacity.clamp(0., 1.))
    }
}

/// Normalised Gaussian taps for `sigma` (in pixels), radius `ceil(3σ)`.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    if sigma.is_nan() || sigma <= 0. {
        return vec![1.];
    }
    let radius = (sigma * 3.).ceil() as i32;
    let taps: Vec<f32> = (-radius..=radius)
        .map(|i| (-(i * i) as f32 / (2. * sigma * sigma)).exp())
        .collect();
    let sum: f32 = taps.iter().sum();
    taps.into_iter().map(|tap| tap / sum).collect()
}

/// Separable Gaussian blur of a premultiplied image with per-axis standard
/// deviations in pixels. Pixels past the edges are transparent, as for a CSS
/// filter on an element; `pad` grows the output on each side so the blur
/// can spill outside (zero keeps the input size).
pub fn gaussian_blur(image: &ColorImage, sigma: [f32; 2], pad: [usize; 2]) -> ColorImage {
    let [width, height] = image.size;
    let (out_w, out_h) = (width + 2 * pad[0], height + 2 * pad[1]);
    let mut plane = vec![[0f32; 4]; out_w * out_h];
    for y in 0..height {
        for x in 0..width {
            let pixel = image.pixels[y * width + x];
            plane[(y + pad[1]) * out_w + x + pad[0]] = [
                f32::from(pixel.r()),
                f32::from(pixel.g()),
                f32::from(pixel.b()),
                f32::from(pixel.a()),
            ];
        }
    }
    let pass = |source: &[[f32; 4]], kernel: &[f32], horizontal: bool| {
        let radius = (kernel.len() / 2) as isize;
        let mut out = vec![[0f32; 4]; source.len()];
        let (len, lines) = if horizontal {
            (out_w, out_h)
        } else {
            (out_h, out_w)
        };
        let index = |line: usize, at: usize| {
            if horizontal {
                line * out_w + at
            } else {
                at * out_w + line
            }
        };
        for line in 0..lines {
            for at in 0..len {
                let mut sum = [0f32; 4];
                for (k, weight) in kernel.iter().enumerate() {
                    let from = at as isize + k as isize - radius;
                    if from < 0 || from >= len as isize {
                        continue;
                    }
                    let value = source[index(line, from as usize)];
                    for channel in 0..4 {
                        sum[channel] += value[channel] * weight;
                    }
                }
                out[index(line, at)] = sum;
            }
        }
        out
    };
    if sigma[0] > 0. {
        plane = pass(&plane, &gaussian_kernel(sigma[0]), true);
    }
    if sigma[1] > 0. {
        plane = pass(&plane, &gaussian_kernel(sigma[1]), false);
    }
    let byte = |value: f32| value.round().clamp(0., 255.) as u8;
    ColorImage::new(
        [out_w, out_h],
        plane
            .into_iter()
            .map(|[r, g, b, a]| {
                let a = byte(a);
                Color32::from_rgba_premultiplied(byte(r).min(a), byte(g).min(a), byte(b).min(a), a)
            })
            .collect(),
    )
}

/// Composite a premultiplied image over an opaque `backdrop`, in place.
pub fn composite_over(image: &mut ColorImage, backdrop: Color32) {
    let [br, bg, bb, _] = backdrop.to_array();
    for pixel in &mut image.pixels {
        let [r, g, b, a] = pixel.to_array();
        let rest = 255 - u16::from(a);
        let mix = |value: u8, under: u8| {
            (u16::from(value) + (u16::from(under) * rest + 127) / 255).min(255) as u8
        };
        *pixel = Color32::from_rgb(mix(r, br), mix(g, bg), mix(b, bb));
    }
}

/// Halve a premultiplied image with a 2×2 box filter.
pub fn downsample(image: &ColorImage) -> ColorImage {
    let [width, height] = image.size;
    let (out_w, out_h) = ((width / 2).max(1), (height / 2).max(1));
    let mut pixels = Vec::with_capacity(out_w * out_h);
    for y in 0..out_h {
        for x in 0..out_w {
            let mut sum = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let pixel = image.pixels
                    [(2 * y + dy).min(height - 1) * width + (2 * x + dx).min(width - 1)];
                for (total, value) in sum.iter_mut().zip(pixel.to_array()) {
                    *total += u32::from(value);
                }
            }
            let [r, g, b, a] = sum.map(|total| ((total + 2) / 4) as u8);
            pixels.push(Color32::from_rgba_premultiplied(r, g, b, a));
        }
    }
    ColorImage::new([out_w, out_h], pixels)
}

/// CSS `filter: blur(<sigma_pt>)` of card media held at 2 px per point.
/// Blurs of a point or more carry no detail a 1 px-per-point copy cannot, so
/// those run on a half-size copy (a quarter of the work).
pub fn blur_card_media(media_2x: &ColorImage, sigma_pt: f32) -> ColorImage {
    if sigma_pt >= 1. {
        gaussian_blur(&downsample(media_2x), [sigma_pt, sigma_pt], [0, 0])
    } else {
        gaussian_blur(media_2x, [sigma_pt * 2., sigma_pt * 2.], [0, 0])
    }
}

/// Signed distance from `p` (relative to the centre) to a rounded rect.
fn rounded_rect_distance(p: Vec2, half: Vec2, radius: f32) -> f32 {
    let radius = radius.clamp(0., half.x.min(half.y).max(0.));
    let q = p.abs() - half + Vec2::splat(radius);
    q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.) - radius
}

/// Pixels per point for a shadow mask: soft shadows need no more than one.
fn shadow_density(blur: f32) -> f32 {
    if blur < 4. { 2. } else { 1. }
}

/// Mask for one layer: the element's rounded rect grown by `spread`,
/// Gaussian-blurred (σ = blur / 2), minus the element itself (CSS never
/// paints an outer shadow under its element). White, premultiplied; the
/// returned margin is the mask's extent past the element on each side.
pub fn shadow_mask(size: Vec2, radius: f32, shadow: &BoxShadow) -> (ColorImage, Vec2) {
    let density = shadow_density(shadow.blur);
    let sigma = shadow.blur.max(0.) / 2.;
    let pad = (sigma * 3.).ceil() + 1.;
    let spread = shadow.spread;
    let shape = (size + Vec2::splat(2. * spread)).max(Vec2::ZERO);
    let shape_radius = if radius > 0. {
        (radius + spread).max(0.)
    } else {
        0.
    };
    let offset = egui::vec2(shadow.x, shadow.y);
    // The canvas centres the grown shape; the element sits at -offset.
    let canvas = shape + Vec2::splat(2. * pad);
    let [w, h] = [
        (canvas.x * density).ceil().max(1.) as usize,
        (canvas.y * density).ceil().max(1.) as usize,
    ];
    let centre = egui::vec2(w as f32, h as f32) / (2. * density);
    let coverage = |p: Vec2, half: Vec2, r: f32| {
        (0.5 - rounded_rect_distance(p, half, r) * density).clamp(0., 1.)
    };
    let mut shape_mask = ColorImage::filled([w, h], Color32::TRANSPARENT);
    for y in 0..h {
        for x in 0..w {
            let p = egui::vec2(x as f32 + 0.5, y as f32 + 0.5) / density - centre;
            let a = (coverage(p, shape / 2., shape_radius) * 255.).round() as u8;
            shape_mask.pixels[y * w + x] = Color32::from_rgba_premultiplied(a, a, a, a);
        }
    }
    let mut blurred = gaussian_blur(&shape_mask, [sigma * density; 2], [0, 0]);
    for y in 0..h {
        for x in 0..w {
            let p = egui::vec2(x as f32 + 0.5, y as f32 + 0.5) / density - centre + offset;
            let keep = 1. - coverage(p, size / 2., radius);
            let pixel = &mut blurred.pixels[y * w + x];
            let a = (f32::from(pixel.a()) * keep).round() as u8;
            *pixel = Color32::from_rgba_premultiplied(a, a, a, a);
        }
    }
    let margin =
        Vec2::splat(pad + spread) + (egui::vec2(w as f32, h as f32) / density - canvas) / 2.;
    (blurred, margin)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MaskKey([i32; 7]);

impl MaskKey {
    fn new(size: Vec2, radius: f32, shadow: &BoxShadow) -> Self {
        let q = |value: f32| (value * 4.).round() as i32;
        Self([
            q(size.x),
            q(size.y),
            q(radius),
            q(shadow.blur),
            q(shadow.spread),
            q(shadow.x),
            q(shadow.y),
        ])
    }
}

/// Uploaded masks by geometry. Morphing controls could mint many sizes, so
/// the cache resets past a small bound instead of growing without limit.
#[derive(Clone, Default)]
struct MaskCache(HashMap<MaskKey, (egui::TextureHandle, Vec2)>);

const MASK_CACHE_LIMIT: usize = 96;

fn shadow_texture(
    ctx: &egui::Context,
    size: Vec2,
    radius: f32,
    shadow: &BoxShadow,
) -> (egui::TextureHandle, Vec2) {
    let key = MaskKey::new(size, radius, shadow);
    let id = egui::Id::unique("captures-box-shadow-masks");
    if let Some(found) = ctx.data(|data| {
        data.get_temp::<MaskCache>(id)
            .and_then(|cache| cache.0.get(&key).cloned())
    }) {
        return found;
    }
    let (image, margin) = shadow_mask(size, radius, shadow);
    let texture = ctx.load_texture("box-shadow", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<MaskCache>(id);
        if cache.0.len() >= MASK_CACHE_LIMIT {
            cache.0.clear();
        }
        cache.0.insert(key, (texture.clone(), margin));
    });
    (texture, margin)
}

/// A textured quad split into a grid so a projective map stays accurate.
fn mapped_quad(
    texture: egui::TextureId,
    local: Rect,
    tint: Color32,
    map: &dyn Fn(Pos2) -> Pos2,
) -> Mesh {
    const STEPS: u32 = 6;
    let mut mesh = Mesh::with_texture(texture);
    for row in 0..=STEPS {
        for column in 0..=STEPS {
            let t = egui::vec2(column as f32, row as f32) / STEPS as f32;
            let point = local.min + local.size() * t;
            mesh.vertices.push(egui::epaint::Vertex {
                pos: map(point),
                uv: t.to_pos2(),
                color: tint,
            });
        }
    }
    let stride = STEPS + 1;
    for row in 0..STEPS {
        for column in 0..STEPS {
            let a = row * stride + column;
            mesh.add_triangle(a, a + 1, a + stride);
            mesh.add_triangle(a + 1, a + stride + 1, a + stride);
        }
    }
    mesh
}

/// Paint CSS `box-shadow` layers for `rect` (last layer first, like CSS),
/// at `opacity`. `map` carries the element's transform (identity for flat
/// elements); layers are laid out in `rect`'s own untransformed space.
pub fn paint_box_shadows_mapped(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    shadows: &[BoxShadow],
    opacity: f32,
    map: &dyn Fn(Pos2) -> Pos2,
) {
    if opacity <= 0. || !rect.is_positive() {
        return;
    }
    for shadow in shadows.iter().rev() {
        let tint = shadow.color32(opacity);
        if tint.a() == 0 {
            continue;
        }
        let (texture, margin) = shadow_texture(painter.ctx(), rect.size(), radius, shadow);
        let local = rect
            .expand2(margin)
            .translate(egui::vec2(shadow.x, shadow.y));
        painter.add(mapped_quad(texture.id(), local, tint, map));
    }
}

/// [`paint_box_shadows_mapped`] for an untransformed element.
pub fn paint_box_shadows(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    shadows: &[BoxShadow],
    opacity: f32,
) {
    paint_box_shadows_mapped(painter, rect, radius, shadows, opacity, &|point| point);
}

/// Tessellate `shape` flat, then move every vertex through `map`: a rounded,
/// stroked or textured rect under a projective transform.
pub fn paint_mapped_rect(
    painter: &egui::Painter,
    shape: egui::epaint::RectShape,
    map: &dyn Fn(Pos2) -> Pos2,
) {
    let ctx = painter.ctx();
    let texture = shape.fill_texture_id();
    let mut tessellator = egui::epaint::Tessellator::new(
        ctx.pixels_per_point(),
        ctx.tessellation_options(|options| *options),
        [1, 1],
        Vec::new(),
    );
    let mut mesh = Mesh::with_texture(texture);
    tessellator.tessellate_rect(&shape.with_round_to_pixels(false), &mut mesh);
    for vertex in &mut mesh.vertices {
        vertex.pos = map(vertex.pos);
    }
    painter.add(mesh);
}

/// Cross-fade weights that approximate an intermediate blur radius from the
/// prepared `levels` (ascending radii; 0 is the sharp image): the two
/// neighbouring levels, as `(lower, upper, upper_weight)`. Exact at a level.
pub fn blur_levels(radius: f32, levels: &[f32]) -> Option<(usize, usize, f32)> {
    let last = levels.len().checked_sub(1)?;
    if radius <= levels[0] {
        return Some((0, 0, 0.));
    }
    if radius >= levels[last] {
        return Some((last, last, 0.));
    }
    let upper = levels.iter().position(|&level| level >= radius)?;
    let lower = upper - 1;
    let span = levels[upper] - levels[lower];
    let weight = if span > 0. {
        (radius - levels[lower]) / span
    } else {
        1.
    };
    Some((lower, upper, weight))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_is_normalised_symmetric_and_three_sigma_wide() {
        let kernel = gaussian_kernel(2.);
        assert_eq!(kernel.len(), 13);
        assert!((kernel.iter().sum::<f32>() - 1.).abs() < 1e-5);
        assert_eq!(kernel[0], kernel[12]);
        assert!(kernel[6] > kernel[5]);
        assert_eq!(gaussian_kernel(0.), vec![1.]);
    }

    #[test]
    fn blur_matches_a_gaussian_edge_and_fades_past_the_element() {
        // A white half-plane: the blurred edge follows the Gaussian CDF.
        let (w, h) = (64, 8);
        let mut image = ColorImage::filled([w, h], Color32::TRANSPARENT);
        for y in 0..h {
            for x in 32..w {
                image.pixels[y * w + x] = Color32::WHITE;
            }
        }
        let blurred = gaussian_blur(&image, [4., 0.], [0, 0]);
        let at = |x: usize| f32::from(blurred.pixels[4 * w + x].a()) / 255.;
        // Pixel 32 is the first covered one: its centre sits half a pixel in.
        assert!((at(32) - 0.55).abs() < 0.02, "{}", at(32));
        assert!((at(36) - 0.87).abs() < 0.02, "{}", at(36));
        assert!(at(20) < 0.01);
        // Transparent beyond the right edge: the edge pixel loses about half.
        assert!((at(63) - 0.55).abs() < 0.02, "{}", at(63));
        // Vertical-only blur leaves rows alone when sigma is zero.
        assert_eq!(gaussian_blur(&image, [0., 0.], [0, 0]).pixels, image.pixels);
        let padded = gaussian_blur(&image, [2., 2.], [3, 5]);
        assert_eq!(padded.size, [w + 6, h + 10]);
        assert!(padded.pixels[0].a() == 0);
        assert!(padded.pixels.iter().all(|p| p.r() <= p.a()));
    }

    #[test]
    fn card_media_blurs_at_the_css_radius_on_a_suitable_grid() {
        let media = ColorImage::filled([568, 320], Color32::WHITE);
        let soft = blur_card_media(&media, 2.3);
        assert_eq!(soft.size, [284, 160]);
        let fine = blur_card_media(&media, 0.6);
        assert_eq!(fine.size, [568, 320]);
        // Centre stays opaque; the transparent edge fades like CSS.
        assert_eq!(soft.pixels[80 * 284 + 142].a(), 255);
        assert!(soft.pixels[80 * 284].a() < 200);
    }

    #[test]
    fn shadow_mask_is_a_blurred_offset_outline_clipped_outside_the_element() {
        let shadow = BoxShadow {
            x: 0.,
            y: 6.,
            blur: 14.,
            spread: 0.,
            color: [0., 0., 0., 0.38],
        };
        let size = egui::vec2(100., 60.);
        let (mask, margin) = shadow_mask(size, 12., &shadow);
        // σ = 7: margin reaches 3σ + 1.
        assert_eq!(margin, Vec2::splat(22.));
        assert_eq!(mask.size, [144, 104]);
        let alpha = |x: usize, y: usize| mask.pixels[y * 144 + x].a();
        // The element (shifted up by the offset) is cut out.
        assert_eq!(alpha(72, 52 - 6), 0);
        // Below the element the offset shadow shows; far corners are clear.
        let below = alpha(72, 22 + 60 + 2);
        assert!(below > 90 && below < 200, "{below}");
        assert_eq!(alpha(0, 0), 0);
        // Above the element, the shadow is fainter than below.
        assert!(alpha(72, 22 - 6 - 3) < below);
        let glow = BoxShadow::glow(3., Color32::from_rgba_unmultiplied(255, 0, 0, 128));
        let (fine, margin) = shadow_mask(egui::vec2(10., 10.), 2., &glow);
        assert_eq!(margin, Vec2::splat(6.));
        assert_eq!(fine.size, [44, 44], "blurs under 4 pt use 2 px per point");
        assert!((glow.color[3] - 128. / 255.).abs() < 1e-6);
    }

    #[test]
    fn blur_levels_cross_fade_between_prepared_radii() {
        let levels = [0., 0.7, 1.1];
        assert_eq!(blur_levels(0., &levels), Some((0, 0, 0.)));
        assert_eq!(blur_levels(1.1, &levels), Some((2, 2, 0.)));
        assert_eq!(blur_levels(2., &levels), Some((2, 2, 0.)));
        let (lower, upper, weight) = blur_levels(0.9, &levels).unwrap();
        assert_eq!((lower, upper), (1, 2));
        assert!((weight - 0.5).abs() < 1e-5);
        assert_eq!(blur_levels(0.35, &levels), Some((0, 1, 0.5)));
        assert_eq!(blur_levels(1., &[]), None);
    }
}
