//! Presentation-only strokes of the actual shaped atlas glyphs. This keeps
//! ligatures, fallback faces and the text field's mesh/caret geometry intact;
//! document rendering and export still use the shared contour renderer.
use eframe::egui;
use std::{collections::BTreeMap, collections::VecDeque, sync::Arc};

// Keep common type sizes (including 128pt at 2×) on the direct disk: composing
// eight fractional filters preserves width but softens their antialiased edges.
const DIRECT_DISK_RADIUS: f32 = 12.;

struct Glyph {
    alpha: Vec<u8>,
    texture: egui::TextureHandle,
}

#[derive(Default)]
pub(super) struct Outline {
    job: Option<Arc<egui::text::LayoutJob>>,
    pixels_per_point: f32,
    width: f32,
    glyphs: BTreeMap<([u16; 2], [u16; 2]), Glyph>,
}

impl Outline {
    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        galley: &Arc<egui::Galley>,
        origin: egui::Pos2,
        width: f32,
        color: egui::Color32,
        transform: impl Fn(egui::Pos2) -> egui::Pos2,
    ) {
        // Selection decorates a clone of the galley, not its layout job. Keep
        // the job/raster scale rather than rebuilding on selection or blink.
        let changed = self
            .job
            .as_ref()
            .is_none_or(|job| !Arc::ptr_eq(job, &galley.job))
            || self.pixels_per_point != galley.pixels_per_point
            || self.width != width;
        if self.pixels_per_point != galley.pixels_per_point || self.width != width {
            self.glyphs.clear();
        }
        // On a changed layout validate pixels before reusing a stroke: atlas
        // coordinates can be recycled. Retain only this layout's visible ink,
        // so typing does not accumulate an unbounded historical glyph cache.
        let mut previous = if changed {
            std::mem::take(&mut self.glyphs)
        } else {
            BTreeMap::new()
        };
        self.job = Some(galley.job.clone());
        self.pixels_per_point = galley.pixels_per_point;
        self.width = width;
        let radius = width * galley.pixels_per_point / 2.;
        let pad = padding(radius);
        let mut atlas = None;
        for row in &galley.rows {
            for glyph in &row.glyphs {
                let uv = glyph.uv_rect;
                if uv.is_nothing() {
                    continue; // Spaces and shaped-cluster continuation slots.
                }
                let bitmap = [
                    (uv.max[0] - uv.min[0]) as usize,
                    (uv.max[1] - uv.min[1]) as usize,
                ];
                let vertices = &row.visuals.mesh.vertices
                    [glyph.first_vertex as usize..glyph.first_vertex as usize + 4];
                // Use egui's already rounded (and possibly sheared) mesh,
                // not reconstructed baseline positions. Only expand its ink.
                let dx = (vertices[1].pos - vertices[0].pos) / bitmap[0] as f32;
                let dy = (vertices[2].pos - vertices[0].pos) / bitmap[1] as f32;
                let start =
                    origin + row.pos.to_vec2() + vertices[0].pos.to_vec2() - (dx + dy) * pad as f32;
                let x = dx * (bitmap[0] + 2 * pad) as f32;
                let y = dy * (bitmap[1] + 2 * pad) as f32;
                let points = [start, start + x, start + y, start + x + y].map(&transform);
                if !painter
                    .clip_rect()
                    .intersects(egui::Rect::from_points(&points))
                {
                    continue;
                }
                let texture = self.glyphs.entry((uv.min, uv.max)).or_insert_with(|| {
                    // Snapshot lazily, once per rebuild. Reading image() does
                    // not consume the renderer's pending font-atlas delta.
                    let atlas = atlas.get_or_insert_with(|| painter.ctx().fonts(|f| f.image()));
                    // Crop before padding: surrounding atlas texels can belong
                    // to another glyph, including a different fallback face.
                    let mut alpha = Vec::with_capacity(bitmap[0] * bitmap[1]);
                    for y in uv.min[1] as usize..uv.max[1] as usize {
                        for x in uv.min[0] as usize..uv.max[0] as usize {
                            alpha.push(atlas[(x, y)].a());
                        }
                    }
                    if let Some(old) = previous.remove(&(uv.min, uv.max))
                        && old.alpha == alpha
                    {
                        return old;
                    }
                    let coverage: Vec<_> = alpha.iter().map(|value| *value as f32 / 255.).collect();
                    let texture = painter.ctx().load_texture(
                        "inline-glyph-outline",
                        stroke(&coverage, bitmap, radius),
                        egui::TextureOptions::LINEAR,
                    );
                    Glyph { alpha, texture }
                });
                let mut mesh = egui::Mesh::with_texture(texture.texture.id());
                mesh.add_rect_with_uv(
                    egui::Rect::ZERO,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                    color,
                );
                for (vertex, point) in mesh.vertices.iter_mut().zip(points) {
                    vertex.pos = point;
                }
                painter.add(mesh);
            }
        }
    }
}

