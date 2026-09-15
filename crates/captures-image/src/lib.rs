//! Toolkit-independent annotation rendering in source-image pixel coordinates.
//! Layers are composited in document order, then the crop is applied. This crate
//! owns neither editor state nor file/clipboard operations.

use std::sync::Arc;

use fontdue::layout::{CoordinateSystem, HorizontalAlign, Layout, LayoutSettings, TextStyle};
use image::{Pixel, Rgba, RgbaImage};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    fn center(self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }

    fn from_points(points: impl IntoIterator<Item = Point>) -> Option<Self> {
        let mut points = points.into_iter();
        let first = points.next()?;
        let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
        for point in points {
            left = left.min(point.x);
            top = top.min(point.y);
            right = right.max(point.x);
            bottom = bottom.max(point.y);
        }
        Some(Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }

    fn corners(self) -> [Point; 4] {
        [
            Point {
                x: self.x,
                y: self.y,
            },
            Point {
                x: self.x + self.width,
                y: self.y,
            },
            Point {
                x: self.x + self.width,
                y: self.y + self.height,
            },
            Point {
                x: self.x,
                y: self.y + self.height,
            },
        ]
    }
}

#[derive(Clone, Debug)]
pub enum Shape {
    Freehand(Vec<Point>),
    /// A closed contour, including concave shapes such as stars. Uses nonzero winding.
    Polygon(Vec<Point>),
    Line {
        start: Point,
        end: Point,
    },
    Arrow {
        start: Point,
        end: Point,
    },
    Rectangle {
        origin: Point,
        width: f32,
        height: f32,
    },
    Ellipse {
        origin: Point,
        width: f32,
        height: f32,
    },
    /// Font bytes are explicit: rendering never consults host-installed fonts.
    Text {
        origin: Point,
        text: String,
        font_size: f32,
        font_data: Arc<[u8]>,
        style: TextStyleSettings,
    },
    /// Straight-alpha pixels fitted to document-space bounds. Layer color's
    /// alpha controls opacity; its RGB, stroke width and fill are ignored.
    Image {
        origin: Point,
        width: f32,
        height: f32,
        pixels: Arc<RgbaImage>,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextShadow {
    pub color: [u8; 4],
    pub blur: f32,
    pub offset: Point,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextStyleSettings {
    pub bold: bool,
    pub italic: bool,
    pub align: TextAlign,
    /// None grows to the longest line; Some wraps into a fixed layout box.
    pub width: Option<f32>,
    pub background: Option<[u8; 4]>,
    pub rounded_background: bool,
    pub outlined: bool,
    pub shadow: Option<TextShadow>,
}

impl Default for TextStyleSettings {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            align: TextAlign::Left,
            width: None,
            background: None,
            rounded_background: false,
            outlined: false,
            shadow: None,
        }
    }
}

/// The six compositing choices exposed by the shipping image editor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

impl BlendMode {
    fn raster(self) -> tiny_skia::BlendMode {
        match self {
            Self::Normal => tiny_skia::BlendMode::SourceOver,
            Self::Multiply => tiny_skia::BlendMode::Multiply,
            Self::Screen => tiny_skia::BlendMode::Screen,
            Self::Overlay => tiny_skia::BlendMode::Overlay,
            Self::Darken => tiny_skia::BlendMode::Darken,
            Self::Lighten => tiny_skia::BlendMode::Lighten,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: u64,
    pub shape: Shape,
    pub color: [u8; 4],
    pub stroke_width: f32,
    pub fill: Option<[u8; 4]>,
    /// Clockwise rotation around the unrotated geometry's center.
    pub rotation_degrees: f32,
    pub blend_mode: BlendMode,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub source: Arc<RgbaImage>,
    pub crop: Option<PixelRect>,
    pub layers: Vec<Layer>,
}

fn rotate(point: Point, center: Point, degrees: f32) -> Point {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Point {
        x: center.x + (point.x - center.x) * cos - (point.y - center.y) * sin,
        y: center.y + (point.x - center.x) * sin + (point.y - center.y) * cos,
    }
}

fn segment_distance(point: Point, start: Point, end: Point) -> f32 {
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0)
    };
    (point.x - start.x - t * dx).hypot(point.y - start.y - t * dy)
}

fn arrow_head(start: Point, end: Point, stroke: f32) -> [Point; 2] {
    let length = (end.x - start.x).hypot(end.y - start.y);
    if length == 0.0 {
        return [end, end];
    }
    let size = (stroke * 4.0).max(10.0).min(length * 0.4);
    let (dx, dy) = (
        (end.x - start.x) / length * size,
        (end.y - start.y) / length * size,
    );
    [
        Point {
            x: end.x - dx - dy * 0.5,
            y: end.y - dy + dx * 0.5,
        },
        Point {
            x: end.x - dx + dy * 0.5,
            y: end.y - dy - dx * 0.5,
        },
    ]
}

fn text_layout(
    origin: Point,
    text: &str,
    size: f32,
    bytes: &[u8],
    style: &TextStyleSettings,
) -> Result<(fontdue::Font, Layout), String> {
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
        .map_err(str::to_owned)?;
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    // fontdue needs a layout width to establish center/right anchors. Measure
    // unwrapped first so auto-width multiline text aligns against its longest line.
    let measured_width = if style.width.is_none() {
        layout.reset(&LayoutSettings {
            x: origin.x,
            y: origin.y,
            line_height: 1.2,
            ..LayoutSettings::default()
        });
        layout.append(&[&font], &TextStyle::new(text, size, 0));
        Some(longest_line_width(&layout).max(size * 0.5) + 0.01)
    } else {
        style.width
    };
    layout.reset(&LayoutSettings {
        x: origin.x,
        y: origin.y,
        max_width: measured_width,
        horizontal_align: match style.align {
            TextAlign::Left => HorizontalAlign::Left,
            TextAlign::Center => HorizontalAlign::Center,
            TextAlign::Right => HorizontalAlign::Right,
        },
        line_height: 1.2,
        ..LayoutSettings::default()
    });
    layout.append(&[&font], &TextStyle::new(text, size, 0));
    Ok((font, layout))
}

fn longest_line_width(layout: &Layout) -> f32 {
    layout.lines().map_or(0.0, |lines| {
        lines
            .iter()
            .map(|line| {
                let glyphs = &layout.glyphs()[line.glyph_start..=line.glyph_end];
                let left = glyphs.first().map_or(0.0, |glyph| glyph.x);
                glyphs
                    .last()
                    .map_or(0.0, |glyph| glyph.x + glyph.width as f32 - left)
            })
            .fold(0.0, f32::max)
    })
}

fn text_glyph_bounds(layout: &Layout) -> Option<Bounds> {
    Bounds::from_points(
        layout
            .glyphs()
            .iter()
            .filter(|glyph| glyph.width > 0 && glyph.height > 0)
            .flat_map(|glyph| {
                [
                    Point {
                        x: glyph.x,
                        y: glyph.y,
                    },
                    Point {
                        x: glyph.x + glyph.width as f32,
                        y: glyph.y + glyph.height as f32,
                    },
                ]
            }),
    )
}

fn text_layout_bounds(
    origin: Point,
    font_size: f32,
    style: &TextStyleSettings,
    layout: &Layout,
) -> Bounds {
    let width = style
        .width
        .unwrap_or_else(|| longest_line_width(layout).max(font_size * 0.5));
    Bounds {
        x: origin.x,
        y: origin.y,
        width,
        height: layout.height().max(font_size * 1.2),
    }
}

fn expand_bounds(mut bounds: Bounds, left: f32, top: f32, right: f32, bottom: f32) -> Bounds {
    bounds.x -= left;
    bounds.y -= top;
    bounds.width += left + right;
    bounds.height += top + bottom;
    bounds
}

impl Layer {
    fn rotation_center(&self, fallback: Bounds) -> Point {
        if let Shape::Text {
            origin,
            text,
            font_size,
            font_data,
            style,
        } = &self.shape
            && let Ok((_, layout)) = text_layout(*origin, text, *font_size, font_data, style)
        {
            return if style.background.is_some() || text.is_empty() {
                text_layout_bounds(*origin, *font_size, style, &layout).center()
            } else {
                text_glyph_bounds(&layout)
                    .unwrap_or_else(|| text_layout_bounds(*origin, *font_size, style, &layout))
                    .center()
            };
        }
        fallback.center()
    }

