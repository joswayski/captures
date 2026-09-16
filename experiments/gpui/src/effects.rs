//! Image-fragment dissolve. Timing/poses ported from
//! apps/desktop/ui/src/lib/thumbnailExit.ts, not a generic confetti effect.
use image::{Rgba, RgbaImage, imageops};

#[derive(Clone, Debug)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub dx: f32,
    pub dy: f32,
    pub rotate: f32,
    pub delay: f32,
    pub duration: f32,
}

pub fn particles_from(
    width: f32,
    height: f32,
    origin_x: f32,
    origin_y: f32,
    mut random: impl FnMut() -> f32,
) -> Vec<Particle> {
    let mut cols = (width / 11.).round().clamp(14., 24.);
    let mut rows = (height / 11.).round().clamp(8., 16.);
    if cols * rows > 220. {
        let scale = (220. / (cols * rows)).sqrt();
        cols = (cols * scale).floor().max(10.);
        rows = (rows * scale).floor().max(6.);
    }
    let (cw, ch) = (width / cols, height / rows);
    let (ox, oy) = (origin_x.clamp(0., width), origin_y.clamp(0., height));
    let max_dist = ox.max(width - ox).hypot(oy.max(height - oy)).max(1.);
    let mut result = Vec::new();
    for row in 0..rows as usize {
        for col in 0..cols as usize {
            let (x, y) = (col as f32 * cw, row as f32 * ch);
            let (cx, cy) = (x + cw / 2., y + ch / 2.);
            let wave = (cx - ox).hypot(cy - oy) / max_dist;
            let angle = (cy - oy).atan2(cx - ox);
            let wobble = (angle * 2.7 + wave * 5.5).sin() * 0.07 * wave;
            let scatter = (random() - 0.5) * 0.34 * wave * wave;
            let delay = ((wave + wobble + scatter).clamp(0., 1.12) * 720.
                + random() * (18. + wave * 140.))
                .floor();
            let dx = (cx - ox) / max_dist * (12. + random() * 26.) + (random() - 0.5) * 22.;
            let dy = -36. - random() * 58.;
            let rotate = (random() - 0.5) * 120.;
            let duration = 780. + (random() * 320. + wave * 80.).floor();
            result.push(Particle {
                x,
                y,
                width: cw + 0.55,
                height: ch + 0.55,
                dx,
                dy,
                rotate,
                delay,
                duration,
            });
        }
    }
    result
}

pub fn particles(width: f32, height: f32, random: impl FnMut() -> f32) -> Vec<Particle> {
    particles_from(
        width,
        height,
        57.5_f32.min(width * 0.35),
        22.5_f32.min(height * 0.3),
        random,
    )
}

fn cubic_bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..22 {
        let t = (lo + hi) / 2.;
        let bx = 3. * (1. - t) * (1. - t) * t * x1 + 3. * (1. - t) * t * t * x2 + t * t * t;
        if bx < x {
            lo = t;
        } else {
            hi = t;
        }
    }
    let t = (lo + hi) / 2.;
    3. * (1. - t) * (1. - t) * t * y1 + 3. * (1. - t) * t * t * y2 + t * t * t
}

/// Opacity, translation x/y, degrees, and scale. Easing applies to the whole
/// duration before sampling the CSS/WAAPI keyframes.
pub fn pose(p: &Particle, elapsed_ms: f32) -> [f32; 5] {
    if elapsed_ms <= p.delay {
        return [1., 0., 0., 0., 1.];
    }
    let t = cubic_bezier(
        ((elapsed_ms - p.delay) / p.duration).clamp(0., 1.),
        0.28,
        0.,
        0.12,
        1.,
    );
    let opacity = if t < 0.14 {
        1.
    } else if t < 0.5 {
        1. - (t - 0.14) / 0.36 * 0.28
    } else if t < 0.82 {
        0.72 * (1. - (t - 0.5) / 0.32)
    } else {
        0.
    };
    let (move_fraction, rotation_fraction, scale) = if t <= 0.14 {
        let mix = t / 0.14;
        (mix * 0.06, mix * 0.08, 1. - mix * 0.02)
    } else {
        let mix = (t - 0.14) / 0.86;
        (0.06 + mix * 0.94, 0.08 + mix * 0.92, 0.98 - mix * 0.8)
    };
    [
        opacity,
        p.dx * move_fraction,
        p.dy * move_fraction,
        p.rotate * rotation_fraction,
        scale,
    ]
}

