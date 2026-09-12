//! Toolkit-independent annotation rendering in source-image pixel coordinates.
//! Layers are composited in document order, then the crop is applied. This crate
//! owns neither editor state nor file/clipboard operations.

use std::sync::Arc;

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use image::RgbaImage;
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
    },
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
) -> Result<(fontdue::Font, Layout), String> {
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
        .map_err(str::to_owned)?;
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        x: origin.x,
        y: origin.y,
        ..LayoutSettings::default()
    });
    layout.append(&[&font], &TextStyle::new(text, size, 0));
    Ok((font, layout))
}

impl Layer {
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
                origin, font_size, ..
            } => point_ok(origin) && font_size.is_finite() && *font_size > 0.0,
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
            } => {
                let (_, layout) = text_layout(*origin, text, *font_size, font_data)?;
                Bounds::from_points(
                    layout
                        .glyphs()
                        .iter()
                        .filter(|g| g.width > 0 && g.height > 0)
                        .flat_map(|g| {
                            [
                                Point { x: g.x, y: g.y },
                                Point {
                                    x: g.x + g.width as f32,
                                    y: g.y + g.height as f32,
                                },
                            ]
                        }),
                )
            }
        })
    }

    /// Axis-aligned bounds after rotation, including the stroke. Empty or invalid
    /// geometry (including an invalid font) has no bounds.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut bounds = self.geometry_bounds().ok()??;
        let center = bounds.center();
        if !matches!(self.shape, Shape::Text { .. }) {
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
    /// not their empty interior; text selects its glyph bounding rectangle.
    pub fn hit_test(&self, point: Point, tolerance: f32) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() || !tolerance.is_finite() || tolerance < 0.0
        {
            return false;
        }
        let Ok(Some(bounds)) = self.geometry_bounds() else {
            return false;
        };
        let point = rotate(point, bounds.center(), -self.rotation_degrees);
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
            Shape::Text { .. } => {
                point.x >= bounds.x - tolerance
                    && point.x <= bounds.x + bounds.width + tolerance
                    && point.y >= bounds.y - tolerance
                    && point.y <= bounds.y + bounds.height + tolerance
            }
        }
    }
}

fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = true;
    paint
}

fn draw_layer(canvas: &mut Pixmap, layer: &Layer) -> Result<(), String> {
    let Some(bounds) = layer.geometry_bounds()? else {
        return Ok(());
    };
    let center = bounds.center();
    let transform = Transform::from_rotate_at(layer.rotation_degrees, center.x, center.y);
    if let Shape::Text {
        origin,
        text,
        font_size,
        font_data,
    } = &layer.shape
    {
        let (font, layout) = text_layout(*origin, text, *font_size, font_data)?;
        for glyph in layout
            .glyphs()
            .iter()
            .filter(|g| g.width > 0 && g.height > 0)
        {
            let (_, coverage) = font.rasterize_config(glyph.key);
            let mut pixels = Pixmap::new(glyph.width as u32, glyph.height as u32)
                .ok_or("Text glyph is too large")?;
            for (pixel, coverage) in pixels.pixels_mut().iter_mut().zip(coverage) {
                let alpha = ((u16::from(coverage) * u16::from(layer.color[3]) + 127) / 255) as u8;
                *pixel = tiny_skia::ColorU8::from_rgba(
                    layer.color[0],
                    layer.color[1],
                    layer.color[2],
                    alpha,
                )
                .premultiply();
            }
            canvas.draw_pixmap(
                0,
                0,
                pixels.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::from_translate(glyph.x, glyph.y).post_concat(transform),
                None,
            );
        }
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
                            &paint(layer.color),
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
        Shape::Text { .. } => unreachable!(),
    }
    let Some(path) = path.finish() else {
        return Ok(());
    };
    if closed && let Some(color) = layer.fill {
        canvas.fill_path(&path, &paint(color), FillRule::Winding, transform, None);
    }
    if layer.stroke_width > 0.0 {
        let stroke = Stroke {
            width: layer.stroke_width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        canvas.stroke_path(&path, &paint(layer.color), &stroke, transform, None);
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
        for layer in &document.layers {
            draw_layer(&mut canvas, layer)?;
        }
        // tiny-skia uses premultiplied RGBA; image uses straight RGBA. Preserve
        // source pixels exactly rather than round-tripping them through skia.
        let bytes = canvas
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        let overlay = RgbaImage::from_raw(width, height, bytes).ok_or("Invalid rendered buffer")?;
        image::imageops::overlay(&mut output, &overlay, 0, 0);
    }
    if let Some(crop) = document.crop {
        output =
            image::imageops::crop_imm(&output, crop.x, crop.y, crop.width, crop.height).to_image();
    }
    Ok(output)
}