    fn validate(&self) -> Result<(), String> {
        let point_ok = |p: &Point| p.x.is_finite() && p.y.is_finite();
        let valid_shape = match &self.shape {
            Shape::Freehand(points) => points.iter().all(point_ok),
            Shape::Polygon(points) => points.len() >= 3 && points.iter().all(point_ok),
            Shape::Line { start, end } | Shape::Arrow { start, end } => {
                point_ok(start) && point_ok(end)
            }
            Shape::Rectangle {
                origin,
                width,
                height,
            }
            | Shape::Ellipse {
                origin,
                width,
                height,
            } => {
                point_ok(origin)
                    && width.is_finite()
                    && height.is_finite()
                    && *width > 0.0
                    && *height > 0.0
            }
            Shape::Text {
                origin,
                font_size,
                style,
                ..
            } => {
                point_ok(origin)
                    && font_size.is_finite()
                    && *font_size > 0.0
                    && style
                        .width
                        .is_none_or(|width| width.is_finite() && width > 0.0)
                    && style.shadow.as_ref().is_none_or(|shadow| {
                        shadow.blur.is_finite() && shadow.blur >= 0.0 && point_ok(&shadow.offset)
                    })
            }
            Shape::Image {
                origin,
                width,
                height,
                pixels,
            } => {
                point_ok(origin)
                    && width.is_finite()
                    && height.is_finite()
                    && *width > 0.0
                    && *height > 0.0
                    && pixels.width() > 0
                    && pixels.height() > 0
            }
        };
        if !valid_shape
            || !self.rotation_degrees.is_finite()
            || !self.stroke_width.is_finite()
            || self.stroke_width < 0.0
        {
            return Err(format!("Layer {} has invalid geometry", self.id));
        }
        Ok(())
    }

