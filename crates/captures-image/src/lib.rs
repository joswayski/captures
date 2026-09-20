//! Toolkit-independent annotation rendering in source-image pixel coordinates.
//! Layers are composited in document order, then the crop is applied. This crate
//! owns neither editor state nor file/clipboard operations.

mod encoding;
pub use encoding::{composite_onto_white, encode_jpeg, encode_webp};

use std::sync::Arc;

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use image::{Pixel, Rgba, RgbaImage};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, PathSegment, Pixmap, Stroke, Transform,
};

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
    /// Shipping freehand smoothing: midpoint quadratics followed by a final line.
    SmoothPath(Vec<Point>),
    /// Shipping line controls: straight, one quadratic, or midpoint quadratics
    /// with a final controlled segment into the endpoint.
    ControlledPath(Vec<Point>),
    /// A closed contour, including concave shapes such as stars. Uses nonzero winding.
    Polygon(Vec<Point>),
    /// Shipping tapered arrow polygon with a mitered hairline outline.
    TaperedArrow(Vec<Point>),
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
    RoundedRectangle {
        origin: Point,
        width: f32,
        height: f32,
        radius: f32,
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
    /// Straight-alpha pixels fitted to document-space bounds. Layer color's
    /// alpha controls opacity; its RGB, stroke width and fill are ignored.
    Image {
        origin: Point,
        width: f32,
        height: f32,
        pixels: Arc<RgbaImage>,
    },
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
    /// Overrides the geometry center when authored bounds exceed the painted path.
    pub rotation_origin: Option<Point>,
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

fn tapered_arrow_stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Miter,
        miter_limit: 2.4,
        ..Stroke::default()
    }
}

fn tapered_arrow_outline(points: &[Point], width: f32) -> Option<Path> {
    if width <= 0.0 {
        return None;
    }
    let first = points.first()?;
    let mut builder = PathBuilder::new();
    builder.move_to(first.x, first.y);
    for point in &points[1..] {
        builder.line_to(point.x, point.y);
    }
    builder.close();
    builder.finish()?.stroke(&tapered_arrow_stroke(width), 1.0)
}

fn path_contains(path: &Path, point: Point) -> bool {
    let mut winding = 0;
    let mut previous = Point::default();
    let mut segments = path.segments();
    segments.set_auto_close(true);
    for segment in segments {
        match segment {
            PathSegment::MoveTo(to) => previous = Point { x: to.x, y: to.y },
            PathSegment::LineTo(to) => {
                let to = Point { x: to.x, y: to.y };
                let side = (to.x - previous.x) * (point.y - previous.y)
                    - (point.x - previous.x) * (to.y - previous.y);
                if previous.y <= point.y && to.y > point.y && side > 0.0 {
                    winding += 1;
                }
                if previous.y > point.y && to.y <= point.y && side < 0.0 {
                    winding -= 1;
                }
                previous = to;
            }
            PathSegment::Close => {}
            PathSegment::QuadTo(..) | PathSegment::CubicTo(..) => {
                debug_assert!(false, "polygon stroke unexpectedly produced a curve");
                return false;
            }
        }
    }
    winding != 0
}