pub const PAD: u32 = 120;
pub const WIDTH: u32 = 284;
pub const HEIGHT: u32 = 160;
const RADIUS: f32 = 12.;
const CHIP_BLUR_PAD: f32 = 8.;

/// CPU rasterization only: excludes GPUI uploads, composition, and presentation.
/// Each repetition samples the complete 2550ms animation at 60Hz.
pub fn benchmark() {
    let source = RgbaImage::from_fn(960, 540, |x, y| {
        Rgba([(x % 256) as u8, (y % 256) as u8, 97, 255])
    });
    let effect = Dissolve::new(&source, 83);
    for frame in 0..153 {
        std::hint::black_box(effect.frame(frame as f32 * 1000. / 60.));
    }
    let mut samples = Vec::new();
    for _ in 0..5 {
        for frame in 0..153 {
            let start = std::time::Instant::now();
            std::hint::black_box(effect.frame(frame as f32 * 1000. / 60.));
            samples.push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "workload": "284x160 source-fragment dissolve, CPU rasterization only; not displayed FPS",
            "samples": samples.len(), "warmup_frames": 153,
            "median_ms": samples[samples.len() / 2], "p95_ms": samples[samples.len() * 95 / 100],
            "max_ms": samples.last(),
        })
    );
}

pub struct Dissolve {
    source: RgbaImage,
    particles: Vec<Particle>,
    chips: Vec<RgbaImage>,
    scale: u32,
}

impl Dissolve {
    pub fn new(image: &RgbaImage, seed: u32) -> Self {
        Self::new_from(image, seed, 57.5, 22.5)
    }

    pub fn new_from(image: &RgbaImage, seed: u32, origin_x: f32, origin_y: f32) -> Self {
        Self::new_from_scaled(image, seed, origin_x, origin_y, 1.)
    }