    fn geometry_bounds(&self) -> Result<Option<Bounds>, String> {
        self.validate()?;
        Ok(match &self.shape {
            Shape::Freehand(points) | Shape::Polygon(points) => {
                Bounds::from_points(points.iter().copied())
            }
            Shape::Line { start, end } => Bounds::from_points([*start, *end]),
            Shape::Arrow { start, end } => {
                let [a, b] = arrow_head(*start, *end, self.stroke_width);
                Bounds::from_points([*start, *end, a, b])
            }
            Shape::Rectangle {
                origin,
                width,
                height,
            }
            | Shape::Ellipse {
                origin,
                width,
                height,
            }
            | Shape::Image {
                origin,
                width,
                height,
                ..
            } => Some(Bounds {
                x: origin.x,
                y: origin.y,
                width: *width,
                height: *height,
            }),
            Shape::Text {
                origin,
                text,
                font_size,
                font_data,
                style,
            } => {
                let (_, layout) = text_layout(*origin, text, *font_size, font_data, style)?;
                let mut bounds = text_layout_bounds(*origin, *font_size, style, &layout);
                if style.background.is_some() {
                    let pad_x = font_size * 0.28;
                    let pad_y = font_size * 0.18;
                    bounds = expand_bounds(bounds, pad_x, pad_y, pad_x, pad_y);
                } else if text.is_empty() {
                    // Empty text remains an editable one-line layout box.
                } else if let Some(glyphs) = text_glyph_bounds(&layout) {
                    bounds = glyphs;
                }
                let outline = if style.outlined {
                    (font_size * 0.08).max(1.5)
                } else {
                    0.0
                };
                bounds = expand_bounds(bounds, outline, outline, outline, outline);
                if style.italic {
                    bounds = expand_bounds(bounds, font_size * 0.2, 0.0, font_size * 0.2, 0.0);
                }
                if let Some(shadow) = &style.shadow {
                    let spread = shadow.blur * 2.0;
                    bounds = expand_bounds(
                        bounds,
                        (spread - shadow.offset.x).max(0.0),
                        (spread - shadow.offset.y).max(0.0),
                        (spread + shadow.offset.x).max(0.0),
                        (spread + shadow.offset.y).max(0.0),
                    );
                }
                Some(bounds)
            }
        })
    }

    /// Axis-aligned bounds after rotation, including the stroke. Empty or invalid
    /// geometry (including an invalid font) has no bounds.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut bounds = self.geometry_bounds().ok()??;
        let center = self.rotation_center(bounds);
        if !matches!(self.shape, Shape::Text { .. } | Shape::Image { .. }) {
            bounds.x -= self.stroke_width / 2.0;
            bounds.y -= self.stroke_width / 2.0;
            bounds.width += self.stroke_width;
            bounds.height += self.stroke_width;
        }
        Bounds::from_points(
            bounds
                .corners()
                .map(|p| rotate(p, center, self.rotation_degrees)),
        )
    }

    /// Hit testing uses source pixels. Unfilled shapes select their outline,
    /// not their empty interior; text and images select their bounding rectangle.
    pub fn hit_test(&self, point: Point, tolerance: f32) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() || !tolerance.is_finite() || tolerance < 0.0
        {
            return false;
        }
        let Ok(Some(bounds)) = self.geometry_bounds() else {
            return false;
        };
        let point = rotate(point, self.rotation_center(bounds), -self.rotation_degrees);
        let radius = self.stroke_width / 2.0 + tolerance;
        let near = |a, b| segment_distance(point, a, b) <= radius;
        match &self.shape {
            Shape::Freehand(points) => {
                points.windows(2).any(|p| near(p[0], p[1]))
                    || (points.len() == 1 && near(points[0], points[0]))
            }
            Shape::Polygon(points) => {
                let mut winding = 0;
                for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
                    if near(a, b) {
                        return true;
                    }
                    let side = (b.x - a.x) * (point.y - a.y) - (point.x - a.x) * (b.y - a.y);
                    if a.y <= point.y && b.y > point.y && side > 0.0 {
                        winding += 1;
                    }
                    if a.y > point.y && b.y <= point.y && side < 0.0 {
                        winding -= 1;
                    }
                }
                self.fill.is_some() && winding != 0
            }
            Shape::Line { start, end } => near(*start, *end),
            Shape::Arrow { start, end } => {
                let [a, b] = arrow_head(*start, *end, self.stroke_width);
                near(*start, *end) || near(a, *end) || near(b, *end)
            }
            Shape::Rectangle { .. } => {
                let [a, b, c, d] = bounds.corners();
                (self.fill.is_some()
                    && point.x >= a.x
                    && point.x <= c.x
                    && point.y >= a.y
                    && point.y <= c.y)
                    || near(a, b)
                    || near(b, c)
                    || near(c, d)
                    || near(d, a)
            }
            Shape::Ellipse { .. } => {
                let center = bounds.center();
                let (x, y) = (point.x - center.x, point.y - center.y);
                let (rx, ry) = (bounds.width / 2.0, bounds.height / 2.0);
                let outer = (x / (rx + radius)).powi(2) + (y / (ry + radius)).powi(2) <= 1.0;
                outer
                    && (self.fill.is_some()
                        || rx <= radius
                        || ry <= radius
                        || (x / (rx - radius)).powi(2) + (y / (ry - radius)).powi(2) >= 1.0)
            }
            Shape::Text { .. } | Shape::Image { .. } => {
                point.x >= bounds.x - tolerance
                    && point.x <= bounds.x + bounds.width + tolerance
                    && point.y >= bounds.y - tolerance
                    && point.y <= bounds.y + bounds.height + tolerance
            }
        }
    }
}

