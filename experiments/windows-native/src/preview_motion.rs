//! Native translation of the shipping `thumbnailExit.ts` dust contract.
//! Keep the differential test: similar-looking easing is not the same animation.
use image::{Rgba, RgbaImage, imageops};
use serde::Serialize;

pub const PAD: f32 = 120.0;
pub const MEDIA_WIDTH: f32 = 340.0;
pub const MEDIA_HEIGHT: f32 = 162.0;
const BLUR_PAD: u32 = 8;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Particle {
    pub source_left: f64,
    pub source_top: f64,
    pub width: f64,
    pub height: f64,
    pub dx: f64,
    pub dy: f64,
    pub rotate: f64,
    pub delay_ms: f64,
    pub duration_ms: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Visual {
    pub opacity: f64,
    pub dx: f64,
    pub dy: f64,
    pub rotate: f64,
    pub scale: f64,
}

pub fn particles(
    width: f64,
    height: f64,
    origin: (f64, f64),
    mut random: impl FnMut() -> f64,
) -> Vec<Particle> {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let mut cols = (width / 11.0).round().clamp(14.0, 24.0);
    let mut rows = (height / 11.0).round().clamp(8.0, 16.0);
    if cols * rows > 220.0 {
        let scale = (220.0 / (cols * rows)).sqrt();
        cols = (cols * scale).floor().max(10.0);
        rows = (rows * scale).floor().max(6.0);
    }
    let max_dist = origin
        .0
        .max(width - origin.0)
        .hypot(origin.1.max(height - origin.1))
        .max(1.0);
    let mut result = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows as usize {
        for col in 0..cols as usize {
            let left = col as f64 * width / cols;
            let top = row as f64 * height / rows;
            let cx = left + width / cols / 2.0;
            let cy = top + height / rows / 2.0;
            let wave = (cx - origin.0).hypot(cy - origin.1) / max_dist;
            let angle = (cy - origin.1).atan2(cx - origin.0);
            let wobble = (angle * 2.7 + wave * 5.5).sin() * 0.07 * wave;
            let scatter = (random() - 0.5) * 0.34 * wave * wave;
            let delay = (wave + wobble + scatter).clamp(0.0, 1.12);
            let jitter = random() * (18.0 + wave * 140.0);
            let dx =
                (cx - origin.0) / max_dist * (12.0 + random() * 26.0) + (random() - 0.5) * 22.0;
            let dy = -36.0 - random() * 58.0;
            result.push(Particle {
                source_left: left,
                source_top: top,
                width: width / cols + 0.55,
                height: height / rows + 0.55,
                dx,
                dy,
                rotate: (random() - 0.5) * 120.0,
                delay_ms: (delay * 720.0 + jitter).floor(),
                duration_ms: 780.0 + (random() * 320.0 + wave * 80.0).floor(),
            });
        }
    }
    result
}

fn sample(t: f64, a: f64, b: f64) -> f64 {
    3.0 * (1.0 - t).powi(2) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t
}

fn ease(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let mut t = x;
    for _ in 0..8 {
        let delta = sample(t, 0.28, 0.12) - x;
        if delta.abs() < 1e-6 {
            return sample(t, 0.0, 1.0);
        }
        let derivative = 3.0 * (1.0 - t).powi(2) * 0.28
            + 6.0 * (1.0 - t) * t * (0.12 - 0.28)
            + 3.0 * t * t * (1.0 - 0.12);
        if derivative.abs() < 1e-6 {
            break;
        }
        t = (t - delta / derivative).clamp(0.0, 1.0);
    }
    let (mut low, mut high) = (0.0, 1.0);
    t = x;
    for _ in 0..12 {
        if sample(t, 0.28, 0.12) < x {
            low = t;
        } else {
            high = t;
        }
        t = (low + high) / 2.0;
    }
    sample(t, 0.0, 1.0)
}

impl Particle {
    pub fn visual(&self, elapsed_ms: f64) -> Visual {
        let t = ease(((elapsed_ms - self.delay_ms) / self.duration_ms.max(1.0)).clamp(0.0, 1.0));
        let opacity = if t <= 0.14 {
            1.0
        } else if t <= 0.5 {
            1.0 - (t - 0.14) / 0.36 * 0.28
        } else if t <= 0.82 {
            0.72 * (1.0 - (t - 0.5) / 0.32)
        } else {
            0.0
        };
        let (travel, turn, scale) = if t <= 0.14 {
            let mix = t / 0.14;
            (0.06 * mix, 0.08 * mix, 1.0 - 0.02 * mix)
        } else {
            let mix = (t - 0.14) / 0.86;
            (0.06 + 0.94 * mix, 0.08 + 0.92 * mix, 0.98 - 0.8 * mix)
        };
        Visual {
            opacity,
            dx: self.dx * travel,
            dy: self.dy * travel,
            rotate: self.rotate * turn,
            scale,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Atlas {
    pub image: RgbaImage,
    pub columns: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl Atlas {
    /// Filter once at deletion, not once per chip per frame. Blur premultiplied
    /// pixels so transparent colored source pixels cannot create bright halos.
    pub fn new(source: &RgbaImage, width: u32, height: u32, particles: &[Particle]) -> Self {
        // Crop before resizing: a very wide one-pixel strip must not allocate
        // a gigantic intermediate just to fill a thumbnail.
        let scale =
            (width as f64 / source.width() as f64).max(height as f64 / source.height() as f64);
        let crop_width = (width as f64 / scale).round().max(1.0) as u32;
        let crop_height = (height as f64 / scale).round().max(1.0) as u32;
        let crop = imageops::crop_imm(
            source,
            (source.width() - crop_width) / 2,
            (source.height() - crop_height) / 2,
            crop_width,
            crop_height,
        );
        let mut card = imageops::resize(
            &crop.to_image(),
            width,
            height,
            imageops::FilterType::Triangle,
        );
        let radius = 12.0_f64.min(width as f64 / 2.0).min(height as f64 / 2.0);
        for (x, y, pixel) in card.enumerate_pixels_mut() {
            let dx = (radius - (x as f64 + 0.5).min(width as f64 - x as f64 - 0.5)).max(0.0);
            let dy = (radius - (y as f64 + 0.5).min(height as f64 - y as f64 - 0.5)).max(0.0);
            let coverage = (radius + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0);
            let alpha = f64::from(pixel[3]) * coverage;
            for channel in 0..3 {
                pixel[channel] = (f64::from(pixel[channel]) * alpha / 255.0 * 0.5).round() as u8;
            }
            pixel[3] = alpha.round() as u8;
        }
        let columns = (particles.len() as f64).sqrt().ceil() as u32;
        let cell_width = particles
            .iter()
            .map(|p| p.width.ceil() as u32)
            .max()
            .unwrap_or(1)
            + BLUR_PAD * 2;
        let cell_height = particles
            .iter()
            .map(|p| p.height.ceil() as u32)
            .max()
            .unwrap_or(1)
            + BLUR_PAD * 2;
        let mut tiles = RgbaImage::new(
            columns * cell_width,
            (particles.len() as u32).div_ceil(columns) * cell_height,
        );
        for (index, particle) in particles.iter().enumerate() {
            for y in 0..particle.height.ceil() as u32 {
                for x in 0..particle.width.ceil() as u32 {
                    let sx = particle.source_left.round() as u32 + x;
                    let sy = particle.source_top.round() as u32 + y;
                    if sx < width && sy < height {
                        tiles.put_pixel(
                            index as u32 % columns * cell_width + BLUR_PAD + x,
                            index as u32 / columns * cell_height + BLUR_PAD + y,
                            *card.get_pixel(sx, sy),
                        );
                    }
                }
            }
        }
        let mut image = imageops::blur(&tiles, 2.0);
        for pixel in image.pixels_mut() {
            if pixel[3] == 0 {
                *pixel = Rgba([0; 4]);
            } else {
                for channel in 0..3 {
                    pixel[channel] =
                        (u32::from(pixel[channel]) * 255 / u32::from(pixel[3])).min(255) as u8;
                }
            }
        }
        Self {
            image,
            columns,
            cell_width,
            cell_height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "release-only microbenchmark; run without builds or other workloads"]
    fn benchmark_preview_dust() {
        use std::{hint::black_box, time::Instant};
        let source = RgbaImage::from_fn(3840, 2160, |x, y| {
            Rgba([(x % 251) as u8, (y % 239) as u8, ((x + y) % 233) as u8, 255])
        });
        let chips = particles(
            MEDIA_WIDTH.into(),
            MEDIA_HEIGHT.into(),
            (297.5, 181.0),
            || 0.5,
        );
        let warm = Atlas::new(&source, MEDIA_WIDTH as u32, MEDIA_HEIGHT as u32, &chips);
        let bytes = warm.image.as_raw().len();
        drop(warm);
        let mut atlas_ms = Vec::new();
        let mut frame_us = Vec::new();
        for _ in 0..6 {
            let start = Instant::now();
            black_box(Atlas::new(
                black_box(&source),
                MEDIA_WIDTH as u32,
                MEDIA_HEIGHT as u32,
                &chips,
            ));
            atlas_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            for cycle in 0..1000 {
                for frame in 0..120 {
                    for chip in &chips {
                        black_box(
                            chip.visual(black_box(((frame + cycle) % 120) as f64 * 1000.0 / 60.0)),
                        );
                    }
                }
            }
            frame_us.push(start.elapsed().as_secs_f64() * 1_000_000.0 / 120_000.0);
        }
        println!(
            "{}",
            serde_json::json!({"workload":"Windows native dust model on Linux; no HWND, GPU, image upload or window-region work", "source":[3840,2160], "card":[MEDIA_WIDTH,MEDIA_HEIGHT], "particle_count":chips.len(), "atlas_pixel_bytes":bytes, "atlas_prepare_ms":atlas_ms,"motion_frame_us":frame_us})
        );
    }

    #[test]
    fn motion_matches_the_shipping_typescript_for_every_chip_and_pose() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let script = r#"
          import {buildThumbnailDustParticles, thumbnailDustVisualAt} from './apps/desktop/ui/src/lib/thumbnailExit.ts';
          const out = [];
          for (const [w,h,x,y] of [[284,160,22.5,22.5],[340,162,282.5,22.5],[97,301,57.5,22.5]]) {
            let seed = 13;
            const random = () => ((seed = (Math.imul(seed,1664525)+1013904223)>>>0) / 4294967296);
            const particles = buildThumbnailDustParticles(w,h,{originX:x,originY:y,random});
            out.push(particles.map(p => ({particle:p, poses:[0,140,420,720,1100,1600,2400].map(t=>thumbnailDustVisualAt(p,t))})));
          }
          process.stdout.write(JSON.stringify(out));
        "#;
        let output = std::process::Command::new("node")
            .current_dir(root)
            .args(["--input-type=module", "-e", script])
            .output()
            .expect("Node.js 24 is required for the shipping/native differential test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reference: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for (case, (w, h, x, y)) in [
            (284.0, 160.0, 22.5, 22.5),
            (340.0, 162.0, 282.5, 22.5),
            (97.0, 301.0, 57.5, 22.5),
        ]
        .into_iter()
        .enumerate()
        {
            let mut seed = 13_u32;
            let chips = particles(w, h, (x, y), || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                f64::from(seed) / 4294967296.0
            });
            assert_eq!(chips.len(), reference[case].as_array().unwrap().len());
            for (index, chip) in chips.iter().enumerate() {
                for (field, value) in serde_json::to_value(chip).unwrap().as_object().unwrap() {
                    assert!(
                        (value.as_f64().unwrap()
                            - reference[case][index]["particle"][field].as_f64().unwrap())
                        .abs()
                            < 1e-8,
                        "case {case} chip {index} {field}"
                    );
                }
                for (time_index, time) in [0.0, 140.0, 420.0, 720.0, 1100.0, 1600.0, 2400.0]
                    .into_iter()
                    .enumerate()
                {
                    for (field, value) in serde_json::to_value(chip.visual(time))
                        .unwrap()
                        .as_object()
                        .unwrap()
                    {
                        assert!(
                            (value.as_f64().unwrap()
                                - reference[case][index]["poses"][time_index][field]
                                    .as_f64()
                                    .unwrap())
                            .abs()
                                < 1e-8,
                            "case {case} chip {index} at {time}: {field}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn atlas_crops_cover_rounds_corners_and_blurs_slices_with_transparent_padding() {
        let mut source = RgbaImage::from_pixel(568, 160, Rgba([255, 0, 0, 255]));
        for y in 0..160 {
            for x in 142..426 {
                source.put_pixel(x, y, Rgba([0, 200, 80, 255]));
            }
        }
        let chips = particles(284.0, 160.0, (22.5, 22.5), || 0.5);
        let atlas = Atlas::new(&source, 284, 160, &chips);
        assert!(atlas.image.as_raw().len() < 1024 * 1024);
        // Center crop contains only green: the red margins must never be sampled.
        assert!(atlas.image.pixels().all(|p| p[0] == 0));
        let index = chips.len() / 2;
        let x = index as u32 % atlas.columns * atlas.cell_width + BLUR_PAD;
        let y = index as u32 / atlas.columns * atlas.cell_height + BLUR_PAD;
        let center = atlas.image.get_pixel(x + 6, y + 6);
        assert!((95..=102).contains(&center[1]));
        assert!((36..=42).contains(&center[2]));
        assert!(
            atlas.image.get_pixel(x - 2, y + 6)[3] > 0,
            "blur extends beyond the cut slice"
        );
        assert_eq!(atlas.image.get_pixel(0, 0)[3], 0);
        assert!(
            atlas.image.get_pixel(BLUR_PAD, BLUR_PAD)[3] < 10,
            "rounded card corner must stay empty"
        );
    }
}