    /// Builds the production dissolve at the window backing scale.
    pub fn new_from_scaled(
        image: &RgbaImage,
        seed: u32,
        origin_x: f32,
        origin_y: f32,
        scale: f32,
    ) -> Self {
        let mut state = seed.max(1);
        let particles = particles_from(WIDTH as f32, HEIGHT as f32, origin_x, origin_y, || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f64 / (u32::MAX as f64 + 1.)) as f32
        });
        Self::from_particles(image, particles, scale)
    }

    /// Builds a dissolve from shipping-compatible card-local particles.
    ///
    /// `scale` must be positive and finite. Rendering uses `ceil(scale)` backing
    /// pixels per logical pixel so fractional/high-DPI windows retain enough
    /// detail; GPUI displays the raster at its fixed 524×400 logical size.
    /// Integer 1× and 2× inputs therefore remain exact for parity adapters.
    pub fn from_particles(image: &RgbaImage, particles: Vec<Particle>, scale: f32) -> Self {
        assert!(
            scale.is_finite() && scale > 0.,
            "dissolve backing scale must be positive and finite"
        );
        let scale = scale.ceil() as u32;
        let covered = cover_card(image, WIDTH * scale, HEIGHT * scale);

        // The source handoff is transformed before its fixed rounded media
        // shell clips it. It remains above the dust until its fade completes.
        let scaled_width = (WIDTH as f32 * 1.015 * scale as f32).ceil() as u32;
        let scaled_height = (HEIGHT as f32 * 1.015 * scale as f32).ceil() as u32;
        let enlarged = imageops::resize(
            &cover_media(image, WIDTH * scale, HEIGHT * scale),
            scaled_width,
            scaled_height,
            imageops::FilterType::Triangle,
        );
        let mut source = imageops::crop_imm(
            &enlarged,
            (scaled_width - WIDTH * scale) / 2,
            (scaled_height - HEIGHT * scale) / 2,
            WIDTH * scale,
            HEIGHT * scale,
        )
        .to_image();
        source = blur_premultiplied(&source, 2. * scale as f32);
        dim(&mut source);
        apply_rounded_mask(
            &mut source,
            0.,
            0.,
            WIDTH as f32,
            HEIGHT as f32,
            RADIUS,
            scale,
        );

        // Include one transparent logical pixel beyond the far edges for the
        // 0.55px overlap, matching the shipping source canvas.
        let mut dust_source = RgbaImage::new((WIDTH + 1) * scale, (HEIGHT + 1) * scale);
        imageops::overlay(&mut dust_source, &covered, 0, 0);
        apply_rounded_mask(
            &mut dust_source,
            0.,
            0.,
            WIDTH as f32,
            HEIGHT as f32,
            RADIUS,
            scale,
        );
        let chips = particles
            .iter()
            .map(|particle| make_chip(&dust_source, particle, scale))
            .collect();

        Self {
            source,
            particles,
            chips,
            scale,
        }
    }

    /// Returns source image plus clipped dust on a transparent padded canvas.
    pub fn frame(&self, elapsed_ms: f32) -> RgbaImage {
        let width = (WIDTH + PAD * 2) * self.scale;
        let height = (HEIGHT + PAD * 2) * self.scale;
        let mut dust = RgbaImage::new(width, height);
        let mut visible_dust = false;
        for (particle, chip) in self.particles.iter().zip(&self.chips) {
            let [opacity, dx, dy, angle, scale] = pose(particle, elapsed_ms);
            if opacity > 0. {
                visible_dust = true;
                draw_chip(
                    &mut dust, particle, chip, opacity, dx, dy, angle, scale, self.scale,
                );
            }
        }

        // Parent clip/opacity happens after the chips source-over one another.
        if visible_dust {
            let (inset, radius, opacity) = dust_clip(elapsed_ms);
            apply_rounded_mask(
                &mut dust,
                inset,
                inset,
                WIDTH as f32 + 2. * (PAD as f32 - inset),
                HEIGHT as f32 + 2. * (PAD as f32 - inset),
                radius,
                self.scale,
            );
            multiply_alpha(&mut dust, opacity);
        }

        let mut output = dust;
        let opacity = source_opacity(elapsed_ms);
        if opacity > 0. {
            let offset = PAD * self.scale;
            for (x, y, pixel) in self.source.enumerate_pixels() {
                blend_pixel(
                    output.get_pixel_mut(offset + x, offset + y),
                    *pixel,
                    opacity,
                );
            }
        }
        output
    }
}

/// Samples centered `object-fit: cover` geometry into an exact pixel extent.
/// Destination pixel centers map through the floating cover dimensions, avoiding
/// the subpixel shift caused by rounding an intermediate resize before cropping.
pub fn cover_card(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    cover(image, width, height, false)
}

/// Browser image elements snap their fitted destination edges to device pixels
/// before sampling (WebKit RenderImage::paintReplaced). Canvas drawImage does not;
/// the dust fragments must keep using `cover_card` instead.
pub fn cover_media(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    cover(image, width, height, true)
}