/// Grayscale dilation minus erosion makes a centered, hollow stroke. Preserve
/// fractional coverage (including glyphs with no opaque pixels) and fractional
/// radii. Wide disks use eight separable periodic lines, as in standard disk
/// morphology decompositions, rather than work proportional to stroke radius.
/// The raster stencil approximates a round contour; it is not export geometry.
fn stroke(alpha: &[f32], size: [usize; 2], radius: f32) -> egui::ColorImage {
    let pad = padding(radius);
    let [width, height] = size.map(|value| value + 2 * pad);
    let mut source = vec![0.; width * height];
    for y in 0..size[1] {
        source[(y + pad) * width + pad..(y + pad) * width + pad + size[0]]
            .copy_from_slice(&alpha[y * size[0]..(y + 1) * size[0]]);
    }
    let mut dilated = source.clone();
    let mut eroded = source.clone();
    if radius > DIRECT_DISK_RADIUS {
        for (dx, dy, span) in disk_lines(radius) {
            line_extrema(&mut dilated, &mut eroded, [width, height], dx, dy, span);
        }
    } else {
        disk_extrema(&source, &mut dilated, &mut eroded, [width, height], radius);
    }
    egui::ColorImage::new(
        [width, height],
        dilated
            .into_iter()
            .zip(eroded)
            .map(|(outer, inner)| {
                let alpha = ((outer - inner).clamp(0., 1.) * 255.).round() as u8;
                egui::Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha)
            })
            .collect(),
    )
}

fn disk_lines(radius: f32) -> [(isize, isize, f32); 8] {
    // Tangential 16-gon: these lengths give exactly the axial radius. Its
    // largest angular gap is atan(1/2), so continuous radial overshoot is at
    // most sec(atan(1/2)/2) - 1 = 2.75%, before raster interpolation.
    let axis = radius * (5_f32.sqrt() - 2.);
    let diagonal = radius * (10_f32.sqrt() - 3.) / std::f32::consts::SQRT_2;
    let skew = radius * (5_f32.sqrt() - 2. + 10_f32.sqrt() - 3.) / (2. * 5_f32.sqrt());
    [
        (1, 0, axis),
        (0, 1, axis),
        (1, 1, diagonal),
        (-1, 1, diagonal),
        (2, 1, skew),
        (-2, 1, skew),
        (1, 2, skew),
        (-1, 2, skew),
    ]
}

fn padding(radius: f32) -> usize {
    if radius > DIRECT_DISK_RADIUS {
        // Fractional line endpoints interpolate neighboring knots, whose
        // faint tails extend past the nominal radius. Pad their complete
        // summed support plus a transparent texel, not just ceil(radius).
        disk_lines(radius)
            .iter()
            .map(|(dx, _, span)| dx.unsigned_abs() * span.ceil() as usize)
            .sum::<usize>()
            + 1
    } else {
        radius.ceil() as usize + 1
    }
}

fn line_extrema(
    dilated: &mut [f32],
    eroded: &mut [f32],
    [width, height]: [usize; 2],
    dx: isize,
    dy: isize,
    span: f32,
) {
    let mut indices = Vec::new();
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    let mut max = Vec::new();
    let mut min = Vec::new();
    let mut line = |mut x: isize, mut y: isize| {
        indices.clear();
        outer.clear();
        inner.clear();
        while x >= 0 && x < width as isize && y < height as isize {
            let index = y as usize * width + x as usize;
            indices.push(index);
            outer.push(dilated[index]);
            inner.push(eroded[index]);
            x += dx;
            y += dy;
        }
        max.clone_from(&outer);
        min.clone_from(&inner);
        extrema(&outer, &inner, span, &mut max, &mut min);
        for (i, index) in indices.iter().enumerate() {
            dilated[*index] = max[i];
            eroded[*index] = min[i];
        }
    };
    // Each grid point belongs to exactly one disjoint periodic line. Start
    // at the top and entering side, never filter a pixel twice in one pass.
    for y in 0..(dy as usize).min(height) {
        for x in 0..width {
            line(x as isize, y as isize);
        }
    }
    for y in dy as usize..height {
        for x in 0..dx.unsigned_abs().min(width) {
            line(if dx > 0 { x } else { width - 1 - x } as isize, y as isize);
        }
    }
}

