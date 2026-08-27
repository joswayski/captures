use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use captures_recording::CaptureRect;

const CLICK_DURATION: Duration = Duration::from_millis(550);
const CLICK_COLOR: [u8; 3] = [255, 103, 64];
const CURSOR_OUTLINE: [(i32, i32); 7] = [
    (0, 0),
    (0, 22),
    (6, 16),
    (11, 27),
    (16, 25),
    (11, 15),
    (21, 15),
];
const CURSOR_FILL: [(i32, i32); 7] = [
    (2, 4),
    (2, 18),
    (6, 13),
    (12, 23),
    (13, 22),
    (8, 12),
    (17, 12),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerSample {
    pub x: i32,
    pub y: i32,
    pub primary_down: bool,
    pub secondary_down: bool,
}

#[derive(Clone, Copy)]
pub struct PointerLayout {
    display_x: i32,
    display_y: i32,
    source: CaptureRect,
    output_width: u32,
    output_height: u32,
    pointer_scale: f64,
}

#[derive(Clone, Copy)]
pub struct PointerCaptureSpace {
    pub display_x: i32,
    pub display_y: i32,
    pub scale_factor: f64,
    pub physical_display_geometry: bool,
    pub source: CaptureRect,
    pub source_is_overlay_space: bool,
}

impl PointerLayout {
    pub fn new(
        display_x: i32,
        display_y: i32,
        source: CaptureRect,
        output_width: u32,
        output_height: u32,
        pointer_scale: f64,
    ) -> Self {
        Self {
            display_x,
            display_y,
            source,
            output_width,
            output_height,
            pointer_scale: pointer_scale.max(1.0),
        }
    }

    /// Map a global pointer sample into the recorded output.
    ///
    /// Region targets are selected in overlay/CSS DIPs. Display/window geometry
    /// stays in the capture backend's native units (physical on Windows, logical
    /// on Linux). Pointer samples are physical on both Windows (`GetCursorPos`)
    /// and X11, so the scale applied here has to put origin, source, and sample
    /// into the same space.
    pub fn for_capture(space: PointerCaptureSpace, output_width: u32, output_height: u32) -> Self {
        let (origin_x, origin_y, pointer_scale) = pointer_origin_and_scale(space);
        Self::new(
            origin_x,
            origin_y,
            space.source,
            output_width,
            output_height,
            pointer_scale,
        )
    }

    fn map(self, sample: PointerSample) -> Option<(i32, i32)> {
        let display_x = f64::from(sample.x) / self.pointer_scale - f64::from(self.display_x);
        let display_y = f64::from(sample.y) / self.pointer_scale - f64::from(self.display_y);
        let source_x = display_x - f64::from(self.source.x);
        let source_y = display_y - f64::from(self.source.y);
        if source_x < 0.0
            || source_y < 0.0
            || source_x >= f64::from(self.source.width)
            || source_y >= f64::from(self.source.height)
        {
            return None;
        }
        Some((
            (source_x * f64::from(self.output_width) / f64::from(self.source.width)).round() as i32,
            (source_y * f64::from(self.output_height) / f64::from(self.source.height)).round()
                as i32,
        ))
    }
}

fn pointer_origin_and_scale(space: PointerCaptureSpace) -> (i32, i32, f64) {
    let scale = space.scale_factor.max(1.0);
    if space.source_is_overlay_space && space.physical_display_geometry {
        (
            (f64::from(space.display_x) / scale).round() as i32,
            (f64::from(space.display_y) / scale).round() as i32,
            scale,
        )
    } else if space.physical_display_geometry {
        (space.display_x, space.display_y, 1.0)
    } else {
        (space.display_x, space.display_y, scale)
    }
}

struct ClickAnimation {
    position: (i32, i32),
    started_at: Instant,
}

pub struct PointerOverlay {
    layout: PointerLayout,
    show_cursor: bool,
    highlight_clicks: bool,
    primary_down: bool,
    secondary_down: bool,
    clicks: Vec<ClickAnimation>,
}

impl PointerOverlay {
    pub fn new(layout: PointerLayout, show_cursor: bool, highlight_clicks: bool) -> Self {
        Self {
            layout,
            show_cursor,
            highlight_clicks,
            primary_down: false,
            secondary_down: false,
            clicks: Vec::new(),
        }
    }

    pub fn draw(
        &mut self,
        rgb: &mut [u8],
        sample: Option<PointerSample>,
        now: Instant,
    ) -> OverlayPatch {
        let mapped = sample.and_then(|sample| self.layout.map(sample));
        if self.highlight_clicks
            && let (Some(sample), Some(position)) = (sample, mapped)
            && ((sample.primary_down && !self.primary_down)
                || (sample.secondary_down && !self.secondary_down))
        {
            if self.clicks.len() == 6 {
                self.clicks.remove(0);
            }
            self.clicks.push(ClickAnimation {
                position,
                started_at: now,
            });
        }
        if let Some(sample) = sample {
            self.primary_down = sample.primary_down;
            self.secondary_down = sample.secondary_down;
        }
        self.clicks
            .retain(|click| now.saturating_duration_since(click.started_at) < CLICK_DURATION);

        let mut patch = OverlayPatch::default();
        for click in &self.clicks {
            draw_click(
                rgb,
                self.layout.output_width,
                self.layout.output_height,
                click,
                now,
                &mut patch,
            );
        }
        if self.show_cursor
            && let Some(position) = mapped
        {
            draw_cursor(
                rgb,
                self.layout.output_width,
                self.layout.output_height,
                position,
                &mut patch,
            );
        }
        patch
    }
}

#[derive(Default)]
pub struct OverlayPatch {
    pixels: BTreeMap<usize, [u8; 3]>,
}

impl OverlayPatch {
    pub fn restore(self, rgb: &mut [u8]) {
        for (index, pixel) in self.pixels {
            if let Some(destination) = rgb.get_mut(index..index.saturating_add(3)) {
                destination.copy_from_slice(&pixel);
            }
        }
    }
}

fn draw_click(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    click: &ClickAnimation,
    now: Instant,
    patch: &mut OverlayPatch,
) {
    let elapsed = now.saturating_duration_since(click.started_at);
    let progress = (elapsed.as_secs_f64() / CLICK_DURATION.as_secs_f64()).clamp(0.0, 1.0);
    let radius = 7.0 + progress * 23.0;
    let thickness = 3.5;
    let alpha = (0.9 * (1.0 - progress)).max(0.0);
    let boundary = radius.ceil() as i32 + 1;
    for y in -boundary..=boundary {
        for x in -boundary..=boundary {
            let distance = f64::from(x * x + y * y).sqrt();
            if (distance - radius).abs() <= thickness {
                blend_pixel(
                    rgb,
                    width,
                    height,
                    click.position.0 + x,
                    click.position.1 + y,
                    CLICK_COLOR,
                    alpha,
                    patch,
                );
            }
        }
    }
}

fn draw_cursor(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    position: (i32, i32),
    patch: &mut OverlayPatch,
) {
    let scale = (f64::from(height) / 1_080.0).round().clamp(1.0, 2.0) as i32;
    draw_polygon(
        rgb,
        width,
        height,
        position,
        &CURSOR_OUTLINE,
        scale,
        [24, 24, 24],
        patch,
    );
    draw_polygon(
        rgb,
        width,
        height,
        position,
        &CURSOR_FILL,
        scale,
        [248, 248, 248],
        patch,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_polygon(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    origin: (i32, i32),
    polygon: &[(i32, i32)],
    scale: i32,
    color: [u8; 3],
    patch: &mut OverlayPatch,
) {
    let scaled = polygon
        .iter()
        .map(|(x, y)| (x * scale, y * scale))
        .collect::<Vec<_>>();
    let max_x = scaled.iter().map(|(x, _)| *x).max().unwrap_or_default();
    let max_y = scaled.iter().map(|(_, y)| *y).max().unwrap_or_default();
    for y in 0..=max_y {
        for x in 0..=max_x {
            if point_in_polygon(x, y, &scaled) {
                blend_pixel(
                    rgb,
                    width,
                    height,
                    origin.0 + x,
                    origin.1 + y,
                    color,
                    1.0,
                    patch,
                );
            }
        }
    }
}

fn point_in_polygon(x: i32, y: i32, polygon: &[(i32, i32)]) -> bool {
    let mut inside = false;
    let mut previous = polygon.last().copied().unwrap_or_default();
    for &current in polygon {
        let crosses = (current.1 > y) != (previous.1 > y)
            && f64::from(x)
                < f64::from(previous.0 - current.0) * f64::from(y - current.1)
                    / f64::from(previous.1 - current.1)
                    + f64::from(current.0);
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

#[allow(clippy::too_many_arguments)]
fn blend_pixel(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    color: [u8; 3],
    alpha: f64,
    patch: &mut OverlayPatch,
) {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return;
    };
    if x >= width || y >= height {
        return;
    }
    let pixel = usize::try_from(y)
        .unwrap_or_default()
        .saturating_mul(usize::try_from(width).unwrap_or_default())
        .saturating_add(usize::try_from(x).unwrap_or_default());
    let index = pixel.saturating_mul(3);
    let Some(destination) = rgb.get_mut(index..index.saturating_add(3)) else {
        return;
    };
    patch
        .pixels
        .entry(index)
        .or_insert([destination[0], destination[1], destination[2]]);
    for channel in 0..3 {
        destination[channel] = (f64::from(destination[channel]) * (1.0 - alpha)
            + f64::from(color[channel]) * alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use captures_recording::CaptureRect;

    use super::{PointerCaptureSpace, PointerLayout, PointerOverlay, PointerSample};

    fn layout() -> PointerLayout {
        PointerLayout::new(
            100,
            50,
            CaptureRect {
                x: 100,
                y: 50,
                width: 800,
                height: 450,
            },
            1_600,
            900,
            2.0,
        )
    }

    #[test]
    fn maps_scaled_global_pointer_into_recorded_region() {
        assert_eq!(
            layout().map(PointerSample {
                x: 800,
                y: 500,
                primary_down: false,
                secondary_down: false,
            }),
            Some((400, 300))
        );
    }

    #[test]
    fn windows_region_layout_converts_physical_cursor_into_overlay_space() {
        let layout = PointerLayout::for_capture(
            PointerCaptureSpace {
                display_x: 0,
                display_y: 0,
                scale_factor: 2.0,
                physical_display_geometry: true,
                source: CaptureRect {
                    x: 100,
                    y: 50,
                    width: 800,
                    height: 450,
                },
                source_is_overlay_space: true,
            },
            1_600,
            900,
        );
        // Physical (800, 500) → DIP (400, 250) → region-local (300, 200) → 1600×900.
        assert_eq!(
            layout.map(PointerSample {
                x: 800,
                y: 500,
                primary_down: false,
                secondary_down: false,
            }),
            Some((600, 400))
        );
    }

    #[test]
    fn windows_display_layout_keeps_physical_cursor_and_source_aligned() {
        let layout = PointerLayout::for_capture(
            PointerCaptureSpace {
                display_x: 100,
                display_y: 50,
                scale_factor: 2.0,
                physical_display_geometry: true,
                source: CaptureRect {
                    x: 0,
                    y: 0,
                    width: 3_840,
                    height: 2_160,
                },
                source_is_overlay_space: false,
            },
            1_920,
            1_080,
        );
        assert_eq!(
            layout.map(PointerSample {
                x: 900,
                y: 550,
                primary_down: false,
                secondary_down: false,
            }),
            Some((400, 250))
        );
    }

    #[test]
    fn linux_region_layout_divides_physical_pointer_samples() {
        let layout = PointerLayout::for_capture(
            PointerCaptureSpace {
                display_x: 100,
                display_y: 50,
                scale_factor: 2.0,
                physical_display_geometry: false,
                source: CaptureRect {
                    x: 100,
                    y: 50,
                    width: 800,
                    height: 450,
                },
                source_is_overlay_space: true,
            },
            1_600,
            900,
        );
        assert_eq!(
            layout.map(PointerSample {
                x: 800,
                y: 500,
                primary_down: false,
                secondary_down: false,
            }),
            Some((400, 300))
        );
    }

    #[test]
    fn cursor_overlay_restores_the_unmodified_frame() {
        let now = Instant::now();
        let mut overlay = PointerOverlay::new(layout(), true, false);
        let mut rgb = vec![32; 1_600 * 900 * 3];
        let original = rgb.clone();
        let patch = overlay.draw(
            &mut rgb,
            Some(PointerSample {
                x: 800,
                y: 500,
                primary_down: false,
                secondary_down: false,
            }),
            now,
        );
        assert_ne!(rgb, original);
        patch.restore(&mut rgb);
        assert_eq!(rgb, original);
    }

    #[test]
    fn click_edge_draws_a_temporary_ripple() {
        let now = Instant::now();
        let mut overlay = PointerOverlay::new(layout(), false, true);
        let mut rgb = vec![0; 1_600 * 900 * 3];
        let patch = overlay.draw(
            &mut rgb,
            Some(PointerSample {
                x: 800,
                y: 500,
                primary_down: true,
                secondary_down: false,
            }),
            now + Duration::from_millis(100),
        );
        assert!(rgb.iter().any(|channel| *channel != 0));
        patch.restore(&mut rgb);
        assert!(rgb.iter().all(|channel| *channel == 0));
    }
}