fn polygon_contains(points: &[Point], point: Point) -> bool {
    let mut winding = 0;
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
        let side = (b.x - a.x) * (point.y - a.y) - (point.x - a.x) * (b.y - a.y);
        if a.y <= point.y && b.y > point.y && side > 0.0 {
            winding += 1;
        }
        if a.y > point.y && b.y <= point.y && side < 0.0 {
            winding -= 1;
        }
    }
    winding != 0
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
            Shape::Freehand(points) | Shape::SmoothPath(points) => points.iter().all(point_ok),
            Shape::ControlledPath(points) => points.len() >= 2 && points.iter().all(point_ok),
            Shape::Polygon(points) | Shape::TaperedArrow(points) => {
                points.len() >= 3 && points.iter().all(point_ok)
            }
            Shape::Line { start, end } | Shape::Arrow { start, end } => {
                point_ok(start) && point_ok(end)
            }
            Shape::Rectangle {
                origin,
                width,
                height,
            }
            | Shape::RoundedRectangle {
                origin,
                width,
                height,
                ..
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
            || self.rotation_origin.is_some_and(|point| !point_ok(&point))
            || !self.stroke_width.is_finite()
            || self.stroke_width < 0.0
            || matches!(&self.shape, Shape::RoundedRectangle { radius, .. } if !radius.is_finite() || *radius < 0.0)
        {
            return Err(format!("Layer {} has invalid geometry", self.id));
        }
        Ok(())
    }

    fn geometry_bounds(&self) -> Result<Option<Bounds>, String> {
        self.validate()?;
        Ok(match &self.shape {
            Shape::Freehand(points) | Shape::Polygon(points) | Shape::TaperedArrow(points) => {
                Bounds::from_points(points.iter().copied())
            }
            Shape::SmoothPath(points) => Bounds::from_points(path_samples(points, false)),
            Shape::ControlledPath(points) => Bounds::from_points(path_samples(points, true)),
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
            | Shape::RoundedRectangle {
                origin,
                width,
                height,
                ..
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
        let geometry_bounds = self.geometry_bounds().ok()??;
        let center = self
            .rotation_origin
            .unwrap_or_else(|| geometry_bounds.center());
        let mut painted_bounds = geometry_bounds;
        if let Shape::TaperedArrow(points) = &self.shape {
            if let Some(outline) = tapered_arrow_outline(points, self.stroke_width) {
                let bounds = outline.bounds();
                painted_bounds = Bounds {
                    x: bounds.x(),
                    y: bounds.y(),
                    width: bounds.width(),
                    height: bounds.height(),
                };
            }
        } else if !matches!(self.shape, Shape::Text { .. } | Shape::Image { .. }) {
            painted_bounds.x -= self.stroke_width / 2.0;
            painted_bounds.y -= self.stroke_width / 2.0;
            painted_bounds.width += self.stroke_width;
            painted_bounds.height += self.stroke_width;
        }
        Bounds::from_points(
            painted_bounds
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
        let point = rotate(
            point,
            self.rotation_origin.unwrap_or_else(|| bounds.center()),
            -self.rotation_degrees,
        );
        let hit_radius = self.stroke_width / 2.0 + tolerance;
        let near = |a, b| segment_distance(point, a, b) <= hit_radius;
        match &self.shape {
            Shape::Freehand(points) => {
                points.windows(2).any(|p| near(p[0], p[1]))
                    || (points.len() == 1 && near(points[0], points[0]))
            }
            Shape::SmoothPath(points) => {
                let samples = path_samples(points, false);
                samples.windows(2).any(|points| near(points[0], points[1]))
                    || (samples.len() == 1 && near(samples[0], samples[0]))
            }
            Shape::ControlledPath(points) => path_samples(points, true)
                .windows(2)
                .any(|points| near(points[0], points[1])),
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
            Shape::TaperedArrow(points) => {
                let outline_hit =
                    tapered_arrow_outline(points, self.stroke_width + tolerance * 2.0)
                        .is_some_and(|outline| path_contains(&outline, point));
                outline_hit || (self.fill.is_some() && polygon_contains(points, point))
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
            Shape::RoundedRectangle { radius, .. } => {
                let outer = Bounds {
                    x: bounds.x - hit_radius,
                    y: bounds.y - hit_radius,
                    width: bounds.width + hit_radius * 2.0,
                    height: bounds.height + hit_radius * 2.0,
                };
                let inside_outer = rounded_rectangle_contains(point, outer, *radius + hit_radius);
                let inner_width = bounds.width - hit_radius * 2.0;
                let inner_height = bounds.height - hit_radius * 2.0;
                let inside_inner = inner_width > 0.0
                    && inner_height > 0.0
                    && rounded_rectangle_contains(
                        point,
                        Bounds {
                            x: bounds.x + hit_radius,
                            y: bounds.y + hit_radius,
                            width: inner_width,
                            height: inner_height,
                        },
                        (*radius - hit_radius).max(0.0),
                    );
                inside_outer && (self.fill.is_some() || !inside_inner)
            }
            Shape::Ellipse { .. } => {
                let center = bounds.center();
                let (x, y) = (point.x - center.x, point.y - center.y);
                let (rx, ry) = (bounds.width / 2.0, bounds.height / 2.0);
                let outer =
                    (x / (rx + hit_radius)).powi(2) + (y / (ry + hit_radius)).powi(2) <= 1.0;
                outer
                    && (self.fill.is_some()
                        || rx <= hit_radius
                        || ry <= hit_radius
                        || (x / (rx - hit_radius)).powi(2) + (y / (ry - hit_radius)).powi(2) >= 1.0)
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

fn rounded_rectangle_contains(point: Point, bounds: Bounds, radius: f32) -> bool {
    if point.x < bounds.x
        || point.x > bounds.x + bounds.width
        || point.y < bounds.y
        || point.y > bounds.y + bounds.height
    {
        return false;
    }
    let radius = radius.min(bounds.width / 2.0).min(bounds.height / 2.0);
    if radius == 0.0 {
        return true;
    }
    let center_x = point
        .x
        .clamp(bounds.x + radius, bounds.x + bounds.width - radius);
    let center_y = point
        .y
        .clamp(bounds.y + radius, bounds.y + bounds.height - radius);
    (point.x - center_x).hypot(point.y - center_y) <= radius
}

fn quadratic_point(from: Point, control: Point, to: Point, t: f32) -> Point {
    let inverse = 1.0 - t;
    Point {
        x: inverse * inverse * from.x + 2.0 * inverse * t * control.x + t * t * to.x,
        y: inverse * inverse * from.y + 2.0 * inverse * t * control.y + t * t * to.y,
    }
}

fn path_samples(points: &[Point], controlled_end: bool) -> Vec<Point> {
    const STEPS: usize = 24;
    if points.len() < 2 {
        return points.to_vec();
    }
    if points.len() == 2 {
        return points.to_vec();
    }
    let mut samples = vec![points[0]];
    let last_control = if controlled_end {
        points.len() - 2
    } else {
        points.len() - 1
    };
    for index in 1..last_control {
        let from = *samples.last().expect("path starts with one sample");
        let to = Point {
            x: (points[index].x + points[index + 1].x) / 2.0,
            y: (points[index].y + points[index + 1].y) / 2.0,
        };
        samples.extend(
            (1..=STEPS)
                .map(|step| quadratic_point(from, points[index], to, step as f32 / STEPS as f32)),
        );
    }
    if controlled_end {
        let from = *samples.last().expect("path starts with one sample");
        let control = points[points.len() - 2];
        let to = points[points.len() - 1];
        samples.extend(
            (1..=STEPS).map(|step| quadratic_point(from, control, to, step as f32 / STEPS as f32)),
        );
    } else {
        samples.push(points[points.len() - 1]);
    }
    samples
}

fn paint(color: [u8; 4], blend_mode: BlendMode) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = true;
    paint.blend_mode = blend_mode.raster();
    paint
}

fn push_rounded_rectangle(path: &mut PathBuilder, rect: tiny_skia::Rect, radius: f32) {
    // Cubic quarter-circle approximation; error stays below 0.003 px at the
    // shipping editor's maximum 12 px corner radius.
    const KAPPA: f32 = 0.552_284_8;
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let offset = radius * KAPPA;
    let (left, top, right, bottom) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    path.move_to(left + radius, top);
    path.line_to(right - radius, top);
    path.cubic_to(
        right - radius + offset,
        top,
        right,
        top + radius - offset,
        right,
        top + radius,
    );
    path.line_to(right, bottom - radius);
    path.cubic_to(
        right,
        bottom - radius + offset,
        right - radius + offset,
        bottom,
        right - radius,
        bottom,
    );
    path.line_to(left + radius, bottom);
    path.cubic_to(
        left + radius - offset,
        bottom,
        left,
        bottom - radius + offset,
        left,
        bottom - radius,
    );
    path.line_to(left, top + radius);
    path.cubic_to(
        left,
        top + radius - offset,
        left + radius - offset,
        top,
        left + radius,
        top,
    );
    path.close();
}

fn draw_layer(canvas: &mut Pixmap, layer: &Layer) -> Result<(), String> {
    let Some(bounds) = layer.geometry_bounds()? else {
        return Ok(());
    };
    let center = layer.rotation_origin.unwrap_or_else(|| bounds.center());
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
                &tiny_skia::PixmapPaint {
                    blend_mode: layer.blend_mode.raster(),
                    ..Default::default()
                },
                Transform::from_translate(glyph.x, glyph.y).post_concat(transform),
                None,
            );
        }
        return Ok(());
    }

    let mut path = PathBuilder::new();
    let mut closed = false;
    match &layer.shape {
        Shape::Freehand(points) | Shape::Polygon(points) | Shape::TaperedArrow(points) => {
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
            if matches!(layer.shape, Shape::Polygon(_) | Shape::TaperedArrow(_)) {
                path.close();
                closed = true;
            }
        }
        Shape::SmoothPath(points) | Shape::ControlledPath(points) => {
            if points.is_empty() {
                return Ok(());
            }
            path.move_to(points[0].x, points[0].y);
            if points.len() == 1 {
                path.line_to(points[0].x + 0.01, points[0].y + 0.01);
            } else if points.len() == 2 {
                path.line_to(points[1].x, points[1].y);
            } else if matches!(layer.shape, Shape::ControlledPath(_)) && points.len() == 3 {
                path.quad_to(points[1].x, points[1].y, points[2].x, points[2].y);
            } else {
                let last_control = if matches!(layer.shape, Shape::ControlledPath(_)) {
                    points.len() - 2
                } else {
                    points.len() - 1
                };
                for index in 1..last_control {
                    path.quad_to(
                        points[index].x,
                        points[index].y,
                        (points[index].x + points[index + 1].x) / 2.0,
                        (points[index].y + points[index + 1].y) / 2.0,
                    );
                }
                if matches!(layer.shape, Shape::ControlledPath(_)) {
                    let control = points[points.len() - 2];
                    let end = points[points.len() - 1];
                    path.quad_to(control.x, control.y, end.x, end.y);
                } else {
                    let end = points[points.len() - 1];
                    path.line_to(end.x, end.y);
                }
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
        | Shape::RoundedRectangle {
            origin,
            width,
            height,
            ..
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
            } else if let Shape::RoundedRectangle { radius, .. } = &layer.shape {
                push_rounded_rectangle(&mut path, rect, *radius);
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
        let stroke = if matches!(layer.shape, Shape::TaperedArrow(_)) {
            tapered_arrow_stroke(layer.stroke_width)
        } else {
            Stroke {
                width: layer.stroke_width,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            }
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
                rotation_origin: None,
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