fn disk_extrema(
    source: &[f32],
    dilated: &mut [f32],
    eroded: &mut [f32],
    [width, height]: [usize; 2],
    radius: f32,
) {
    let mut rows: Vec<_> = (-(radius.floor() as i32)..=radius.floor() as i32)
        .map(|dy| {
            let dy = dy as f32;
            (dy, (radius * radius - dy * dy).max(0.).sqrt())
        })
        .collect();
    if radius.fract() != 0. {
        rows.extend([(-radius, 0.), (radius, 0.)]);
    }
    let mut row = vec![0.; width];
    for (dy, span) in rows {
        for y in 0..height {
            let sy = y as f32 + dy;
            if sy.fract() == 0. && sy >= 0. && sy < height as f32 {
                let start = sy as usize * width;
                row.copy_from_slice(&source[start..start + width]);
            } else if sy < 0. || sy > (height - 1) as f32 {
                row.fill(0.);
            } else {
                let top = sy.floor() as usize * width;
                let fraction = sy.fract();
                for (x, value) in row.iter_mut().enumerate() {
                    *value = source[top + x] * (1. - fraction) + source[top + width + x] * fraction;
                }
            }
            extrema(
                &row,
                &row,
                span,
                &mut dilated[y * width..(y + 1) * width],
                &mut eroded[y * width..(y + 1) * width],
            );
        }
    }
    // The diagonal samples keep subpixel disks round as well: integer rows
    // alone contain only the horizontal diameter when radius < 1 pixel.
    let diagonal = radius / std::f32::consts::SQRT_2;
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            for dy in [-diagonal, diagonal] {
                for dx in [-diagonal, diagonal] {
                    let value = sample(source, width, height, x as f32 + dx, y as f32 + dy);
                    dilated[index] = dilated[index].max(value);
                    eroded[index] = eroded[index].min(value);
                }
            }
        }
    }
}

fn sample(values: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
    let left = x.floor() as isize;
    let top = y.floor() as isize;
    let at = |x: isize, y: isize| {
        if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
            0.
        } else {
            values[y as usize * width + x as usize]
        }
    };
    let fx = x - left as f32;
    let fy = y - top as f32;
    let a = at(left, top) * (1. - fx) + at(left + 1, top) * fx;
    let b = at(left, top + 1) * (1. - fx) + at(left + 1, top + 1) * fx;
    a * (1. - fy) + b * fy
}