fn cover(image: &RgbaImage, width: u32, height: u32, snap: bool) -> RgbaImage {
    let ratio = (width as f32 / image.width().max(1) as f32)
        .max(height as f32 / image.height().max(1) as f32);
    let mut surface_width = image.width() as f32 * ratio;
    let mut surface_height = image.height() as f32 * ratio;
    let mut offset_x = (width as f32 - surface_width) / 2.;
    let mut offset_y = (height as f32 - surface_height) / 2.;
    if snap {
        // WebKit rounds negative half-pixel origins toward positive infinity,
        // like positive absolute coordinates, not Rust's away-from-zero round.
        let round = |value: f32| (value + 0.5).floor();
        surface_width = round(offset_x + surface_width) - round(offset_x);
        surface_height = round(offset_y + surface_height) - round(offset_y);
        offset_x = round(offset_x);
        offset_y = round(offset_y);
    }
    RgbaImage::from_fn(width, height, |x, y| {
        let source_x = (x as f32 + 0.5 - offset_x) * image.width() as f32 / surface_width - 0.5;
        let source_y = (y as f32 + 0.5 - offset_y) * image.height() as f32 / surface_height - 0.5;
        straight_pixel(sample_premultiplied_clamped(image, source_x, source_y))
    })
}

fn make_chip(source: &RgbaImage, particle: &Particle, scale: u32) -> RgbaImage {
    let scale_f = scale as f32;
    let width = ((particle.width + CHIP_BLUR_PAD * 2.) * scale_f).ceil() as u32;
    let height = ((particle.height + CHIP_BLUR_PAD * 2.) * scale_f).ceil() as u32;
    let content_width = (particle.width * scale_f).ceil() as u32;
    let content_height = (particle.height * scale_f).ceil() as u32;
    let pad = (CHIP_BLUR_PAD * scale_f) as u32;
    let mut chip = RgbaImage::new(width, height);
    for y in 0..content_height {
        for x in 0..content_width {
            let sample = sample_premultiplied(
                source,
                particle.x * scale_f + x as f32,
                particle.y * scale_f + y as f32,
            );
            chip.put_pixel(pad + x, pad + y, straight_pixel(sample));
        }
    }
    let mut chip = blur_premultiplied(&chip, 2. * scale_f);
    dim(&mut chip);
    chip
}

#[allow(clippy::too_many_arguments)]
fn draw_chip(
    output: &mut RgbaImage,
    particle: &Particle,
    chip: &RgbaImage,
    opacity: f32,
    dx: f32,
    dy: f32,
    angle: f32,
    pose_scale: f32,
    backing_scale: u32,
) {
    let physical_scale = backing_scale as f32;
    let tile_width = chip.width() as f32 / physical_scale;
    let tile_height = chip.height() as f32 / physical_scale;
    let (sin, cos) = angle.to_radians().sin_cos();
    let half_w = tile_width * pose_scale / 2.;
    let half_h = tile_height * pose_scale / 2.;
    let extent_x = half_w * cos.abs() + half_h * sin.abs();
    let extent_y = half_h * cos.abs() + half_w * sin.abs();
    let center_x = PAD as f32 + particle.x + particle.width / 2. + dx;
    let center_y = PAD as f32 + particle.y + particle.height / 2. + dy;
    let min_x = ((center_x - extent_x) * physical_scale).floor().max(0.) as u32;
    let max_x = ((center_x + extent_x) * physical_scale)
        .ceil()
        .min(output.width() as f32) as u32;
    let min_y = ((center_y - extent_y) * physical_scale).floor().max(0.) as u32;
    let max_y = ((center_y + extent_y) * physical_scale)
        .ceil()
        .min(output.height() as f32) as u32;

    for y in min_y..max_y {
        for x in min_x..max_x {
            let relative_x = (x as f32 + 0.5) / physical_scale - center_x;
            let relative_y = (y as f32 + 0.5) / physical_scale - center_y;
            let local_x = (relative_x * cos + relative_y * sin) / pose_scale + tile_width / 2.;
            let local_y = (-relative_x * sin + relative_y * cos) / pose_scale + tile_height / 2.;
            let sample = sample_premultiplied(
                chip,
                local_x * physical_scale - 0.5,
                local_y * physical_scale - 0.5,
            );
            blend_premultiplied(output.get_pixel_mut(x, y), sample, opacity);
        }
    }
}