fn paint(color: [u8; 4], blend_mode: BlendMode) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = true;
    paint.blend_mode = blend_mode.raster();
    paint
}

fn colorized_mask(mask: &[u8], width: u32, height: u32, color: [u8; 4]) -> Result<Pixmap, String> {
    let mut pixmap = Pixmap::new(width, height).ok_or("Text mask is too large")?;
    for (pixel, &coverage) in pixmap.pixels_mut().iter_mut().zip(mask) {
        let alpha = ((u16::from(coverage) * u16::from(color[3]) + 127) / 255) as u8;
        *pixel = tiny_skia::ColorU8::from_rgba(color[0], color[1], color[2], alpha).premultiply();
    }
    Ok(pixmap)
}

fn gaussian_blur(mask: &[u8], width: usize, height: usize, blur: f32) -> Vec<u8> {
    let sigma = (blur / 2.0).max(0.01);
    let radius = (sigma * 3.0).ceil().min(48.0) as isize;
    if radius == 0 {
        return mask.to_vec();
    }
    let mut kernel = (-radius..=radius)
        .map(|offset| (-0.5 * (offset as f32 / sigma).powi(2)).exp())
        .collect::<Vec<_>>();
    let sum: f32 = kernel.iter().sum();
    for value in &mut kernel {
        *value /= sum;
    }
    let mut horizontal = vec![0.0; mask.len()];
    for y in 0..height {
        for x in 0..width {
            horizontal[y * width + x] = (-radius..=radius)
                .map(|offset| {
                    let sx = (x as isize + offset).clamp(0, width as isize - 1) as usize;
                    f32::from(mask[y * width + sx]) * kernel[(offset + radius) as usize]
                })
                .sum();
        }
    }
    let mut output = vec![0; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let value: f32 = (-radius..=radius)
                .map(|offset| {
                    let sy = (y as isize + offset).clamp(0, height as isize - 1) as usize;
                    horizontal[sy * width + x] * kernel[(offset + radius) as usize]
                })
                .sum();
            output[y * width + x] = value.round().clamp(0.0, 255.0) as u8;
        }
    }
    output
}

fn dilated_ring(mask: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let mut output = vec![0; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let mut maximum = 0;
            for oy in -(radius as isize)..=radius as isize {
                for ox in -(radius as isize)..=radius as isize {
                    if ox * ox + oy * oy > (radius * radius) as isize {
                        continue;
                    }
                    let sx = x as isize + ox;
                    let sy = y as isize + oy;
                    if sx >= 0 && sy >= 0 && sx < width as isize && sy < height as isize {
                        maximum = maximum.max(mask[sy as usize * width + sx as usize]);
                    }
                }
            }
            output[y * width + x] = maximum.saturating_sub(mask[y * width + x]);
        }
    }
    output
}