fn extrema(outer: &[f32], inner: &[f32], span: f32, dilated: &mut [f32], eroded: &mut [f32]) {
    let radius = span.floor() as isize;
    let fraction = span.fract();
    let mut max = VecDeque::<(isize, f32)>::new();
    let mut min = VecDeque::<(isize, f32)>::new();
    for index in -radius..outer.len() as isize + radius {
        let a = outer.get(index as usize).copied().unwrap_or(0.);
        let b = inner.get(index as usize).copied().unwrap_or(0.);
        while max.back().is_some_and(|(_, old)| *old <= a) {
            max.pop_back();
        }
        while min.back().is_some_and(|(_, old)| *old >= b) {
            min.pop_back();
        }
        max.push_back((index, a));
        min.push_back((index, b));
        let start = index - 2 * radius;
        while max.front().is_some_and(|(index, _)| *index < start) {
            max.pop_front();
        }
        while min.front().is_some_and(|(index, _)| *index < start) {
            min.pop_front();
        }
        let x = index - radius;
        if x < 0 {
            continue;
        }
        let mut a = max.front().unwrap().1;
        let mut b = min.front().unwrap().1;
        if fraction != 0. {
            for direction in [-1, 1] {
                let near = x + direction * radius;
                let far = near + direction;
                let at = |row: &[f32], index: isize| row.get(index as usize).copied().unwrap_or(0.);
                a = a.max(at(outer, near) * (1. - fraction) + at(outer, far) * fraction);
                b = b.min(at(inner, near) * (1. - fraction) + at(inner, far) * fraction);
            }
        }
        dilated[x as usize] = dilated[x as usize].max(a);
        eroded[x as usize] = eroded[x as usize].min(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_extrema_match_asymmetric_brute_force_windows() {
        let row = [0.3_f32, 0.9, 0.1, 0.5, 0.7, 0., 0.4];
        for radius in [0., 0.25, 0.75, 1., 1.4, 3.8, 9.2] {
            let mut max = vec![0.; row.len()];
            let mut min = vec![1.; row.len()];
            extrema(&row, &row, radius, &mut max, &mut min);
            // Independently evaluate the finite piecewise-linear row: its
            // extrema are integer knots or the two fractional endpoints.
            let reference = |x: f32| {
                let left = x.floor() as isize;
                let a = row.get(left as usize).copied().unwrap_or(0.);
                let b = row.get((left + 1) as usize).copied().unwrap_or(0.);
                a + (b - a) * (x - left as f32)
            };
            for x in 0..row.len() {
                let mut values = vec![reference(x as f32 - radius), reference(x as f32 + radius)];
                for offset in -(radius.floor() as isize)..=radius.floor() as isize {
                    values.push(
                        row.get((x as isize + offset) as usize)
                            .copied()
                            .unwrap_or(0.),
                    );
                }
                assert!((max[x] - values.iter().copied().fold(0., f32::max)).abs() < 1e-6);
                assert!((min[x] - values.iter().copied().fold(1., f32::min)).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn periodic_lines_match_independent_asymmetric_two_dimensional_windows() {
        let size = [7, 11];
        let outer: Vec<_> = (0..77)
            .map(|i| ((i * 37 + 19) % 101) as f32 / 100.)
            .collect();
        let inner: Vec<_> = (0..77)
            .map(|i| ((i * 13 + 51) % 103) as f32 / 102.)
            .collect();
        for (dx, dy) in [
            (1, 0),
            (0, 1),
            (1, 1),
            (-1, 1),
            (2, 1),
            (-2, 1),
            (1, 2),
            (-1, 2),
        ] {
            for span in [0.25_f32, 1., 2.6, 12.3] {
                let mut max = outer.clone();
                let mut min = inner.clone();
                line_extrema(&mut max, &mut min, size, dx, dy, span);
                for y in 0..size[1] as isize {
                    for x in 0..size[0] as isize {
                        let at = |values: &[f32], step: f32| {
                            let knot = |step: isize| {
                                let (x, y) = (x + step * dx, y + step * dy);
                                if x < 0 || y < 0 || x >= size[0] as isize || y >= size[1] as isize
                                {
                                    0.
                                } else {
                                    values[y as usize * size[0] + x as usize]
                                }
                            };
                            let a = knot(step.floor() as isize);
                            let b = knot(step.floor() as isize + 1);
                            a + (b - a) * (step - step.floor())
                        };
                        let offsets = (-(span.floor() as i32)..=span.floor() as i32)
                            .map(|step| step as f32)
                            .chain([-span, span]);
                        let expected_max = offsets
                            .clone()
                            .map(|step| at(&outer, step))
                            .fold(0., f32::max);
                        let expected_min = offsets.map(|step| at(&inner, step)).fold(1., f32::min);
                        let index = y as usize * size[0] + x as usize;
                        assert!(
                            (max[index] - expected_max).abs() < 1e-5,
                            "max {dx},{dy} at {x},{y}"
                        );
                        assert!(
                            (min[index] - expected_min).abs() < 1e-5,
                            "min {dx},{dy} at {x},{y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn centered_strokes_clear_deep_ink_and_do_not_clip_the_outer_half() {
        let size = [13, 19];
        for opacity in [1., 0.25] {
            let mut alpha = vec![0.; size[0] * size[1]];
            for y in 3..16 {
                for x in 3..10 {
                    alpha[y * size[0] + x] = opacity;
                }
            }
            let image = stroke(&alpha, size, 1.5);
            let at = |x: usize, y: usize| image[(x + 3, y + 3)].a();
            assert_eq!(at(6, 9), 0, "not a filled glyph");
            assert_eq!(at(3, 9), (opacity * 255.).round() as u8, "inner half");
            assert_eq!(at(2, 9), (opacity * 255.).round() as u8, "outer half");
            assert_eq!(at(1, 9), (opacity * 127.5).round() as u8, "fractional edge");
            assert!(
                image.pixels[..image.size[0]]
                    .iter()
                    .all(|pixel| pixel.a() == 0)
            );
            for y in 0..image.size[1] {
                assert_eq!(image[(0, y)].a(), 0);
                assert_eq!(image[(image.size[0] - 1, y)].a(), 0);
            }
        }
    }

    #[test]
    fn weak_marks_survive_subpixel_strokes_without_alpha_thresholds() {
        for radius in [0.25_f32, 0.5, 0.75] {
            for coverage in [0.1, 0.25, 0.49] {
                let image = stroke(&[coverage], [1, 1], radius);
                // For this isolated texel, the lowest disk sample is a
                // diagonal: coverage * (1 - radius / sqrt(2))².
                let inner = (1. - radius / std::f32::consts::SQRT_2).powi(2);
                let expected = (coverage * (1. - inner) * 255.).round() as u8;
                assert_eq!(image[(2, 2)].a(), expected);
                assert!(expected > 0 && expected <= (coverage * 255.).round() as u8);
                let diagonal = stroke(&[coverage, 0., 0., coverage], [2, 2], radius);
                assert!(diagonal.pixels.iter().any(|pixel| pixel.a() > 0));
            }
        }
    }

    #[test]
    fn wide_fractional_stencils_do_not_clip_faint_endpoint_tails() {
        for radius in [4.01, 5.12, 8.7, 20.48, 40.96] {
            let image = stroke(&[0.25], [1, 1], radius);
            let [width, height] = image.size;
            for x in 0..width {
                assert_eq!(image[(x, 0)].a(), 0, "top halo, radius={radius}");
                assert_eq!(
                    image[(x, height - 1)].a(),
                    0,
                    "bottom halo, radius={radius}"
                );
            }
            for y in 0..height {
                assert_eq!(image[(0, y)].a(), 0, "left halo, radius={radius}");
                assert_eq!(image[(width - 1, y)].a(), 0, "right halo, radius={radius}");
            }
            assert!(image.pixels.iter().any(|p| p.a() > 0), "no alpha threshold");
        }
    }

    #[test]
    fn counters_remain_open_and_opaque_walls_remain_hollow() {
        let size = [25, 31];
        let mut alpha = vec![0.; size[0] * size[1]];
        for y in 3..28 {
            for x in 2..23 {
                if !(9..16).contains(&x) || !(12..21).contains(&y) {
                    alpha[y * size[0] + x] = 1.;
                }
            }
        }
        let image = stroke(&alpha, size, 1.5);
        let at = |x: usize, y: usize| image[(x + 3, y + 3)].a();
        assert_eq!(at(12, 16), 0, "counter center");
        assert_eq!(at(5, 16), 0, "deep wall interior");
        assert_eq!(at(2, 16), 255, "outer contour");
        assert_eq!(at(8, 16), 255, "inner contour");
        assert_eq!(at(9, 16), 255, "stroke extends into counter");
    }

    #[test]
    fn antialiased_edges_keep_width_and_sharpness_at_fractional_pixel_phases() {
        for radius in [
            0.25_f32, 0.75, 1.5, 3.25, 4., 4.01, 5.12, 10.24, 12., 12.01, 20.48,
        ] {
            let size = [
                6 * radius.ceil() as usize + 40,
                2 * radius.ceil() as usize + 21,
            ];
            let edge = size[0] / 2;
            for phase in [0., 0.17, 0.63] {
                let alpha: Vec<_> = (0..size[0] * size[1])
                    .map(|i| ((i % size[0]) as f32 + phase - edge as f32 - 0.5).clamp(0., 1.))
                    .collect();
                let image = stroke(&alpha, size, radius);
                let pad = padding(radius);
                let extent = radius.ceil() as usize + 4;
                let area: f32 = (edge - extent..=edge + extent)
                    .map(|x| image[(x + pad, size[1] / 2 + pad)].a() as f32 / 255.)
                    .sum();
                assert!(
                    (area - 2. * radius).abs() < 0.04,
                    "radius={radius}, phase={phase}: {area}"
                );
                if radius <= 12. {
                    // For a straight grayscale edge, disk extrema are its
                    // two horizontal endpoints. Match their coverage, not
                    // only total area: repeated interpolation can keep the
                    // width while blurring the contour over several pixels.
                    let edge_coverage = |x: f32| {
                        let knot = |x: f32| (x + phase - edge as f32 - 0.5).clamp(0., 1.);
                        let fraction = x.fract();
                        knot(x.floor()) * (1. - fraction) + knot(x.ceil()) * fraction
                    };
                    for x in edge - extent..=edge + extent {
                        let expected = ((edge_coverage(x as f32 + radius)
                            - edge_coverage(x as f32 - radius))
                            * 255.)
                            .round() as u8;
                        let actual = image[(x + pad, size[1] / 2 + pad)].a();
                        assert!(
                            actual.abs_diff(expected) <= 1,
                            "soft edge: radius={radius}, phase={phase}, x={x}: {actual} vs {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cached_ink_keeps_the_shaped_mesh_and_does_not_consume_atlas_updates() {
        cache_matrix(&[32., 128.]);
    }

    #[test]
    #[ignore = "large-font timing matrix; run explicitly with optimized host code"]
    fn large_font_timing_matrix() {
        cache_matrix(&[512.]);
    }

    #[test]
    fn recycled_rasters_and_width_changes_replace_ink_but_tint_does_not() {
        let ctx = egui::Context::default();
        crate::ui_fonts::install(&ctx);
        ctx.begin_pass(Default::default());
        let format = egui::TextFormat {
            font_id: egui::FontId::proportional(32.),
            color: egui::Color32::TRANSPARENT,
            ..Default::default()
        };
        let layout =
            |text| super::super::inline_galley(&ctx, text, &format, egui::Align::LEFT, true, 400.);
        let painter = ctx.layer_painter(egui::LayerId::background());
        let mut cache = Outline::default();
        let galley = layout("O");
        cache.paint(
            &painter,
            &galley,
            egui::Pos2::ZERO,
            2.56,
            egui::Color32::WHITE,
            |p| p,
        );
        let (&uv, glyph) = cache.glyphs.first_key_value().unwrap();
        let original_id = glyph.texture.id();
        cache.paint(
            &painter,
            &galley,
            egui::Pos2::ZERO,
            2.56,
            egui::Color32::from_gray(90),
            |p| p,
        );
        assert_eq!(cache.glyphs[&uv].texture.id(), original_id);

        // A previous atlas occupant at these same UVs had different alpha.
        // Keep the stale bitmap and its stale texture consistent, so this
        // catches a cache keyed by UV alone, not merely a broken test record.
        let glyph = cache.glyphs.get_mut(&uv).unwrap();
        glyph.alpha[0] ^= 255;
        glyph.texture = ctx.load_texture(
            "old-atlas-occupant",
            stroke(
                &glyph
                    .alpha
                    .iter()
                    .map(|a| *a as f32 / 255.)
                    .collect::<Vec<_>>(),
                [(uv.1[0] - uv.0[0]) as usize, (uv.1[1] - uv.0[1]) as usize],
                1.28,
            ),
            egui::TextureOptions::LINEAR,
        );
        let stale_id = glyph.texture.id();
        let typed = layout("O ");
        cache.paint(
            &painter,
            &typed,
            egui::Pos2::ZERO,
            2.56,
            egui::Color32::WHITE,
            |p| p,
        );
        let restored_id = cache.glyphs[&uv].texture.id();
        assert_ne!(restored_id, stale_id);
        cache.paint(
            &painter,
            &typed,
            egui::Pos2::ZERO,
            3.5,
            egui::Color32::WHITE,
            |p| p,
        );
        assert_ne!(cache.glyphs[&uv].texture.id(), restored_id);
        ctx.end_pass().textures_delta.clear();
    }

    fn cache_matrix(sizes: &[f32]) {
        for pixels_per_point in [1., 2.] {
            for &size in sizes {
                let ctx = egui::Context::default();
                crate::ui_fonts::install(&ctx);
                ctx.set_pixels_per_point(pixels_per_point);
                ctx.begin_pass(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(2048., 2048.),
                    )),
                    ..Default::default()
                });
                let format = egui::TextFormat {
                    font_id: egui::FontId::new(size, egui::FontFamily::Proportional),
                    color: egui::Color32::TRANSPARENT,
                    ..Default::default()
                };
                let galley = super::super::inline_galley(
                    &ctx,
                    "Oo office Á Ω العربية",
                    &format,
                    egui::Align::LEFT,
                    false,
                    1600.,
                );
                let before = galley.clone();
                let painter = ctx.layer_painter(egui::LayerId::background());
                let color = egui::Color32::from_rgba_unmultiplied(22, 96, 148, 127);
                let mut cache = Outline::default();
                let width = (size * 0.08).max(1.5);
                let start = std::time::Instant::now();
                cache.paint(
                    &painter,
                    &galley,
                    egui::pos2(30., 40.),
                    width,
                    color,
                    |point| point,
                );
                let cold = start.elapsed();
                let ids: Vec<_> = cache
                    .glyphs
                    .values()
                    .map(|glyph| glyph.texture.id())
                    .collect();
                let original_atlas = ctx.fonts(|f| f.image());
                let crop = |atlas: &egui::ColorImage, (min, max): ([u16; 2], [u16; 2])| {
                    let mut alpha = Vec::new();
                    for y in min[1]..max[1] {
                        for x in min[0]..max[0] {
                            alpha.push(atlas[(x as usize, y as usize)].a());
                        }
                    }
                    alpha
                };
                let original: BTreeMap<_, _> = cache
                    .glyphs
                    .iter()
                    .map(|(uv, glyph)| (*uv, (glyph.texture.id(), crop(&original_atlas, *uv))))
                    .collect();
                assert!(!ids.is_empty());
                let start = std::time::Instant::now();
                cache.paint(
                    &painter,
                    &galley,
                    egui::pos2(90., 70.),
                    width,
                    color,
                    |point| point,
                );
                let warm = start.elapsed();
                assert_eq!(
                    ids,
                    cache
                        .glyphs
                        .values()
                        .map(|glyph| glyph.texture.id())
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    galley, before,
                    "painting cannot change glyphs, row positions or cursor layout"
                );
                let typed = super::super::inline_galley(
                    &ctx,
                    "Oo office Á Ω العربية o",
                    &format,
                    egui::Align::LEFT,
                    false,
                    1600.,
                );
                let start = std::time::Instant::now();
                cache.paint(
                    &painter,
                    &typed,
                    egui::pos2(30., 40.),
                    width,
                    color,
                    |point| point,
                );
                let typing = start.elapsed();
                let typed_ids: Vec<_> = cache
                    .glyphs
                    .values()
                    .map(|glyph| glyph.texture.id())
                    .collect();
                let typed_atlas = ctx.fonts(|f| f.image());
                for (uv, glyph) in &cache.glyphs {
                    if let Some((id, alpha)) = original.get(uv) {
                        if crop(&typed_atlas, *uv) == *alpha {
                            assert_eq!(glyph.texture.id(), *id, "unchanged raster reuse");
                        } else {
                            assert_ne!(
                                glyph.texture.id(),
                                *id,
                                "recycled UVs cannot reuse old ink"
                            );
                        }
                    }
                }
                let mut selected = typed.clone();
                let mut visuals = egui::Visuals::default();
                visuals.selection.stroke.color = egui::Color32::TRANSPARENT;
                let range = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(4),
                );
                egui::text_selection::visuals::paint_text_selection(
                    &mut selected,
                    &visuals,
                    &range,
                    None,
                );
                assert!(!Arc::ptr_eq(&selected, &typed));
                cache.paint(
                    &painter,
                    &selected,
                    egui::pos2(30., 40.),
                    width,
                    color,
                    |point| point,
                );
                assert_eq!(
                    typed_ids,
                    cache
                        .glyphs
                        .values()
                        .map(|glyph| glyph.texture.id())
                        .collect::<Vec<_>>(),
                    "selection decorations do not replace the glyph cache"
                );
                let mut output = ctx.end_pass();
                let updates = std::mem::take(&mut output.textures_delta.set);
                output.textures_delta.clear();
                assert!(
                    updates
                        .iter()
                        .any(|(id, _)| *id == egui::TextureId::Managed(0))
                );
                let expected_uploads: std::collections::BTreeSet<_> =
                    ids.iter().chain(&typed_ids).collect();
                assert_eq!(
                    updates.len(),
                    expected_uploads.len() + 1,
                    "uploads only for new ink"
                );
                for shape in &output.shapes {
                    if let egui::Shape::Mesh(mesh) = &shape.shape {
                        assert!(mesh.vertices.iter().all(|vertex| vertex.color == color));
                    }
                }
                println!(
                    "outline {size}pt @{pixels_per_point}x: cold={cold:?}, warm={warm:?}, typing={typing:?}, {} glyphs",
                    ids.len()
                );
            }
        }
    }
}