fn sample_premultiplied(image: &RgbaImage, x: f32, y: f32) -> [f32; 4] {
    if x < -1. || y < -1. || x > image.width() as f32 || y > image.height() as f32 {
        return [0.; 4];
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = x - x.floor();
    let ty = y - y.floor();
    let mut result = [0.; 4];
    for (sample_x, weight_x) in [(x0, 1. - tx), (x0 + 1, tx)] {
        for (sample_y, weight_y) in [(y0, 1. - ty), (y0 + 1, ty)] {
            if sample_x < 0
                || sample_y < 0
                || sample_x >= image.width() as i32
                || sample_y >= image.height() as i32
            {
                continue;
            }
            let pixel = image.get_pixel(sample_x as u32, sample_y as u32);
            let alpha = f32::from(pixel[3]) / 255.;
            let weight = weight_x * weight_y;
            for channel in 0..3 {
                result[channel] += f32::from(pixel[channel]) / 255. * alpha * weight;
            }
            result[3] += alpha * weight;
        }
    }
    result
}

fn sample_premultiplied_clamped(image: &RgbaImage, x: f32, y: f32) -> [f32; 4] {
    sample_premultiplied(
        image,
        x.clamp(0., image.width().saturating_sub(1) as f32),
        y.clamp(0., image.height().saturating_sub(1) as f32),
    )
}

fn straight_pixel(premultiplied: [f32; 4]) -> Rgba<u8> {
    let alpha = premultiplied[3];
    if alpha <= 0. {
        return Rgba([0, 0, 0, 0]);
    }
    Rgba([
        (premultiplied[0] / alpha * 255.).round().clamp(0., 255.) as u8,
        (premultiplied[1] / alpha * 255.).round().clamp(0., 255.) as u8,
        (premultiplied[2] / alpha * 255.).round().clamp(0., 255.) as u8,
        (alpha * 255.).round().clamp(0., 255.) as u8,
    ])
}

fn blend_premultiplied(destination: &mut Rgba<u8>, source: [f32; 4], opacity: f32) {
    let source_alpha = source[3] * opacity;
    if source_alpha <= 0. {
        return;
    }
    let destination_alpha = f32::from(destination[3]) / 255.;
    let output_alpha = source_alpha + destination_alpha * (1. - source_alpha);
    for channel in 0..3 {
        let destination_channel = f32::from(destination[channel]) / 255. * destination_alpha;
        let output_channel = source[channel] * opacity + destination_channel * (1. - source_alpha);
        destination[channel] = (output_channel / output_alpha * 255.)
            .round()
            .clamp(0., 255.) as u8;
    }
    destination[3] = (output_alpha * 255.).round().clamp(0., 255.) as u8;
}

fn blend_pixel(destination: &mut Rgba<u8>, source: Rgba<u8>, opacity: f32) {
    let alpha = f32::from(source[3]) / 255.;
    blend_premultiplied(
        destination,
        [
            f32::from(source[0]) / 255. * alpha,
            f32::from(source[1]) / 255. * alpha,
            f32::from(source[2]) / 255. * alpha,
            alpha,
        ],
        opacity,
    );
}

fn blur_premultiplied(image: &RgbaImage, sigma: f32) -> RgbaImage {
    let mut premultiplied = image.clone();
    for pixel in premultiplied.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in 0..3 {
            pixel[channel] = ((u16::from(pixel[channel]) * alpha + 127) / 255) as u8;
        }
    }
    let mut blurred = imageops::blur(&premultiplied, sigma);
    for pixel in blurred.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        if alpha == 0 {
            *pixel = Rgba([0, 0, 0, 0]);
        } else {
            for channel in 0..3 {
                pixel[channel] =
                    ((u16::from(pixel[channel]) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    blurred
}

fn dim(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] = ((u16::from(pixel[channel]) * 128) / 255) as u8;
        }
    }
}

fn multiply_alpha(image: &mut RgbaImage, opacity: f32) {
    for pixel in image.pixels_mut() {
        pixel[3] = (f32::from(pixel[3]) * opacity).round().clamp(0., 255.) as u8;
    }
}

fn rounded_coverage(
    x: f32,
    y: f32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    radius: f32,
) -> f32 {
    if x < left || y < top || x >= left + width || y >= top + height {
        return 0.;
    }
    let radius = radius.min(width / 2.).min(height / 2.).max(0.);
    if radius == 0. {
        return 1.;
    }
    let nearest_x = x.clamp(left + radius, left + width - radius);
    let nearest_y = y.clamp(top + radius, top + height - radius);
    (radius + 0.5 - (x - nearest_x).hypot(y - nearest_y)).clamp(0., 1.)
}

fn apply_rounded_mask(
    image: &mut RgbaImage,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    radius: f32,
    scale: u32,
) {
    let scale = scale as f32;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let coverage = rounded_coverage(
            (x as f32 + 0.5) / scale,
            (y as f32 + 0.5) / scale,
            left,
            top,
            width,
            height,
            radius,
        );
        pixel[3] = (f32::from(pixel[3]) * coverage).round().clamp(0., 255.) as u8;
    }
}

