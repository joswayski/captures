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

pub fn particles(width: f32, height: f32, mut random: impl FnMut() -> f32) -> Vec<Particle> {
    let mut cols = (width / 11.).round().clamp(14., 24.);
    let mut rows = (height / 11.).round().clamp(8., 16.);
    if cols * rows > 220. {
        let scale = (220. / (cols * rows)).sqrt();
        cols = (cols * scale).floor().max(10.);
        rows = (rows * scale).floor().max(6.);
    }
    let (cw, ch) = (width / cols, height / rows);
    let (ox, oy) = (57.5_f32.min(width * 0.35), 22.5_f32.min(height * 0.3));
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

fn bezier(x: f32) -> f32 {
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..22 {
        let t = (lo + hi) / 2.;
        let bx = 3. * (1. - t) * (1. - t) * t * 0.28 + 3. * (1. - t) * t * t * 0.12 + t * t * t;
        if bx < x {
            lo = t;
        } else {
            hi = t;
        }
    }
    let t = (lo + hi) / 2.;
    3. * (1. - t) * t * t + t * t * t
}

/// opacity, translation x/y, degrees, scale. Easing applies to the whole
/// duration before sampling the CSS/WAAPI keyframes.
pub fn pose(p: &Particle, elapsed_ms: f32) -> [f32; 5] {
    if elapsed_ms <= p.delay {
        return [1., 0., 0., 0., 1.];
    }
    let t = bezier(((elapsed_ms - p.delay) / p.duration).clamp(0., 1.));
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
}
impl Dissolve {
    pub fn new(image: &RgbaImage, seed: u32) -> Self {
        let source = imageops::resize(
            image,
            ((image.width() as f32
                * (WIDTH as f32 / image.width() as f32).max(HEIGHT as f32 / image.height() as f32))
            .ceil()) as u32,
            ((image.height() as f32
                * (WIDTH as f32 / image.width() as f32).max(HEIGHT as f32 / image.height() as f32))
            .ceil()) as u32,
            imageops::FilterType::Triangle,
        );
        let mut source = imageops::crop_imm(
            &source,
            (source.width() - WIDTH) / 2,
            (source.height() - HEIGHT) / 2,
            WIDTH,
            HEIGHT,
        )
        .to_image();
        // Keep rounded source corners throughout the flight; no square chips
        // reappear outside the card when fragments separate.
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            let dx = (12. - (x as f32 + 0.5).min(WIDTH as f32 - x as f32 - 0.5)).max(0.);
            let dy = (12. - (y as f32 + 0.5).min(HEIGHT as f32 - y as f32 - 0.5)).max(0.);
            if dx.hypot(dy) > 12. {
                pixel.0[3] = 0;
            }
        }
        let mut state = seed.max(1);
        let particles = particles(WIDTH as f32, HEIGHT as f32, || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f64 / (u32::MAX as f64 + 1.)) as f32
        });
        Self { source, particles }
    }
    pub fn frame(&self, elapsed_ms: f32) -> RgbaImage {
        let mut output = RgbaImage::new(WIDTH + PAD * 2, HEIGHT + PAD * 2);
        for p in &self.particles {
            let [alpha, dx, dy, angle, scale] = pose(p, elapsed_ms);
            if alpha <= 0. {
                continue;
            }
            let (sin, cos) = angle.to_radians().sin_cos();
            let half_w = p.width * scale / 2.;
            let half_h = p.height * scale / 2.;
            let ext_x = half_w * cos.abs() + half_h * sin.abs();
            let ext_y = half_h * cos.abs() + half_w * sin.abs();
            let cx = PAD as f32 + p.x + p.width / 2. + dx;
            let cy = PAD as f32 + p.y + p.height / 2. + dy;
            for y in (cy - ext_y).floor().max(0.) as u32
                ..(cy + ext_y).ceil().min(output.height() as f32) as u32
            {
                for x in (cx - ext_x).floor().max(0.) as u32
                    ..(cx + ext_x).ceil().min(output.width() as f32) as u32
                {
                    let (rx, ry) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                    let sx = (rx * cos + ry * sin) / scale + p.width / 2.;
                    let sy = (-rx * sin + ry * cos) / scale + p.height / 2.;
                    if sx < 0. || sy < 0. || sx >= p.width || sy >= p.height {
                        continue;
                    }
                    let source_x = (p.x + sx) as u32;
                    let source_y = (p.y + sy) as u32;
                    if source_x >= WIDTH || source_y >= HEIGHT {
                        continue;
                    }
                    let pixel = self.source.get_pixel(source_x, source_y);
                    output.put_pixel(
                        x,
                        y,
                        Rgba([
                            pixel[0],
                            pixel[1],
                            pixel[2],
                            (f32::from(pixel[3]) * alpha) as u8,
                        ]),
                    );
                }
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chips_are_source_fragments_not_confetti() {
        let source = RgbaImage::from_fn(320, 190, |x, y| Rgba([x as u8, y as u8, 17, 255]));
        let effect = Dissolve::new(&source, 83);
        let first = effect.frame(0.);
        assert_eq!(first.get_pixel(PAD + 100, PAD + 80)[2], 17);
        assert_eq!(first.get_pixel(PAD, PAD)[3], 0);
        let flying = effect.frame(600.);
        assert!(
            flying
                .enumerate_pixels()
                .any(|(_, y, p)| y < PAD && p[3] > 0)
        );
        assert!(effect.frame(2550.).pixels().all(|p| p[3] == 0));
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
        assert!(chips.iter().all(|p| p.dy == -65. && p.rotate == 0.));
        assert!(chips[0].delay < chips.last().unwrap().delay);
        assert_eq!(pose(&chips[0], chips[0].delay), [1., 0., 0., 0., 1.]);
    }
}