fn draw_layer(canvas: &mut Pixmap, layer: &Layer) -> Result<(), String> {
    let Some(bounds) = layer.geometry_bounds()? else {
        return Ok(());
    };
    let center = layer.rotation_center(bounds);
    let transform = Transform::from_rotate_at(layer.rotation_degrees, center.x, center.y);
    if let Shape::Image {
        origin,
        width,
        height,
        pixels,
    } = &layer.shape
    {
        let mut bitmap =
            Pixmap::new(pixels.width(), pixels.height()).ok_or("Image layer is too large")?;
        for (destination, source) in bitmap.pixels_mut().iter_mut().zip(pixels.pixels()) {
            *destination =
                tiny_skia::ColorU8::from_rgba(source[0], source[1], source[2], source[3])
                    .premultiply();
        }
        canvas.draw_pixmap(
            0,
            0,
            bitmap.as_ref(),
            &tiny_skia::PixmapPaint {
                opacity: f32::from(layer.color[3]) / 255.0,
                quality: tiny_skia::FilterQuality::Bilinear,
                blend_mode: layer.blend_mode.raster(),
            },
            Transform::from_scale(
                *width / pixels.width() as f32,
                *height / pixels.height() as f32,
            )
            .post_translate(origin.x, origin.y)
            .post_concat(transform),
            None,
        );
        return Ok(());
    }
    if let Shape::Text {
        origin,
        text,
        font_size,
        font_data,
        style,
    } = &layer.shape
    {
        let (font, layout) = text_layout(*origin, text, *font_size, font_data, style)?;
        let layout_bounds = text_layout_bounds(*origin, *font_size, style, &layout);
        let line_height = font_size * 1.2;
        let box_width = layout_bounds.width;
        let box_height = layout.height().max(line_height);
        let pad_x = font_size * 0.28;
        let pad_y = font_size * 0.18;
        let plate = tiny_skia::Rect::from_xywh(
            origin.x - pad_x,
            origin.y - pad_y,
            box_width + pad_x * 2.0,
            box_height + pad_y * 2.0,
        );
        let width = canvas.width();
        let height = canvas.height();
        let mut plate_mask = Pixmap::new(width, height).ok_or("Text mask is too large")?;
        let draw_plate = |canvas: &mut Pixmap| {
            if let Some(rect) = plate {
                let mut path = PathBuilder::new();
                if style.rounded_background {
                    let radius = (font_size * 0.32)
                        .min(rect.width() / 2.0)
                        .min(rect.height() / 2.0);
                    path.push_rect(
                        tiny_skia::Rect::from_xywh(
                            rect.x() + radius,
                            rect.y(),
                            (rect.width() - radius * 2.0).max(f32::EPSILON),
                            rect.height(),
                        )
                        .expect("positive rounded plate interior"),
                    );
                    path.push_rect(
                        tiny_skia::Rect::from_xywh(
                            rect.x(),
                            rect.y() + radius,
                            rect.width(),
                            (rect.height() - radius * 2.0).max(f32::EPSILON),
                        )
                        .expect("positive rounded plate interior"),
                    );
                    for (x, y) in [
                        (rect.x() + radius, rect.y() + radius),
                        (rect.right() - radius, rect.y() + radius),
                        (rect.x() + radius, rect.bottom() - radius),
                        (rect.right() - radius, rect.bottom() - radius),
                    ] {
                        path.push_circle(x, y, radius);
                    }
                } else {
                    path.push_rect(rect);
                }
                if let Some(path) = path.finish() {
                    canvas.fill_path(
                        &path,
                        &paint([255, 255, 255, 255], BlendMode::Normal),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        };
        if style.background.is_some() {
            draw_plate(&mut plate_mask);
        }
        let mut glyph_mask = Pixmap::new(width, height).ok_or("Text mask is too large")?;
        for glyph in layout
            .glyphs()
            .iter()
            .filter(|g| g.width > 0 && g.height > 0)
        {
            let (_, coverage) = font.rasterize_config(glyph.key);
            let mut pixels = Pixmap::new(glyph.width as u32, glyph.height as u32)
                .ok_or("Text glyph is too large")?;
            for (pixel, coverage) in pixels.pixels_mut().iter_mut().zip(coverage) {
                *pixel = tiny_skia::ColorU8::from_rgba(255, 255, 255, coverage).premultiply();
            }
            let bold = i32::from(style.bold);
            for ox in 0..=bold {
                let glyph_transform = if style.italic {
                    Transform::from_skew(-0.20, 0.0)
                        .post_translate(glyph.x + ox as f32 + glyph.height as f32 * 0.20, glyph.y)
                } else {
                    Transform::from_translate(glyph.x + ox as f32, glyph.y)
                };
                glyph_mask.draw_pixmap(
                    0,
                    0,
                    pixels.as_ref(),
                    &tiny_skia::PixmapPaint {
                        blend_mode: tiny_skia::BlendMode::SourceOver,
                        ..Default::default()
                    },
                    glyph_transform,
                    None,
                );
            }
        }
        let glyph_alpha = glyph_mask
            .pixels()
            .iter()
            .map(|pixel| pixel.alpha())
            .collect::<Vec<_>>();
        let ink_alpha = if style.outlined {
            dilated_ring(
                &glyph_alpha,
                width as usize,
                height as usize,
                (font_size * 0.08).max(1.5).round() as usize,
            )
        } else {
            glyph_alpha
        };
        let shadow_source = if style.background.is_some() {
            plate_mask
                .pixels()
                .iter()
                .map(|pixel| pixel.alpha())
                .collect::<Vec<_>>()
        } else {
            ink_alpha.clone()
        };
        if let Some(shadow) = &style.shadow {
            let blurred =
                gaussian_blur(&shadow_source, width as usize, height as usize, shadow.blur);
            let mut color = shadow.color;
            color[3] = ((u16::from(color[3]) * u16::from(layer.color[3]) + 127) / 255) as u8;
            let pixmap = colorized_mask(&blurred, width, height, color)?;
            canvas.draw_pixmap(
                0,
                0,
                pixmap.as_ref(),
                &tiny_skia::PixmapPaint {
                    blend_mode: layer.blend_mode.raster(),
                    ..Default::default()
                },
                Transform::from_translate(shadow.offset.x, shadow.offset.y).post_concat(transform),
                None,
            );
        }
        if let Some(mut background) = style.background {
            background[3] =
                ((u16::from(background[3]) * u16::from(layer.color[3]) + 127) / 255) as u8;
            let alpha = plate_mask
                .pixels()
                .iter()
                .map(|pixel| pixel.alpha())
                .collect::<Vec<_>>();
            let pixmap = colorized_mask(&alpha, width, height, background)?;
            canvas.draw_pixmap(
                0,
                0,
                pixmap.as_ref(),
                &tiny_skia::PixmapPaint {
                    blend_mode: layer.blend_mode.raster(),
                    ..Default::default()
                },
                transform,
                None,
            );
        }
        let pixmap = colorized_mask(&ink_alpha, width, height, layer.color)?;
        canvas.draw_pixmap(
            0,
            0,
            pixmap.as_ref(),
            &tiny_skia::PixmapPaint {
                blend_mode: layer.blend_mode.raster(),
                ..Default::default()
            },
            transform,
            None,
        );
        return Ok(());
    }

    let mut path = PathBuilder::new();
    let mut closed = false;
    match &layer.shape {
        Shape::Freehand(points) | Shape::Polygon(points) => {
            if points.len() == 1 {
                if layer.stroke_width > 0.0 {
                    path.push_circle(points[0].x, points[0].y, layer.stroke_width / 2.0);
                    if let Some(path) = path.finish() {
                        canvas.fill_path(
                            &path,
                            &paint(layer.color, layer.blend_mode),
                            FillRule::Winding,
                            transform,
                            None,
                        );
                    }
                }
                return Ok(());
            }
            path.move_to(points[0].x, points[0].y);
            for point in &points[1..] {
                path.line_to(point.x, point.y);
            }
            if matches!(layer.shape, Shape::Polygon(_)) {
                path.close();
                closed = true;
            }
        }
        Shape::Line { start, end } | Shape::Arrow { start, end } => {
            path.move_to(start.x, start.y);
            path.line_to(end.x, end.y);
            if matches!(layer.shape, Shape::Arrow { .. }) {
                let [a, b] = arrow_head(*start, *end, layer.stroke_width);
                path.move_to(a.x, a.y);
                path.line_to(end.x, end.y);
                path.line_to(b.x, b.y);
            }
        }
        Shape::Rectangle {
            origin,
            width,
            height,
        }
        | Shape::Ellipse {
            origin,
            width,
            height,
        } => {
            let rect = tiny_skia::Rect::from_xywh(origin.x, origin.y, *width, *height)
                .ok_or("Invalid shape bounds")?;
            if matches!(layer.shape, Shape::Rectangle { .. }) {
                path.push_rect(rect);
            } else {
                path.push_oval(rect);
            }
            closed = true;
        }
        Shape::Text { .. } | Shape::Image { .. } => unreachable!(),
    }
    let Some(path) = path.finish() else {
        return Ok(());
    };
    if closed && let Some(color) = layer.fill {
        canvas.fill_path(
            &path,
            &paint(color, layer.blend_mode),
            FillRule::Winding,
            transform,
            None,
        );
    }
    if layer.stroke_width > 0.0 {
        let stroke = Stroke {
            width: layer.stroke_width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        canvas.stroke_path(
            &path,
            &paint(layer.color, layer.blend_mode),
            &stroke,
            transform,
            None,
        );
    }
    Ok(())
}

pub fn render(document: &Document) -> Result<RgbaImage, String> {
    let (width, height) = document.source.dimensions();
    if width == 0 || height == 0 {
        return Err("Source image is empty".into());
    }
    if let Some(crop) = document.crop
        && (crop.width == 0
            || crop.height == 0
            || crop.x.checked_add(crop.width).is_none_or(|x| x > width)
            || crop.y.checked_add(crop.height).is_none_or(|y| y > height))
    {
        return Err("Crop must be nonempty and inside the source image".into());
    }
    let mut output = (*document.source).clone();
    if !document.layers.is_empty() {
        let mut canvas = Pixmap::new(width, height).ok_or("Source image is too large")?;
        let blend_source = document
            .layers
            .iter()
            .any(|layer| layer.blend_mode != BlendMode::Normal);
        if blend_source {
            // Non-normal blending needs the source image as its backdrop,
            // not a transparent annotation plane composed over it afterward.
            for (pixel, source) in canvas.pixels_mut().iter_mut().zip(output.pixels()) {
                *pixel = tiny_skia::ColorU8::from_rgba(source[0], source[1], source[2], source[3])
                    .premultiply();
            }
        }
        for layer in &document.layers {
            draw_layer(&mut canvas, layer)?;
        }
        if blend_source {
            for (source, rendered) in output.pixels_mut().zip(canvas.pixels()) {
                let original =
                    tiny_skia::ColorU8::from_rgba(source[0], source[1], source[2], source[3])
                        .premultiply();
                // Keep untouched source pixels, including hidden RGB and low
                // alpha, exact rather than round-tripping them through skia.
                if original != *rendered {
                    let color = rendered.demultiply();
                    source.0 = [color.red(), color.green(), color.blue(), color.alpha()];
                }
            }
        } else {
            // Match imageops::overlay's Pixel::blend operation without first
            // materializing a second full-size RgbaImage. Pixel::blend is a
            // no-op for transparent foreground pixels, so avoid demultiplying
            // and visiting the output for the common untouched case.
            for (source, rendered) in output.pixels_mut().zip(canvas.pixels()) {
                if rendered.alpha() != 0 {
                    let color = rendered.demultiply();
                    source.blend(&Rgba([
                        color.red(),
                        color.green(),
                        color.blue(),
                        color.alpha(),
                    ]));
                }
            }
        }
    }
    if let Some(crop) = document.crop {
        output =
            image::imageops::crop_imm(&output, crop.x, crop.y, crop.width, crop.height).to_image();
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::{hint::black_box, sync::Arc, time::Instant};

    use super::*;

    fn text_document(style: TextStyleSettings) -> Document {
        Document {
            source: Arc::new(RgbaImage::from_pixel(360, 180, Rgba([255, 255, 255, 255]))),
            crop: None,
            layers: vec![Layer {
                id: 1,
                shape: Shape::Text {
                    origin: Point { x: 30.0, y: 25.0 },
                    text: "LLLL LLLL\nL L".into(),
                    font_size: 26.0,
                    font_data: Arc::from(include_bytes!("../tests/test-font.ttf").as_slice()),
                    style,
                },
                color: [220, 20, 30, 255],
                stroke_width: 0.0,
                fill: None,
                rotation_degrees: 0.0,
                blend_mode: BlendMode::Normal,
            }],
        }
    }

    #[test]
    fn outline_mask_keeps_glyph_interiors_empty_and_only_paints_the_edge() {
        let mut filled = vec![0; 49];
        for y in 2..=4 {
            for x in 2..=4 {
                filled[y * 7 + x] = 255;
            }
        }
        let outline = dilated_ring(&filled, 7, 7, 1);
        assert_eq!(outline[3 * 7 + 3], 0, "interior is hollow");
        assert_eq!(outline[2 * 7 + 2], 0, "original glyph ink is removed");
        assert_eq!(
            outline[3 * 7 + 1],
            255,
            "edge extends by the requested radius"
        );
        assert_eq!(outline[7 + 1], 0, "disk dilation excludes diagonal corners");
    }

    #[test]
    fn blur_spreads_alpha_without_multiplying_shadow_opacity() {
        let mut mask = vec![0; 31 * 31];
        for y in 14..=16 {
            for x in 14..=16 {
                mask[y * 31 + x] = 128;
            }
        }
        let sharp = gaussian_blur(&mask, 31, 31, 0.);
        let soft = gaussian_blur(&mask, 31, 31, 4.);
        assert_eq!(sharp, mask);
        assert!(soft[15 * 31 + 15] < 128);
        assert!(soft[15 * 31 + 12] > 0);
        let total: i32 = soft.iter().map(|&alpha| i32::from(alpha)).sum();
        assert!(
            (total - 9 * 128).abs() < 50,
            "alpha changed from 1152 to {total}"
        );
    }

    #[test]
    fn empty_text_is_a_valid_empty_render_with_editable_bounds() {
        let mut document = text_document(TextStyleSettings::default());
        let Shape::Text { text, .. } = &mut document.layers[0].shape else {
            unreachable!()
        };
        text.clear();
        assert!(document.layers[0].bounds().is_some());
        assert_eq!(render(&document).unwrap(), *document.source);
    }

    #[test]
    fn auto_width_plate_adds_background_padding_once() {
        let document = text_document(TextStyleSettings {
            background: Some([0, 0, 0, 255]),
            ..Default::default()
        });
        let layer = &document.layers[0];
        let Shape::Text {
            origin,
            text,
            font_size,
            font_data,
            style,
        } = &layer.shape
        else {
            unreachable!()
        };
        let (_, layout) = text_layout(*origin, text, *font_size, font_data, style).unwrap();
        let bounds = layer.geometry_bounds().unwrap().unwrap();
        assert!((bounds.width - (longest_line_width(&layout) + font_size * 0.56)).abs() < 0.1);
    }

    #[test]
    fn styled_text_alignment_wrap_plate_outline_and_shadow_render_pixels() {
        let base = render(&text_document(TextStyleSettings {
            width: Some(210.0),
            ..Default::default()
        }))
        .unwrap();
        let styled = render(&text_document(TextStyleSettings {
            bold: true,
            italic: true,
            align: TextAlign::Right,
            width: Some(210.0),
            background: Some([15, 30, 60, 255]),
            rounded_background: true,
            outlined: true,
            shadow: Some(TextShadow {
                color: [0, 0, 0, 150],
                blur: 5.0,
                offset: Point { x: 4.0, y: 6.0 },
            }),
        }))
        .unwrap();
        assert_ne!(base, styled);
        assert_eq!(styled.get_pixel(30, 25).0, [15, 30, 60, 255]);
        // Rounded plate leaves its extreme padded corner untouched.
        assert_eq!(styled.get_pixel(23, 21).0, [255, 255, 255, 255]);
        assert!(styled.pixels().any(|pixel| {
            pixel[0] < 255 && pixel[0] == pixel[1] && pixel[1] == pixel[2] && pixel[3] == 255
        }));

        let centered = render(&text_document(TextStyleSettings {
            align: TextAlign::Center,
            width: Some(210.0),
            ..Default::default()
        }))
        .unwrap();
        let right = render(&text_document(TextStyleSettings {
            align: TextAlign::Right,
            width: Some(210.0),
            ..Default::default()
        }))
        .unwrap();
        assert_ne!(
            centered, right,
            "asymmetric lines must move independently with alignment"
        );
    }

    fn legacy_render(document: &Document) -> RgbaImage {
        let (width, height) = document.source.dimensions();
        let mut output = (*document.source).clone();
        let mut canvas = Pixmap::new(width, height).unwrap();
        for layer in &document.layers {
            draw_layer(&mut canvas, layer).unwrap();
        }
        let bytes = canvas
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect();
        let overlay = RgbaImage::from_raw(width, height, bytes).unwrap();
        image::imageops::overlay(&mut output, &overlay, 0, 0);
        if let Some(crop) = document.crop {
            output = image::imageops::crop_imm(&output, crop.x, crop.y, crop.width, crop.height)
                .to_image();
        }
        output
    }

    fn normal_document(width: u32, height: u32, dense: bool) -> Document {
        let source = RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                (x.wrapping_mul(37).wrapping_add(y * 11)) as u8,
                (x.wrapping_mul(3).wrapping_add(y * 29)) as u8,
                (x ^ y).wrapping_add(91) as u8,
                (x * 17 + y * 13) as u8,
            ])
        });
        let count = if dense { 96 } else { 3 };
        let mut layers = Vec::with_capacity(count);
        for index in 0..count {
            let index = index as u32;
            let mut layer = Layer {
                id: u64::from(index),
                shape: if index.is_multiple_of(3) {
                    Shape::Ellipse {
                        origin: Point {
                            x: (index * 83 % width.saturating_sub(300).max(1)) as f32 + 0.35,
                            y: (index * 47 % height.saturating_sub(220).max(1)) as f32 + 0.7,
                        },
                        width: if dense { 900.5 } else { 180.5 },
                        height: if dense { 620.25 } else { 120.25 },
                    }
                } else {
                    Shape::Rectangle {
                        origin: Point {
                            x: (index * 101 % width.saturating_sub(400).max(1)) as f32 + 0.2,
                            y: (index * 61 % height.saturating_sub(300).max(1)) as f32 + 0.4,
                        },
                        width: if dense { 1200.75 } else { 240.75 },
                        height: if dense { 800.5 } else { 160.5 },
                    }
                },
                color: [17, 211, 93, 137],
                stroke_width: 5.25,
                fill: Some([231, 41, 167, 89]),
                rotation_degrees: index as f32 * 13.7 + 17.25,
                blend_mode: BlendMode::Normal,
            };
            if index % 5 == 4 {
                layer.fill = None;
            }
            layers.push(layer);
        }
        Document {
            source: Arc::new(source),
            crop: None,
            layers,
        }
    }

    #[test]
    fn normal_compositing_is_byte_exact_with_legacy_overlay() {
        for dense in [false, true] {
            // Sparse includes untouched pixels and partial coverage; dense
            // saturates overlapping alpha. Test both before and after cropping.
            let mut document = normal_document(641, 479, dense);
            document.layers.truncate(16);
            assert_eq!(render(&document).unwrap(), legacy_render(&document));
            document.crop = Some(PixelRect {
                x: 19,
                y: 11,
                width: 537,
                height: 403,
            });
            assert_eq!(render(&document).unwrap(), legacy_render(&document));
        }
    }

    #[test]
    #[ignore = "manual release benchmark"]
    fn benchmark_normal_compositing_4k() {
        for (name, dense) in [("sparse", false), ("dense", true)] {
            let document = normal_document(3840, 2160, dense);
            for (implementation, render_once) in [
                (
                    "direct",
                    render as fn(&Document) -> Result<RgbaImage, String>,
                ),
                ("legacy", |document: &Document| Ok(legacy_render(document))),
            ] {
                let started = Instant::now();
                for _ in 0..3 {
                    black_box(render_once(black_box(&document)).unwrap());
                }
                eprintln!(
                    "4k {name} {implementation}: {:.3} ms/render (3 renders)",
                    started.elapsed().as_secs_f64() * 1000.0 / 3.0
                );
            }
        }
    }
}