fn source_opacity(elapsed_ms: f32) -> f32 {
    if elapsed_ms <= 110. {
        1.
    } else if elapsed_ms < 550. {
        1. - cubic_bezier((elapsed_ms - 110.) / 440., 0.22, 0.1, 0.25, 1.)
    } else {
        0.
    }
}

fn dust_clip(elapsed_ms: f32) -> (f32, f32, f32) {
    if elapsed_ms <= 204. {
        (PAD as f32, RADIUS, 1.)
    } else if elapsed_ms < 1_785. {
        let progress = cubic_bezier((elapsed_ms - 204.) / 1_581., 0.33, 0., 0.2, 1.);
        (PAD as f32 * (1. - progress), RADIUS * (1. - progress), 1.)
    } else if elapsed_ms < 2_295. {
        let opacity = 1. - cubic_bezier((elapsed_ms - 1_785.) / 510., 0.33, 0., 0.2, 1.);
        (0., 0., opacity)
    } else {
        (0., 0., 0.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stationary_particle(x: f32, width: f32) -> Particle {
        Particle {
            x,
            y: 60.,
            width,
            height: 20.,
            dx: 0.,
            dy: 0.,
            rotate: 0.,
            delay: 10_000.,
            duration: 1_000.,
        }
    }

    #[test]
    fn chips_are_source_fragments_not_confetti() {
        let source = RgbaImage::from_fn(320, 190, |x, y| Rgba([x as u8, y as u8, 17, 255]));
        let effect = Dissolve::new(&source, 83);
        let first = effect.frame(0.);
        assert!(first.get_pixel(PAD + 100, PAD + 80)[2] <= 9);
        assert_eq!(first.get_pixel(PAD, PAD)[3], 0);
        let flying = effect.frame(600.);
        assert!(
            flying
                .enumerate_pixels()
                .any(|(_, y, pixel)| y < PAD && pixel[3] > 0)
        );
        assert!(effect.frame(2550.).pixels().all(|pixel| pixel[3] == 0));
    }

    #[test]
    fn radial_wave_uses_shipping_grid_and_upward_motion() {
        let chips = particles(284., 160., || 0.5);
        // Independent reference: buildThumbnailDustParticles(284, 160,
        // { random: () => 0.5 }) in the shipping TypeScript implementation.
        assert_eq!(chips.len(), 198);
        assert_eq!(chips[0].delay, 160.);
        assert_eq!(chips[0].duration, 955.);
        let reference = [0.052_838_67, -3.639765, -50.543182, 0., 0.3692873];
        for (actual, expected) in pose(&chips[0], 600.).into_iter().zip(reference) {
            assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
        }
        assert!(
            chips
                .iter()
                .all(|particle| particle.dy == -65. && particle.rotate == 0.)
        );
        assert!(chips[0].delay < chips.last().unwrap().delay);
        assert_eq!(pose(&chips[0], chips[0].delay), [1., 0., 0., 0., 1.]);
    }

    #[test]
    fn mirrored_delete_control_mirrors_the_asymmetric_wave() {
        let left = particles_from(284., 160., 22.5, 22.5, || 0.5);
        let right = particles_from(284., 160., 261.5, 22.5, || 0.5);
        assert!(left[0].delay < right[0].delay);
        assert!(left.last().unwrap().delay > right.last().unwrap().delay);
        // The 220-chip cap resolves this card to 18 columns. Compare mirrored
        // cells, not two cells on the same side of the card.
        assert!((left[0].dx + right[17].dx).abs() < 0.001);
    }

    #[test]
    fn source_fade_restarts_easing_after_its_hold_keyframe() {
        assert_eq!(source_opacity(110.), 1.);
        assert!((source_opacity(420.) - 0.056_480_1).abs() < 0.000_01);
        assert_eq!(source_opacity(550.), 0.);
        // Independently solving cubic(.22,.1,.25,1) at (220-110)/440.
        assert!((source_opacity(220.) - 0.564_000_8).abs() < 0.000_01);
    }

    #[test]
    fn dust_clip_and_opacity_use_independent_keyframe_intervals() {
        assert_eq!(dust_clip(204.), (120., 12., 1.));
        let opening = dust_clip(600.);
        let expected = cubic_bezier((600. - 204.) / 1_581., 0.33, 0., 0.2, 1.);
        assert!((opening.0 - 120. * (1. - expected)).abs() < 0.000_1);
        assert!((opening.1 - 12. * (1. - expected)).abs() < 0.000_1);
        assert_eq!(opening.2, 1.);
        assert_eq!(dust_clip(1_785.), (0., 0., 1.));
        assert_eq!(dust_clip(2_295.), (0., 0., 0.));
    }

    #[test]
    fn chips_are_cut_before_blur_and_keep_transparent_padding() {
        let source = RgbaImage::from_fn(WIDTH, HEIGHT, |x, _| {
            if x < WIDTH / 2 {
                Rgba([240, 20, 10, 255])
            } else {
                Rgba([10, 20, 240, 255])
            }
        });
        let effect = Dissolve::from_particles(
            &source,
            vec![stationary_particle(0., WIDTH as f32 / 2.)],
            1.,
        );
        let frame = effect.frame(600.);
        let fringe = frame.get_pixel(PAD + WIDTH / 2 + 2, PAD + 70);
        assert!(
            fringe[3] > 0,
            "per-chip blur must extend beyond the cut edge"
        );
        assert!(
            fringe[0] > fringe[2] * 4,
            "neighboring blue must not bleed in"
        );
    }

    #[test]
    fn overlapping_chips_composite_source_over_instead_of_overwriting() {
        let source = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([180, 80, 20, 128]));
        let particle = stationary_particle(80., 30.);
        let single = Dissolve::from_particles(&source, vec![particle.clone()], 1.).frame(600.);
        let double =
            Dissolve::from_particles(&source, vec![particle.clone(), particle], 1.).frame(600.);
        let sample = (PAD + 95, PAD + 70);
        assert!(double.get_pixel(sample.0, sample.1)[3] > single.get_pixel(sample.0, sample.1)[3]);
    }

    #[test]
    fn backing_scale_changes_pixels_not_logical_extent() {
        let source = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([200, 100, 40, 255]));
        let particle = stationary_particle(80., 30.);
        let one = Dissolve::from_particles(&source, vec![particle.clone()], 1.).frame(600.);
        let two = Dissolve::from_particles(&source, vec![particle], 2.).frame(600.);
        assert_eq!(one.dimensions(), (524, 400));
        assert_eq!(two.dimensions(), (1048, 800));
        let logical = (PAD + 95, PAD + 70);
        assert!(
            (i16::from(one.get_pixel(logical.0, logical.1)[3])
                - i16::from(two.get_pixel(logical.0 * 2, logical.1 * 2)[3]))
            .abs()
                <= 3
        );
    }

    #[test]
    fn cover_card_samples_the_exact_floating_cover_geometry() {
        let source = RgbaImage::from_fn(3, 2, |x, _| match x {
            0 => Rgba([255, 0, 0, 255]),
            1 => Rgba([0, 255, 0, 255]),
            _ => Rgba([0, 0, 255, 255]),
        });
        // 4×4 cover scales the 3×2 source by 2 and crops one physical pixel
        // from each side. Destination centers map to source x=.25 and 1.75.
        let covered = cover_card(&source, 4, 4);
        assert_eq!(covered.get_pixel(0, 2).0, [191, 64, 0, 255]);
        assert_eq!(covered.get_pixel(3, 2).0, [0, 64, 191, 255]);
    }

    #[test]
    fn image_cover_snaps_both_edges_but_canvas_cover_does_not() {
        let source = RgbaImage::from_fn(3, 2, |x, _| match x {
            0 => Rgba([255, 0, 0, 255]),
            1 => Rgba([0, 255, 0, 255]),
            _ => Rgba([0, 0, 255, 255]),
        });
        // Exact bounds [-1.25, 6.25] snap to [-1, 6], not a centered
        // integer resize to width 8. Pixel centers sample x=1/7 and 13/7.
        let media = cover_media(&source, 5, 5);
        assert_eq!(media.get_pixel(0, 2).0, [219, 36, 0, 255]);
        assert_eq!(media.get_pixel(4, 2).0, [0, 36, 219, 255]);
        assert_eq!(
            cover_card(&source, 5, 5).get_pixel(0, 2).0,
            [204, 51, 0, 255]
        );
        // Negative halfway origins round toward positive infinity: -1.5 → -1.
        let half = cover_media(&source, 6, 6);
        assert_eq!(half.get_pixel(0, 2).0, [255, 0, 0, 255]);
        assert_eq!(half.get_pixel(5, 2).0, [0, 85, 170, 255]);
        let portrait = imageops::rotate90(&source);
        let media = cover_media(&portrait, 5, 5);
        assert_eq!(media.get_pixel(2, 0).0, [219, 36, 0, 255]);
        assert_eq!(media.get_pixel(2, 4).0, [0, 36, 219, 255]);
    }

    #[test]
    fn fractional_and_high_dpi_scales_rasterize_at_the_ceiling() {
        let source = RgbaImage::new(WIDTH, HEIGHT);
        let fractional = Dissolve::from_particles(&source, Vec::new(), 1.5).frame(0.);
        let high_dpi = Dissolve::from_particles(&source, Vec::new(), 2.25).frame(0.);
        assert_eq!(fractional.dimensions(), (1048, 800));
        assert_eq!(high_dpi.dimensions(), (1572, 1200));
    }

    #[test]
    #[should_panic(expected = "positive and finite")]
    fn malformed_backing_scale_is_rejected() {
        let source = RgbaImage::new(WIDTH, HEIGHT);
        let _ = Dissolve::from_particles(&source, Vec::new(), f32::NAN);
    }

    #[test]
    fn source_is_clipped_to_the_rounded_media_shell() {
        let source = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([240, 160, 80, 255]));
        let effect = Dissolve::from_particles(&source, Vec::new(), 1.);
        let frame = effect.frame(0.);
        assert_eq!(frame.get_pixel(PAD, PAD)[3], 0);
        assert_eq!(frame.get_pixel(PAD + WIDTH / 2, PAD + HEIGHT / 2)[3], 255);
        let edge_alpha = frame.get_pixel(PAD, PAD + 8)[3];
        assert!(edge_alpha > 0 && edge_alpha < 255);
    }
}
