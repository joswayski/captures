use crate::geometry::{Point, Rect};
use image::{GenericImageView, Rgba, RgbaImage, imageops::FilterType};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const MAX_CANVAS_DIMENSION: u32 = 16_384;
const MAX_CANVAS_PIXELS: u64 = 100_000_000;
static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tool {
    Select,
    Crop,
    Text,
    Pen,
    Arrow,
    Line,
    Rectangle,
    Ellipse,
    Triangle,
    Diamond,
    Star,
    Eraser,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoveBackgroundMode {
    Wand,
    Erase,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageTarget {
    Source,
    Layer(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Stroke(Vec<Point>),
    Arrow(Point, Point),
    Line(Point, Point),
    Rectangle(Rect),
    Ellipse(Rect),
    Polygon(Vec<Point>),
    Image {
        origin: Point,
        width: f32,
        height: f32,
        pixels: Arc<RgbaImage>,
    },
    Text {
        origin: Point,
        value: String,
        font_size: f32,
        font_data: Arc<[u8]>,
    },
}

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

pub struct FreehandGesture {
    points: Vec<Point>,
}

impl FreehandGesture {
    pub fn begin(point: Point) -> Self {
        Self {
            points: vec![point],
        }
    }

    pub fn sample(&mut self, point: Point) {
        let distinct = self
            .points
            .last()
            .is_none_or(|last| (last.x - point.x).powi(2) + (last.y - point.y).powi(2) >= 0.25);
        if distinct {
            self.points.push(point);
        }
    }

    pub fn finish(mut self, point: Point) -> Vec<Point> {
        self.sample(point);
        self.points
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub shape: Shape,
    pub original_pixels: Option<Arc<RgbaImage>>,
    pub color: [u8; 4],
    pub stroke: f32,
    pub fill: Option<[u8; 4]>,
    pub opacity: u8,
    pub blend_mode: BlendMode,
    pub rotation_degrees: f32,
    pub visible: bool,
    pub locked: bool,
}

impl Layer {
    pub fn supports_fill(&self) -> bool {
        matches!(
            self.shape,
            Shape::Rectangle(_) | Shape::Ellipse(_) | Shape::Polygon(_)
        )
    }

    /// Unrotated geometry bounds in source pixels. This is the transform source
    /// of truth; using the post-rotation AABB would skew rotated annotations.
    pub fn geometry_bounds(&self) -> Option<Rect> {
        let mut raster = to_raster_layer(self);
        raster.rotation_degrees = 0.0;
        raster.stroke_width = 0.0;
        raster.bounds().map(|bounds| Rect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        })
    }

    pub fn selection_corners(&self) -> Option<[Point; 4]> {
        let bounds = self.geometry_bounds()?;
        let center = Point {
            x: bounds.x + bounds.width / 2.0,
            y: bounds.y + bounds.height / 2.0,
        };
        Some(
            [
                Point {
                    x: bounds.x,
                    y: bounds.y,
                },
                Point {
                    x: bounds.x + bounds.width,
                    y: bounds.y,
                },
                Point {
                    x: bounds.x + bounds.width,
                    y: bounds.y + bounds.height,
                },
                Point {
                    x: bounds.x,
                    y: bounds.y + bounds.height,
                },
            ]
            .map(|point| rotate(point, center, self.rotation_degrees)),
        )
    }

    /// Resize handle identifiers and positions. Lines expose only their real
    /// endpoints; duplicate corners on an axis-degenerate bounds rectangle are
    /// not meaningful controls.
    pub fn resize_handles(&self) -> Vec<(usize, Point)> {
        if let Shape::Line(start, end) = &self.shape {
            if (end.x - start.x).hypot(end.y - start.y) < 1.0 {
                return Vec::new();
            }
            let center = Point {
                x: (start.x + end.x) / 2.0,
                y: (start.y + end.y) / 2.0,
            };
            return vec![
                (4, rotate(*start, center, self.rotation_degrees)),
                (5, rotate(*end, center, self.rotation_degrees)),
            ];
        }
        let Some(bounds) = self.geometry_bounds() else {
            return Vec::new();
        };
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return Vec::new();
        }
        self.selection_corners()
            .into_iter()
            .flatten()
            .enumerate()
            .collect()
    }

    pub fn translated(&self, delta: Point) -> Self {
        let mut layer = self.clone();
        transform_shape(&mut layer.shape, |point| Point {
            x: point.x + delta.x,
            y: point.y + delta.y,
        });
        layer
    }
}

fn rotate(point: Point, center: Point, degrees: f32) -> Point {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Point {
        x: center.x + (point.x - center.x) * cos - (point.y - center.y) * sin,
        y: center.y + (point.x - center.x) * sin + (point.y - center.y) * cos,
    }
}

fn transform_shape(shape: &mut Shape, transform: impl Fn(Point) -> Point) {
    match shape {
        Shape::Stroke(points) | Shape::Polygon(points) => {
            points
                .iter_mut()
                .for_each(|point| *point = transform(*point));
        }
        Shape::Arrow(start, end) | Shape::Line(start, end) => {
            *start = transform(*start);
            *end = transform(*end);
        }
        Shape::Rectangle(rect) | Shape::Ellipse(rect) => {
            let top_left = transform(Point {
                x: rect.x,
                y: rect.y,
            });
            let bottom_right = transform(Point {
                x: rect.x + rect.width,
                y: rect.y + rect.height,
            });
            *rect = Rect::from_points(top_left, bottom_right);
        }
        Shape::Image {
            origin,
            width,
            height,
            ..
        } => {
            let top_left = transform(*origin);
            let bottom_right = transform(Point {
                x: origin.x + *width,
                y: origin.y + *height,
            });
            let rect = Rect::from_points(top_left, bottom_right);
            *origin = Point {
                x: rect.x,
                y: rect.y,
            };
            *width = rect.width;
            *height = rect.height;
        }
        Shape::Text { origin, .. } => *origin = transform(*origin),
    }
}

/// Resize a rotated layer by dragging one visual corner while keeping its
/// opposite visual corner fixed. The returned geometry stays unrotated and
/// retains the layer rotation, matching the raster backend's coordinate model.
pub fn resize_from_corner(layer: &Layer, corner: usize, pointer: Point) -> Option<Layer> {
    if let Shape::Line(start, end) = &layer.shape {
        if !matches!(corner, 4 | 5) {
            return None;
        }
        let old_center = Point {
            x: (start.x + end.x) / 2.0,
            y: (start.y + end.y) / 2.0,
        };
        let start = rotate(*start, old_center, layer.rotation_degrees);
        let end = rotate(*end, old_center, layer.rotation_degrees);
        let fixed = if corner == 4 { end } else { start };
        if (pointer.x - fixed.x).hypot(pointer.y - fixed.y) < 1.0 {
            return None;
        }
        let center = Point {
            x: (fixed.x + pointer.x) / 2.0,
            y: (fixed.y + pointer.y) / 2.0,
        };
        let dragged = rotate(pointer, center, -layer.rotation_degrees);
        let fixed = rotate(fixed, center, -layer.rotation_degrees);
        let mut resized = layer.clone();
        resized.shape = if corner == 4 {
            Shape::Line(dragged, fixed)
        } else {
            Shape::Line(fixed, dragged)
        };
        return Some(resized);
    }

    let source = layer.geometry_bounds()?;
    if source.width <= 0.0 || source.height <= 0.0 || corner >= 4 {
        return None;
    }
    let corners = layer.selection_corners()?;
    let fixed = corners[(corner + 2) % 4];
    let center = Point {
        x: (fixed.x + pointer.x) / 2.0,
        y: (fixed.y + pointer.y) / 2.0,
    };
    let destination_fixed = rotate(fixed, center, -layer.rotation_degrees);
    let destination_dragged = rotate(pointer, center, -layer.rotation_degrees);
    let source_corners = [
        Point {
            x: source.x,
            y: source.y,
        },
        Point {
            x: source.x + source.width,
            y: source.y,
        },
        Point {
            x: source.x + source.width,
            y: source.y + source.height,
        },
        Point {
            x: source.x,
            y: source.y + source.height,
        },
    ];
    let source_fixed = source_corners[(corner + 2) % 4];
    let source_dragged = source_corners[corner];
    let source_dx = source_dragged.x - source_fixed.x;
    let source_dy = source_dragged.y - source_fixed.y;
    let destination_dx = destination_dragged.x - destination_fixed.x;
    let destination_dy = destination_dragged.y - destination_fixed.y;
    if destination_dx.abs() < 1.0 || destination_dy.abs() < 1.0 {
        return None;
    }
    let mut resized = layer.clone();
    transform_shape(&mut resized.shape, |point| Point {
        x: destination_fixed.x + (point.x - source_fixed.x) / source_dx * destination_dx,
        y: destination_fixed.y + (point.y - source_fixed.y) / source_dy * destination_dy,
    });
    if let Shape::Text { font_size, .. } = &mut resized.shape {
        let scale_x = (destination_dx / source_dx).abs();
        let scale_y = (destination_dy / source_dy).abs();
        *font_size *= scale_x.min(scale_y);
    }
    Some(resized)
}

#[derive(Clone)]
pub struct Document {
    id: u64,
    original: Arc<RgbaImage>,
    source: Arc<RgbaImage>,
    pub source_visible: bool,
    pub crop: Rect,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub background: Option<[u8; 4]>,
    pub layers: Vec<Layer>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    next_id: u64,
    revision: u64,
}

#[derive(Clone)]
struct Snapshot {
    crop: Rect,
    source: Arc<RgbaImage>,
    source_visible: bool,
    canvas_width: u32,
    canvas_height: u32,
    background: Option<[u8; 4]>,
    layers: Vec<Layer>,
}

impl Document {
    pub fn new(image: RgbaImage) -> Self {
        let crop = Rect {
            x: 0.0,
            y: 0.0,
            width: image.width() as f32,
            height: image.height() as f32,
        };
        let original = Arc::new(image);
        Self {
            id: NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed),
            original: original.clone(),
            source: original,
            source_visible: true,
            crop,
            canvas_width: crop.width.round() as u32,
            canvas_height: crop.height.round() as u32,
            background: None,
            layers: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            revision: 1,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn render_key(&self) -> (u64, u64) {
        (self.id, self.revision)
    }

    pub fn add(&mut self, shape: Shape, color: [u8; 4], stroke: f32) -> u64 {
        self.checkpoint();
        self.push_layer(shape, color, stroke)
    }

    fn push_layer(&mut self, shape: Shape, color: [u8; 4], stroke: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.layers.push(Layer {
            id,
            name: default_layer_name(&shape).into(),
            shape,
            original_pixels: None,
            color,
            stroke: stroke.clamp(1.0, 48.0),
            fill: None,
            opacity: 255,
            blend_mode: BlendMode::Normal,
            rotation_degrees: 0.0,
            visible: true,
            locked: false,
        });
        id
    }

    pub fn add_image(&mut self, image: RgbaImage, offset: usize, name: String) -> u64 {
        self.add_images(vec![(image, name)], offset)
            .into_iter()
            .next()
            .expect("one image produces one layer")
    }

    pub fn add_images(
        &mut self,
        images: Vec<(RgbaImage, String)>,
        initial_offset: usize,
    ) -> Vec<u64> {
        if images.is_empty() {
            return Vec::new();
        }
        self.checkpoint();
        images
            .into_iter()
            .enumerate()
            .map(|(offset, (image, name))| self.push_image(image, initial_offset + offset, name))
            .collect()
    }

    fn push_image(&mut self, image: RgbaImage, offset: usize, name: String) -> u64 {
        let source_width = image.width().max(1) as f32;
        let source_height = image.height().max(1) as f32;
        let scale = (self.crop.width * 0.55 / source_width)
            .min(self.crop.height * 0.55 / source_height)
            .min(1.0);
        let width = source_width * scale;
        let height = source_height * scale;
        let cascade = offset as f32 * 18.0;
        let origin = Point {
            x: self.crop.x + (self.crop.width - width) / 2.0 + cascade,
            y: self.crop.y + (self.crop.height - height) / 2.0 + cascade,
        };
        let id = self.push_layer(
            Shape::Image {
                origin,
                width,
                height,
                pixels: Arc::new(image),
            },
            [255, 255, 255, 255],
            1.0,
        );
        self.layers.last_mut().expect("layer was just added").name = name;
        id
    }

    pub fn delete(&mut self, id: u64) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        if self.layers[index].locked {
            return false;
        }
        self.checkpoint();
        self.layers.remove(index);
        true
    }

    pub fn toggle_visibility(&mut self, id: u64) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        self.checkpoint();
        self.layers[index].visible = !self.layers[index].visible;
        true
    }

    pub fn toggle_locked(&mut self, id: u64) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        self.checkpoint();
        self.layers[index].locked = !self.layers[index].locked;
        true
    }

    pub fn set_layer_opacity(&mut self, id: u64, opacity: u8) -> bool {
        self.edit_layer(id, |layer| layer.opacity = opacity)
    }

    pub fn set_layer_blend_mode(&mut self, id: u64, blend_mode: BlendMode) -> bool {
        self.edit_layer(id, |layer| layer.blend_mode = blend_mode)
    }

    pub fn rename_layer(&mut self, id: u64, name: String) -> bool {
        let name = name.trim().chars().take(80).collect::<String>();
        if name.is_empty() {
            return false;
        }
        self.edit_layer(id, move |layer| layer.name = name)
    }

    pub fn duplicate(&mut self, id: u64) -> Option<u64> {
        let source = self.layers.iter().find(|layer| layer.id == id)?.clone();
        self.checkpoint();
        let new_id = self.next_id;
        self.next_id += 1;
        let mut duplicate = source.translated(Point { x: 16.0, y: 16.0 });
        duplicate.id = new_id;
        duplicate.name = format!("{} copy", duplicate.name);
        self.layers.push(duplicate);
        Some(new_id)
    }

    pub fn move_layer(&mut self, id: u64, delta: isize) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        let destination = index
            .saturating_add_signed(delta)
            .min(self.layers.len() - 1);
        if index == destination || self.layers[index].locked {
            return false;
        }
        self.checkpoint();
        let layer = self.layers.remove(index);
        self.layers.insert(destination, layer);
        true
    }

    pub fn set_layer_color(&mut self, id: u64, color: [u8; 4]) -> bool {
        self.edit_layer(id, |layer| layer.color = color)
    }

    pub fn set_layer_stroke(&mut self, id: u64, stroke: f32) -> bool {
        self.edit_layer(id, |layer| layer.stroke = stroke.clamp(1.0, 48.0))
    }

    pub fn set_layer_fill(&mut self, id: u64, fill: Option<[u8; 4]>) -> bool {
        self.edit_layer(id, |layer| layer.fill = fill)
    }

    pub fn set_layer_rotation(&mut self, id: u64, rotation_degrees: f32) -> bool {
        self.edit_layer(id, |layer| {
            layer.rotation_degrees = ((rotation_degrees + 180.0).rem_euclid(360.0)) - 180.0;
        })
    }

    fn edit_layer(&mut self, id: u64, edit: impl FnOnce(&mut Layer)) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        if self.layers[index].locked {
            return false;
        }
        let before = self.layers[index].clone();
        edit(&mut self.layers[index]);
        if self.layers[index] == before {
            return false;
        }
        self.push_undo_with_layer(index, before);
        true
    }

    pub fn preview_layer(&mut self, layer: Layer) -> bool {
        let Some(current) = self
            .layers
            .iter_mut()
            .find(|current| current.id == layer.id)
        else {
            return false;
        };
        *current = layer;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Commits a sequence of preview replacements as one undoable edit.
    pub fn commit_layer_preview(&mut self, original: Layer) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == original.id) else {
            return false;
        };
        if self.layers[index] == original {
            return false;
        }
        self.push_undo_with_layer(index, original);
        true
    }

    fn push_undo_with_layer(&mut self, index: usize, layer: Layer) {
        let mut snapshot = self.snapshot();
        snapshot.layers[index] = layer;
        self.undo.push(snapshot);
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn set_crop(&mut self, crop: Rect) -> bool {
        let canvas_right = self.crop.x + self.canvas_width as f32;
        let canvas_bottom = self.crop.y + self.canvas_height as f32;
        let right = (crop.x + crop.width).min(canvas_right);
        let bottom = (crop.y + crop.height).min(canvas_bottom);
        let crop = Rect {
            x: crop.x.clamp(self.crop.x, canvas_right - 1.0),
            y: crop.y.clamp(self.crop.y, canvas_bottom - 1.0),
            width: (right - crop.x.max(self.crop.x)).max(1.0),
            height: (bottom - crop.y.max(self.crop.y)).max(1.0),
        };
        if crop == self.crop {
            return false;
        }
        self.checkpoint();
        self.crop = crop;
        self.canvas_width = crop.width.round().max(1.0) as u32;
        self.canvas_height = crop.height.round().max(1.0) as u32;
        true
    }

    pub fn set_canvas_size(&mut self, width: u32, height: u32) -> Result<bool, &'static str> {
        let width = width.clamp(1, MAX_CANVAS_DIMENSION);
        let height = height.clamp(1, MAX_CANVAS_DIMENSION);
        if u64::from(width) * u64::from(height) > MAX_CANVAS_PIXELS {
            return Err("Canvas size is limited to 100 million pixels");
        }
        if (width, height) == (self.canvas_width, self.canvas_height) {
            return Ok(false);
        }
        self.checkpoint();
        self.canvas_width = width;
        self.canvas_height = height;
        Ok(true)
    }

    pub fn set_background(&mut self, background: Option<[u8; 4]>) -> bool {
        if self.background == background {
            return false;
        }
        self.checkpoint();
        self.background = background;
        true
    }

    pub fn toggle_source_visibility(&mut self) -> bool {
        self.checkpoint();
        self.source_visible = !self.source_visible;
        self.source_visible
    }

    pub fn can_trim_to_visible_content(&self) -> bool {
        match self.visible_content_frame() {
            Ok(Some(frame)) => {
                self.crop.x != frame.x
                    || self.crop.y != frame.y
                    || self.canvas_width != frame.width.round() as u32
                    || self.canvas_height != frame.height.round() as u32
            }
            Err(_) => true,
            Ok(None) => false,
        }
    }

    pub fn trim_to_visible_content(&mut self) -> Result<bool, &'static str> {
        let Some(frame) = self.visible_content_frame()? else {
            return Ok(false);
        };
        let width = frame.width.round() as u32;
        let height = frame.height.round() as u32;
        if self.crop.x == frame.x
            && self.crop.y == frame.y
            && self.canvas_width == width
            && self.canvas_height == height
        {
            return Ok(false);
        }
        self.checkpoint();
        self.crop = frame;
        self.canvas_width = width;
        self.canvas_height = height;
        Ok(true)
    }

    fn visible_content_frame(&self) -> Result<Option<Rect>, &'static str> {
        let mut bounds = self.source_visible.then(|| Rect {
            x: 0.0,
            y: 0.0,
            width: self.source.width() as f32,
            height: self.source.height() as f32,
        });
        for layer in self.layers.iter().filter(|layer| layer.visible) {
            if !layer_geometry_is_finite(layer) {
                return Err("Trim bounds must be finite");
            }
            let Some(layer_bounds) = to_raster_layer(layer).bounds().map(|value| Rect {
                x: value.x,
                y: value.y,
                width: value.width,
                height: value.height,
            }) else {
                continue;
            };
            if !rect_is_finite(layer_bounds) {
                return Err("Trim bounds must be finite");
            }
            bounds = Some(match bounds {
                Some(current) => union_rect(current, layer_bounds),
                None => layer_bounds,
            });
        }
        let Some(bounds) = bounds else {
            return Ok(None);
        };
        if !rect_is_finite(bounds) {
            return Err("Trim bounds must be finite");
        }
        let frame = {
            let x = bounds.x.floor();
            let y = bounds.y.floor();
            let right = (bounds.x + bounds.width).ceil();
            let bottom = (bounds.y + bounds.height).ceil();
            Rect {
                x,
                y,
                width: (right - x).max(1.0),
                height: (bottom - y).max(1.0),
            }
        };
        if !rect_is_finite(frame) {
            return Err("Trim bounds must be finite");
        }
        if frame.width > MAX_CANVAS_DIMENSION as f32 || frame.height > MAX_CANVAS_DIMENSION as f32 {
            return Err("Trim canvas dimensions are limited to 16384 pixels");
        }
        let width = frame.width.round() as u64;
        let height = frame.height.round() as u64;
        if width * height > MAX_CANVAS_PIXELS {
            return Err("Trim canvas size is limited to 100 million pixels");
        }
        Ok(Some(frame))
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(previous);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            crop: self.crop,
            source: self.source.clone(),
            source_visible: self.source_visible,
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
            background: self.background,
            layers: self.layers.clone(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.crop = snapshot.crop;
        self.source = snapshot.source;
        self.source_visible = snapshot.source_visible;
        self.canvas_width = snapshot.canvas_width;
        self.canvas_height = snapshot.canvas_height;
        self.background = snapshot.background;
        self.layers = snapshot.layers;
    }

    pub fn render(&self) -> Result<RgbaImage, String> {
        let mut source = RgbaImage::from_pixel(
            self.canvas_width,
            self.canvas_height,
            Rgba(self.background.unwrap_or([0, 0, 0, 0])),
        );
        let crop_x = self.crop.x.max(0.0).round() as u32;
        let crop_y = self.crop.y.max(0.0).round() as u32;
        let crop_width =
            self.crop
                .width
                .max(1.0)
                .round()
                .min(self.original.width().saturating_sub(crop_x) as f32) as u32;
        let crop_height =
            self.crop
                .height
                .max(1.0)
                .round()
                .min(self.original.height().saturating_sub(crop_y) as f32) as u32;
        if self.source_visible && crop_width > 0 && crop_height > 0 {
            let original = self
                .source
                .view(crop_x, crop_y, crop_width, crop_height)
                .to_image();
            let destination_x = (crop_x as f32 - self.crop.x).round().max(0.0) as i64;
            let destination_y = (crop_y as f32 - self.crop.y).round().max(0.0) as i64;
            image::imageops::overlay(&mut source, &original, destination_x, destination_y);
        }
        let offset = captures_image::Point {
            x: -self.crop.x,
            y: -self.crop.y,
        };
        captures_image::render(&captures_image::Document {
            source: Arc::new(source),
            crop: None,
            layers: self
                .layers
                .iter()
                .filter(|layer| layer.visible)
                .map(|layer| {
                    let mut layer = to_raster_layer(layer);
                    translate_raster_shape(&mut layer.shape, offset);
                    layer
                })
                .collect(),
        })
    }

    pub fn render_resized(&self, width: u32, height: u32) -> Result<RgbaImage, String> {
        let width = width.clamp(1, 16_384);
        let height = height.clamp(1, 16_384);
        if u64::from(width) * u64::from(height) > MAX_CANVAS_PIXELS {
            return Err("Output size is limited to 100 million pixels".into());
        }
        let rendered = self.render()?;
        if rendered.dimensions() == (width, height) {
            Ok(rendered)
        } else {
            Ok(image::imageops::resize(
                &rendered,
                width,
                height,
                FilterType::Lanczos3,
            ))
        }
    }

    pub fn hit_test_image(&self, point: Point) -> Option<ImageTarget> {
        self.layers
            .iter()
            .rev()
            .filter(|layer| layer.visible)
            .find_map(|layer| image_pixel(layer, point).map(|_| ImageTarget::Layer(layer.id)))
            .or_else(|| {
                self.source_visible
                    .then(|| source_pixel(&self.source, point))
                    .flatten()
                    .map(|_| ImageTarget::Source)
            })
    }

    pub fn remove_background_wand(
        &mut self,
        target: ImageTarget,
        point: Point,
        tolerance: u8,
        contiguous: bool,
    ) -> Result<bool, String> {
        let (pixels, sample) = self.target_pixels_and_point(target, point)?;
        let mut edited = pixels.as_ref().clone();
        if remove_color_to_transparent(&mut edited, sample.0, sample.1, tolerance, contiguous) == 0
        {
            return Ok(false);
        }
        self.replace_target_pixels(target, Arc::new(edited));
        Ok(true)
    }

    pub fn remove_background_stroke(
        &mut self,
        target: ImageTarget,
        points: &[Point],
        brush_size: f32,
        softness: f32,
        restore: bool,
    ) -> Result<bool, String> {
        let first = points.first().ok_or("Background brush stroke is empty")?;
        let (pixels, first_pixel) = self.target_pixels_and_point(target, *first)?;
        let original = if restore {
            Some(
                self.target_original_pixels(target)
                    .ok_or("Nothing to restore yet — remove some background first")?,
            )
        } else {
            None
        };
        let display_width = match target {
            ImageTarget::Source => pixels.width() as f32,
            ImageTarget::Layer(id) => self
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .and_then(|layer| match layer.shape {
                    Shape::Image { width, .. } => Some(width),
                    _ => None,
                })
                .ok_or("Image layer is unavailable")?,
        };
        let radius = (brush_size * pixels.width() as f32 / display_width.max(1.0) * 0.5).max(1.0);
        let hardness = 1.0 - (softness / 100.0).clamp(0.0, 1.0);
        let mut edited = pixels.as_ref().clone();
        let mut previous = first_pixel;
        let mut changed = 0;
        for point in points {
            let Ok((_, next)) = self.target_pixels_and_point(target, *point) else {
                continue;
            };
            changed += stroke_background_brush(
                &mut edited,
                previous,
                next,
                radius,
                restore,
                original.as_deref(),
                hardness,
            );
            previous = next;
        }
        if changed == 0 {
            return Ok(false);
        }
        self.replace_target_pixels(target, Arc::new(edited));
        Ok(true)
    }

    fn target_pixels_and_point(
        &self,
        target: ImageTarget,
        point: Point,
    ) -> Result<(Arc<RgbaImage>, (u32, u32)), String> {
        match target {
            ImageTarget::Source => source_pixel(&self.source, point)
                .map(|pixel| (self.source.clone(), pixel))
                .ok_or_else(|| "Point is outside the source image".into()),
            ImageTarget::Layer(id) => self
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .and_then(|layer| match &layer.shape {
                    Shape::Image { pixels, .. } => {
                        image_pixel(layer, point).map(|pixel| (pixels.clone(), pixel))
                    }
                    _ => None,
                })
                .ok_or_else(|| "Point is outside the image layer".into()),
        }
    }

    fn target_original_pixels(&self, target: ImageTarget) -> Option<Arc<RgbaImage>> {
        match target {
            ImageTarget::Source => Some(self.original.clone()),
            ImageTarget::Layer(id) => self
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .and_then(|layer| layer.original_pixels.clone()),
        }
    }

    fn replace_target_pixels(&mut self, target: ImageTarget, pixels: Arc<RgbaImage>) {
        self.checkpoint();
        self.background = None;
        match target {
            ImageTarget::Source => self.source = pixels,
            ImageTarget::Layer(id) => {
                if let Some(layer) = self.layers.iter_mut().find(|layer| layer.id == id)
                    && let Shape::Image {
                        pixels: current, ..
                    } = &mut layer.shape
                {
                    if layer.original_pixels.is_none() {
                        layer.original_pixels = Some(current.clone());
                    }
                    *current = pixels;
                }
            }
        }
    }

    pub fn hit_test(&self, point: Point, tolerance: f32) -> Option<u64> {
        self.layers
            .iter()
            .rev()
            .filter(|layer| layer.visible && !layer.locked)
            .find_map(|layer| {
                let raster = to_raster_layer(layer);
                raster
                    .hit_test(to_raster_point(point), tolerance)
                    .then_some(layer.id)
            })
    }
}

fn source_pixel(image: &RgbaImage, point: Point) -> Option<(u32, u32)> {
    (point.x >= 0.0
        && point.y >= 0.0
        && point.x < image.width() as f32
        && point.y < image.height() as f32)
        .then(|| (point.x.floor() as u32, point.y.floor() as u32))
}

fn union_rect(left: Rect, right: Rect) -> Rect {
    let x = left.x.min(right.x);
    let y = left.y.min(right.y);
    let far_x = (left.x + left.width).max(right.x + right.width);
    let far_y = (left.y + left.height).max(right.y + right.height);
    Rect {
        x,
        y,
        width: far_x - x,
        height: far_y - y,
    }
}

fn rect_is_finite(rect: Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width >= 0.0
        && rect.height >= 0.0
}

fn layer_geometry_is_finite(layer: &Layer) -> bool {
    let point_is_finite = |point: &Point| point.x.is_finite() && point.y.is_finite();
    let shape_is_finite = match &layer.shape {
        Shape::Stroke(points) | Shape::Polygon(points) => points.iter().all(point_is_finite),
        Shape::Arrow(start, end) | Shape::Line(start, end) => {
            point_is_finite(start) && point_is_finite(end)
        }
        Shape::Rectangle(rect) | Shape::Ellipse(rect) => rect_is_finite(*rect),
        Shape::Image {
            origin,
            width,
            height,
            ..
        } => point_is_finite(origin) && width.is_finite() && height.is_finite(),
        Shape::Text {
            origin, font_size, ..
        } => point_is_finite(origin) && font_size.is_finite(),
    };
    shape_is_finite && layer.stroke.is_finite() && layer.rotation_degrees.is_finite()
}

fn image_pixel(layer: &Layer, point: Point) -> Option<(u32, u32)> {
    let Shape::Image {
        origin,
        width,
        height,
        pixels,
    } = &layer.shape
    else {
        return None;
    };
    if *width <= 0.0 || *height <= 0.0 || pixels.width() == 0 || pixels.height() == 0 {
        return None;
    }
    let center = Point {
        x: origin.x + width / 2.0,
        y: origin.y + height / 2.0,
    };
    let local = rotate(point, center, -layer.rotation_degrees);
    let ratio_x = (local.x - origin.x) / width;
    let ratio_y = (local.y - origin.y) / height;
    if !(0.0..1.0).contains(&ratio_x) || !(0.0..1.0).contains(&ratio_y) {
        return None;
    }
    Some((
        (ratio_x * pixels.width() as f32).floor() as u32,
        (ratio_y * pixels.height() as f32).floor() as u32,
    ))
}

fn remove_color_to_transparent(
    image: &mut RgbaImage,
    start_x: u32,
    start_y: u32,
    tolerance: u8,
    contiguous: bool,
) -> usize {
    let target = *image.get_pixel(start_x, start_y);
    if target[3] == 0 {
        return 0;
    }
    let matches = |pixel: &Rgba<u8>| {
        pixel[3] > 0 && (0..3).all(|channel| pixel[channel].abs_diff(target[channel]) <= tolerance)
    };
    if !contiguous {
        let mut changed = 0;
        for pixel in image.pixels_mut() {
            if matches(pixel) {
                *pixel = Rgba([0, 0, 0, 0]);
                changed += 1;
            }
        }
        return changed;
    }
    let width = image.width();
    let height = image.height();
    let mut visited = vec![false; width as usize * height as usize];
    let mut queue = std::collections::VecDeque::from([(start_x, start_y)]);
    let mut changed = 0;
    while let Some((x, y)) = queue.pop_front() {
        let index = y as usize * width as usize + x as usize;
        if visited[index] {
            continue;
        }
        visited[index] = true;
        if !matches(image.get_pixel(x, y)) {
            continue;
        }
        image.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        changed += 1;
        if x > 0 {
            queue.push_back((x - 1, y));
        }
        if x + 1 < width {
            queue.push_back((x + 1, y));
        }
        if y > 0 {
            queue.push_back((x, y - 1));
        }
        if y + 1 < height {
            queue.push_back((x, y + 1));
        }
    }
    changed
}

fn stroke_background_brush(
    working: &mut RgbaImage,
    from: (u32, u32),
    to: (u32, u32),
    radius: f32,
    restore: bool,
    original: Option<&RgbaImage>,
    hardness: f32,
) -> usize {
    let dx = to.0 as f32 - from.0 as f32;
    let dy = to.1 as f32 - from.1 as f32;
    let distance = dx.hypot(dy);
    if distance < 0.001 {
        return stamp_background_brush(
            working,
            to.0 as f32,
            to.1 as f32,
            radius,
            restore,
            original,
            hardness,
        );
    }
    let steps = (distance / (radius * 0.35).max(0.5)).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|step| {
            let progress = step as f32 / steps as f32;
            stamp_background_brush(
                working,
                from.0 as f32 + dx * progress,
                from.1 as f32 + dy * progress,
                radius,
                restore,
                original,
                hardness,
            )
        })
        .sum()
}

fn stamp_background_brush(
    working: &mut RgbaImage,
    center_x: f32,
    center_y: f32,
    radius: f32,
    restore: bool,
    original: Option<&RgbaImage>,
    hardness: f32,
) -> usize {
    if restore && original.is_none() {
        return 0;
    }
    let radius = radius.max(0.5);
    let hard_start = radius * hardness.clamp(0.0, 1.0);
    let min_x = (center_x - radius).floor().max(0.0) as u32;
    let max_x = (center_x + radius).ceil().min(working.width() as f32 - 1.0) as u32;
    let min_y = (center_y - radius).floor().max(0.0) as u32;
    let max_y = (center_y + radius)
        .ceil()
        .min(working.height() as f32 - 1.0) as u32;
    let mut changed = 0;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let distance = (x as f32 + 0.5 - center_x).hypot(y as f32 + 0.5 - center_y);
            if distance > radius {
                continue;
            }
            let strength = if distance <= hard_start || radius == hard_start {
                1.0
            } else {
                (1.0 - (distance - hard_start) / (radius - hard_start)).clamp(0.0, 1.0)
            };
            let before = *working.get_pixel(x, y);
            let after = if restore {
                let source = *original
                    .expect("restore source checked above")
                    .get_pixel(x, y);
                Rgba(std::array::from_fn(|channel| {
                    (before[channel] as f32
                        + (source[channel] as f32 - before[channel] as f32) * strength)
                        .round() as u8
                }))
            } else {
                let alpha = (before[3] as f32 * (1.0 - strength)).round() as u8;
                if alpha == 0 {
                    Rgba([0, 0, 0, 0])
                } else {
                    Rgba([before[0], before[1], before[2], alpha])
                }
            };
            if after != before {
                working.put_pixel(x, y, after);
                changed += 1;
            }
        }
    }
    changed
}

fn translate_raster_shape(shape: &mut captures_image::Shape, offset: captures_image::Point) {
    let translate = |point: &mut captures_image::Point| {
        point.x += offset.x;
        point.y += offset.y;
    };
    match shape {
        captures_image::Shape::Freehand(points) | captures_image::Shape::Polygon(points) => {
            points.iter_mut().for_each(translate)
        }
        captures_image::Shape::Arrow { start, end }
        | captures_image::Shape::Line { start, end } => {
            translate(start);
            translate(end);
        }
        captures_image::Shape::Rectangle { origin, .. }
        | captures_image::Shape::Ellipse { origin, .. }
        | captures_image::Shape::Image { origin, .. }
        | captures_image::Shape::Text { origin, .. } => translate(origin),
    }
}

fn to_raster_layer(layer: &Layer) -> captures_image::Layer {
    let shape = match &layer.shape {
        Shape::Stroke(points) => {
            captures_image::Shape::Freehand(points.iter().copied().map(to_raster_point).collect())
        }
        Shape::Arrow(start, end) => captures_image::Shape::Arrow {
            start: to_raster_point(*start),
            end: to_raster_point(*end),
        },
        Shape::Line(start, end) => captures_image::Shape::Line {
            start: to_raster_point(*start),
            end: to_raster_point(*end),
        },
        Shape::Rectangle(rect) => captures_image::Shape::Rectangle {
            origin: to_raster_point(Point {
                x: rect.x,
                y: rect.y,
            }),
            width: rect.width,
            height: rect.height,
        },
        Shape::Ellipse(rect) => captures_image::Shape::Ellipse {
            origin: to_raster_point(Point {
                x: rect.x,
                y: rect.y,
            }),
            width: rect.width,
            height: rect.height,
        },
        Shape::Polygon(points) => {
            captures_image::Shape::Polygon(points.iter().copied().map(to_raster_point).collect())
        }
        Shape::Image {
            origin,
            width,
            height,
            pixels,
        } => captures_image::Shape::Image {
            origin: to_raster_point(*origin),
            width: *width,
            height: *height,
            pixels: pixels.clone(),
        },
        Shape::Text {
            origin,
            value,
            font_size,
            font_data,
        } => captures_image::Shape::Text {
            origin: to_raster_point(*origin),
            text: value.clone(),
            font_size: *font_size,
            font_data: font_data.clone(),
        },
    };
    let apply_opacity = |mut color: [u8; 4]| {
        color[3] = ((u16::from(color[3]) * u16::from(layer.opacity) + 127) / 255) as u8;
        color
    };
    captures_image::Layer {
        id: layer.id,
        shape,
        color: apply_opacity(layer.color),
        stroke_width: layer.stroke,
        fill: layer.fill.map(apply_opacity),
        rotation_degrees: layer.rotation_degrees,
        blend_mode: match layer.blend_mode {
            BlendMode::Normal => captures_image::BlendMode::Normal,
            BlendMode::Multiply => captures_image::BlendMode::Multiply,
            BlendMode::Screen => captures_image::BlendMode::Screen,
            BlendMode::Overlay => captures_image::BlendMode::Overlay,
            BlendMode::Darken => captures_image::BlendMode::Darken,
            BlendMode::Lighten => captures_image::BlendMode::Lighten,
        },
    }
}

fn default_layer_name(shape: &Shape) -> &'static str {
    match shape {
        Shape::Stroke(_) => "Freehand",
        Shape::Arrow(_, _) => "Arrow",
        Shape::Line(_, _) => "Line",
        Shape::Rectangle(_) => "Rectangle",
        Shape::Ellipse(_) => "Ellipse",
        Shape::Polygon(_) => "Shape",
        Shape::Image { .. } => "Image",
        Shape::Text { .. } => "Text",
    }
}

fn to_raster_point(point: Point) -> captures_image::Point {
    captures_image::Point {
        x: point.x,
        y: point.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_image_geometry(document: &mut Document, id: u64, rect: Rect) {
        let layer = document
            .layers
            .iter_mut()
            .find(|layer| layer.id == id)
            .unwrap();
        let Shape::Image {
            origin,
            width,
            height,
            ..
        } = &mut layer.shape
        else {
            panic!("expected image");
        };
        *origin = Point {
            x: rect.x,
            y: rect.y,
        };
        *width = rect.width;
        *height = rect.height;
    }

    #[test]
    fn undo_and_redo_restore_asymmetric_edits() {
        let mut document = Document::new(RgbaImage::new(80, 40));
        let first = document.add(
            Shape::Rectangle(Rect {
                x: 2.0,
                y: 3.0,
                width: 20.0,
                height: 10.0,
            }),
            [255, 0, 0, 255],
            2.0,
        );
        document.add(
            Shape::Line(Point { x: 1.0, y: 30.0 }, Point { x: 70.0, y: 4.0 }),
            [0, 255, 0, 255],
            3.0,
        );
        assert!(document.undo());
        assert_eq!(document.layers.len(), 1);
        assert_eq!(document.layers[0].id, first);
        assert!(document.redo());
        assert_eq!(document.layers.len(), 2);
    }

    #[test]
    fn layer_order_duplicate_and_delete_are_independently_undoable() {
        let mut document = Document::new(RgbaImage::new(160, 90));
        let back = document.add(
            Shape::Line(Point { x: 3.0, y: 8.0 }, Point { x: 71.0, y: 29.0 }),
            [10, 20, 30, 255],
            2.0,
        );
        let middle = document.add(
            Shape::Rectangle(Rect {
                x: 17.0,
                y: 11.0,
                width: 41.0,
                height: 23.0,
            }),
            [40, 50, 60, 255],
            3.0,
        );
        let front = document.add(
            Shape::Ellipse(Rect {
                x: 33.0,
                y: 19.0,
                width: 27.0,
                height: 39.0,
            }),
            [70, 80, 90, 255],
            4.0,
        );

        assert!(document.move_layer(back, isize::MAX));
        assert_eq!(
            document
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            [middle, front, back]
        );
        assert!(document.undo());
        assert_eq!(
            document
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            [back, middle, front]
        );

        assert!(document.toggle_locked(middle));
        let duplicate = document
            .duplicate(middle)
            .expect("locked layers remain duplicable");
        assert_eq!(document.layers.last().unwrap().id, duplicate);
        assert_eq!(document.layers.last().unwrap().name, "Rectangle copy");
        assert!(document.layers.last().unwrap().locked);
        assert!(document.undo());
        assert_eq!(document.layers.len(), 3);
        assert!(document.redo());
        assert_eq!(document.layers.len(), 4);

        assert!(!document.delete(duplicate));
        assert!(document.toggle_locked(duplicate));
        assert!(document.delete(duplicate));
        assert_eq!(document.layers.len(), 3);
        assert!(document.undo());
        assert_eq!(document.layers.last().unwrap().id, duplicate);
    }

    #[test]
    fn crop_render_has_exact_requested_dimensions() {
        let mut document = Document::new(RgbaImage::new(100, 80));
        assert!(document.set_crop(Rect {
            x: 10.0,
            y: 20.0,
            width: 33.0,
            height: 17.0,
        }));
        assert_eq!(document.render().unwrap().dimensions(), (33, 17));
        assert!(document.undo());
        assert_eq!(document.render().unwrap().dimensions(), (100, 80));
        assert!(document.redo());
        assert_eq!(document.render().unwrap().dimensions(), (33, 17));
    }

    #[test]
    fn canvas_expansion_background_and_output_resize_are_undoable() {
        let mut source = RgbaImage::from_pixel(3, 2, Rgba([12, 34, 56, 255]));
        source.put_pixel(2, 1, Rgba([91, 72, 53, 255]));
        let mut document = Document::new(source);
        assert!(document.set_canvas_size(7, 5).unwrap());
        assert!(document.set_background(Some([240, 230, 220, 255])));

        let rendered = document.render().unwrap();
        assert_eq!(rendered.dimensions(), (7, 5));
        assert_eq!(rendered.get_pixel(2, 1).0, [91, 72, 53, 255]);
        assert_eq!(rendered.get_pixel(6, 4).0, [240, 230, 220, 255]);
        assert_eq!(document.render_resized(5, 9).unwrap().dimensions(), (5, 9));

        assert!(document.undo());
        assert_eq!(document.background, None);
        assert_eq!((document.canvas_width, document.canvas_height), (7, 5));
        assert!(document.undo());
        assert_eq!((document.canvas_width, document.canvas_height), (3, 2));
    }

    #[test]
    fn expanded_canvas_crop_keeps_annotation_pixels_beyond_original_raster() {
        let mut source = RgbaImage::from_pixel(40, 20, Rgba([11, 22, 33, 255]));
        source.put_pixel(39, 19, Rgba([44, 55, 66, 255]));
        let mut document = Document::new(source);
        document.set_canvas_size(100, 70).unwrap();
        document.set_background(Some([5, 7, 9, 255]));
        let annotation = document.add(
            Shape::Rectangle(Rect {
                x: 63.0,
                y: 34.0,
                width: 11.0,
                height: 9.0,
            }),
            [210, 30, 20, 255],
            1.0,
        );
        assert!(document.set_layer_fill(annotation, Some([90, 170, 40, 255])));

        assert!(document.set_crop(Rect {
            x: 60.0,
            y: 30.0,
            width: 25.0,
            height: 23.0,
        }));
        assert_eq!(document.crop.x, 60.0);
        assert_eq!(document.crop.y, 30.0);
        let cropped = document.render().unwrap();
        assert_eq!(cropped.dimensions(), (25, 23));
        assert_eq!(cropped.get_pixel(8, 8).0, [90, 170, 40, 255]);
        assert_eq!(cropped.get_pixel(24, 22).0, [5, 7, 9, 255]);

        assert!(document.undo());
        assert_eq!(
            document.crop,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 20.0
            }
        );
        assert_eq!((document.canvas_width, document.canvas_height), (100, 70));
        let restored = document.render().unwrap();
        assert_eq!(restored.get_pixel(68, 38).0, [90, 170, 40, 255]);
        assert_eq!(restored.get_pixel(39, 19).0, [44, 55, 66, 255]);
    }

    #[test]
    fn contiguous_wand_edits_source_pixels_clears_background_and_undoes() {
        let mut source = RgbaImage::from_pixel(7, 5, Rgba([190, 20, 30, 255]));
        source.put_pixel(4, 1, Rgba([248, 248, 245, 255]));
        source.put_pixel(4, 2, Rgba([250, 247, 244, 255]));
        source.put_pixel(6, 4, Rgba([249, 249, 246, 255]));
        let mut document = Document::new(source);
        assert!(Arc::ptr_eq(&document.original, &document.source));
        assert!(document.set_background(Some([9, 11, 13, 255])));

        assert!(
            document
                .remove_background_wand(ImageTarget::Source, Point { x: 4.2, y: 2.2 }, 5, true,)
                .unwrap()
        );
        assert_eq!(document.background, None);
        let rendered = document.render().unwrap();
        assert_eq!(rendered.get_pixel(4, 1).0, [0, 0, 0, 0]);
        assert_eq!(rendered.get_pixel(4, 2).0, [0, 0, 0, 0]);
        assert_eq!(rendered.get_pixel(6, 4).0, [249, 249, 246, 255]);
        assert_eq!(rendered.get_pixel(0, 0).0, [190, 20, 30, 255]);
        assert!(!Arc::ptr_eq(&document.original, &document.source));

        assert!(document.undo());
        assert_eq!(document.background, Some([9, 11, 13, 255]));
        let restored = document.render().unwrap();
        assert_eq!(restored.get_pixel(4, 1).0, [248, 248, 245, 255]);
        assert_eq!(restored.get_pixel(4, 2).0, [250, 247, 244, 255]);
    }

    #[test]
    fn global_wand_reaches_disconnected_matches_and_soft_brush_has_alpha_falloff() {
        let mut source = RgbaImage::from_pixel(9, 7, Rgba([20, 40, 60, 255]));
        source.put_pixel(1, 1, Rgba([244, 242, 238, 255]));
        source.put_pixel(7, 5, Rgba([242, 245, 240, 255]));
        source.put_pixel(4, 3, Rgba([238, 242, 238, 255]));
        let mut document = Document::new(source);

        assert!(
            document
                .remove_background_wand(ImageTarget::Source, Point { x: 1.2, y: 1.4 }, 5, false,)
                .unwrap()
        );
        let global = document.render().unwrap();
        assert_eq!(global.get_pixel(1, 1)[3], 0);
        assert_eq!(global.get_pixel(7, 5)[3], 0);
        assert_eq!(global.get_pixel(4, 3).0, [238, 242, 238, 255]);

        assert!(document.undo());
        assert!(
            document
                .remove_background_stroke(
                    ImageTarget::Source,
                    &[Point { x: 4.0, y: 3.0 }],
                    6.0,
                    100.0,
                    false,
                )
                .unwrap()
        );
        let soft = document.render().unwrap();
        let center = soft.get_pixel(4, 3)[3];
        let feather = soft.get_pixel(6, 3)[3];
        assert!(
            center > 0 && center < feather,
            "soft center must be translucent"
        );
        assert!(feather < 255, "soft edge must change alpha");
        assert_eq!(soft.get_pixel(0, 0)[3], 255);
    }

    #[test]
    fn trim_uses_locked_visible_geometry_ignores_hidden_overhang_and_undoes_once() {
        let source = RgbaImage::from_pixel(40, 20, Rgba([12, 34, 56, 255]));
        let mut document = Document::new(source);
        assert!(!document.can_trim_to_visible_content());
        assert!(!document.toggle_source_visibility());
        assert!(document.undo());
        assert!(document.source_visible);
        assert!(document.redo());
        assert!(!document.source_visible);

        let pixels = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgba([220, 40, 20, 255])
            } else {
                Rgba([10, 80, 230, 255])
            }
        });
        let visible = document.add_image(pixels, 0, "Visible.png".into());
        let hidden = document.add_image(
            RgbaImage::from_pixel(1, 1, Rgba([1, 250, 2, 255])),
            1,
            "Hidden.png".into(),
        );
        if let Shape::Image {
            origin,
            width,
            height,
            ..
        } = &mut document
            .layers
            .iter_mut()
            .find(|layer| layer.id == visible)
            .unwrap()
            .shape
        {
            *origin = Point { x: -17.2, y: 23.4 };
            *width = 31.1;
            *height = 19.2;
        }
        document
            .layers
            .iter_mut()
            .find(|layer| layer.id == visible)
            .unwrap()
            .locked = true;
        let hidden_layer = document
            .layers
            .iter_mut()
            .find(|layer| layer.id == hidden)
            .unwrap();
        hidden_layer.visible = false;
        if let Shape::Image { origin, .. } = &mut hidden_layer.shape {
            *origin = Point { x: 500.0, y: 600.0 };
        }

        assert!(document.can_trim_to_visible_content());
        assert!(document.trim_to_visible_content().unwrap());
        assert_eq!(
            document.crop,
            Rect {
                x: -18.0,
                y: 23.0,
                width: 32.0,
                height: 20.0,
            }
        );
        let rendered = document.render().unwrap();
        assert_eq!(rendered.dimensions(), (32, 20));
        let left = rendered.get_pixel(1, 10);
        let right = rendered.get_pixel(30, 10);
        assert!(left[0] > left[2], "left image orientation changed");
        assert!(right[2] > right[0], "right image orientation changed");
        assert_eq!((left[3], right[3]), (255, 255));
        assert!(!document.trim_to_visible_content().unwrap());

        assert!(document.undo());
        assert_eq!((document.canvas_width, document.canvas_height), (40, 20));
        assert!(!document.source_visible);
        assert!(document.redo());
        assert_eq!((document.canvas_width, document.canvas_height), (32, 20));
    }

    #[test]
    fn trim_rejects_nonfinite_dimension_and_area_without_mutation_or_checkpoint() {
        let make_document = |frame: Rect| {
            let mut document = Document::new(RgbaImage::new(3, 2));
            document.toggle_source_visibility();
            let id = document.add_image(RgbaImage::new(1, 1), 0, "Far.png".into());
            set_image_geometry(&mut document, id, frame);
            document
        };

        let mut dimension = make_document(Rect {
            x: -7.0,
            y: 11.0,
            width: 16_385.0,
            height: 3.0,
        });
        let dimension_key = dimension.render_key();
        let dimension_crop = dimension.crop;
        let dimension_canvas = (dimension.canvas_width, dimension.canvas_height);
        assert_eq!(
            dimension.trim_to_visible_content(),
            Err("Trim canvas dimensions are limited to 16384 pixels")
        );
        assert_eq!(dimension.render_key(), dimension_key);
        assert_eq!(dimension.crop, dimension_crop);
        assert_eq!(
            (dimension.canvas_width, dimension.canvas_height),
            dimension_canvas
        );
        assert!(dimension.undo());
        assert!(
            dimension.layers.is_empty(),
            "rejection added an undo checkpoint"
        );

        let mut area = make_document(Rect {
            x: 13.0,
            y: -17.0,
            width: 10_001.0,
            height: 10_000.0,
        });
        let area_key = area.render_key();
        let area_crop = area.crop;
        let area_canvas = (area.canvas_width, area.canvas_height);
        assert_eq!(
            area.trim_to_visible_content(),
            Err("Trim canvas size is limited to 100 million pixels")
        );
        assert_eq!(area.render_key(), area_key);
        assert_eq!(area.crop, area_crop);
        assert_eq!((area.canvas_width, area.canvas_height), area_canvas);
        assert!(area.undo());
        assert!(area.layers.is_empty(), "rejection added an undo checkpoint");

        let mut nonfinite = make_document(Rect {
            x: f32::NAN,
            y: 0.0,
            width: 2.0,
            height: 3.0,
        });
        let nonfinite_key = nonfinite.render_key();
        assert_eq!(
            nonfinite.trim_to_visible_content(),
            Err("Trim bounds must be finite")
        );
        assert_eq!(nonfinite.render_key(), nonfinite_key);
    }

    #[test]
    fn trim_accepts_exact_limits_without_allocating_render_output() {
        let mut exact_dimension = Document::new(RgbaImage::new(1, 1));
        exact_dimension.toggle_source_visibility();
        let id = exact_dimension.add_image(RgbaImage::new(1, 1), 0, "Wide.png".into());
        set_image_geometry(
            &mut exact_dimension,
            id,
            Rect {
                x: -3.0,
                y: 5.0,
                width: 16_384.0,
                height: 2.0,
            },
        );
        assert!(exact_dimension.trim_to_visible_content().unwrap());
        assert_eq!(
            (exact_dimension.canvas_width, exact_dimension.canvas_height),
            (16_384, 2)
        );

        let mut exact_area = Document::new(RgbaImage::new(1, 1));
        exact_area.toggle_source_visibility();
        let id = exact_area.add_image(RgbaImage::new(1, 1), 0, "Area.png".into());
        set_image_geometry(
            &mut exact_area,
            id,
            Rect {
                x: 9.0,
                y: -4.0,
                width: 10_000.0,
                height: 10_000.0,
            },
        );
        assert!(exact_area.trim_to_visible_content().unwrap());
        assert_eq!(
            (exact_area.canvas_width, exact_area.canvas_height),
            (10_000, 10_000)
        );
    }

    #[test]
    fn negative_overhang_shifts_visible_asymmetric_source_in_expanded_frame() {
        let mut source = RgbaImage::from_pixel(3, 2, Rgba([0, 0, 0, 0]));
        source.put_pixel(0, 0, Rgba([11, 21, 31, 255]));
        source.put_pixel(1, 0, Rgba([41, 51, 61, 255]));
        source.put_pixel(2, 0, Rgba([71, 81, 91, 255]));
        source.put_pixel(0, 1, Rgba([101, 111, 121, 255]));
        let mut document = Document::new(source.clone());
        let id = document.add_image(
            RgbaImage::from_pixel(1, 1, Rgba([230, 20, 40, 255])),
            0,
            "Overhang.png".into(),
        );
        set_image_geometry(
            &mut document,
            id,
            Rect {
                x: -2.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        );

        assert!(document.trim_to_visible_content().unwrap());
        assert_eq!(
            document.crop,
            Rect {
                x: -2.0,
                y: 0.0,
                width: 5.0,
                height: 2.0
            }
        );
        let rendered = document.render().unwrap();
        assert_eq!(rendered.get_pixel(0, 0).0, [230, 20, 40, 255]);
        assert_eq!(rendered.get_pixel(1, 0).0, [0, 0, 0, 0]);
        assert_eq!(rendered.get_pixel(2, 0), source.get_pixel(0, 0));
        assert_eq!(rendered.get_pixel(4, 0), source.get_pixel(2, 0));
        assert_eq!(rendered.get_pixel(2, 1), source.get_pixel(0, 1));

        assert!(document.undo());
        assert_eq!(
            document.crop,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 3.0,
                height: 2.0
            }
        );
        assert!(document.source_visible);
    }

    #[test]
    fn eraser_targets_topmost_locked_rotated_image_and_restore_uses_frozen_pixels() {
        let mut document = Document::new(RgbaImage::from_pixel(80, 60, Rgba([2, 4, 8, 255])));
        let mut inset = RgbaImage::from_pixel(6, 4, Rgba([30, 70, 150, 255]));
        inset.put_pixel(2, 1, Rgba([201, 91, 37, 220]));
        let id = document.add_image(inset, 0, "Inset".into());
        assert!(document.set_layer_rotation(id, 90.0));
        assert!(document.toggle_locked(id));
        let layer = document.layers.iter().find(|layer| layer.id == id).unwrap();
        let Shape::Image {
            origin,
            width,
            height,
            ..
        } = layer.shape
        else {
            panic!("expected image");
        };
        let center = Point {
            x: origin.x + width / 2.0,
            y: origin.y + height / 2.0,
        };
        let sample = rotate(
            Point {
                x: origin.x + width * 2.5 / 6.0,
                y: origin.y + height * 1.5 / 4.0,
            },
            center,
            90.0,
        );
        assert_eq!(
            document.hit_test_image(sample),
            Some(ImageTarget::Layer(id))
        );

        assert!(
            document
                .remove_background_stroke(ImageTarget::Layer(id), &[sample], 1.0, 0.0, false)
                .unwrap()
        );
        let erased = document.layers.iter().find(|layer| layer.id == id).unwrap();
        let Shape::Image { pixels, .. } = &erased.shape else {
            panic!("expected image");
        };
        assert_eq!(pixels.get_pixel(2, 1).0, [0, 0, 0, 0]);
        assert_eq!(
            erased.original_pixels.as_ref().unwrap().get_pixel(2, 1).0,
            [201, 91, 37, 220]
        );

        assert!(
            document
                .remove_background_stroke(ImageTarget::Layer(id), &[sample], 1.0, 0.0, true)
                .unwrap()
        );
        let restored = document.layers.iter().find(|layer| layer.id == id).unwrap();
        let Shape::Image { pixels, .. } = &restored.shape else {
            panic!("expected image");
        };
        assert_eq!(pixels.get_pixel(2, 1).0, [201, 91, 37, 220]);
        assert!(document.undo());
        let Shape::Image { pixels, .. } = &document.layers.last().unwrap().shape else {
            panic!("expected image");
        };
        assert_eq!(pixels.get_pixel(2, 1).0, [0, 0, 0, 0]);
    }

    #[test]
    fn fast_hard_erase_stroke_has_no_gaps_and_is_one_undo_step() {
        let source = RgbaImage::from_pixel(31, 9, Rgba([80, 120, 160, 255]));
        let mut document = Document::new(source);
        let points = [Point { x: 2.0, y: 4.0 }, Point { x: 28.0, y: 4.0 }];
        assert!(
            document
                .remove_background_stroke(ImageTarget::Source, &points, 3.0, 0.0, false)
                .unwrap()
        );
        let erased = document.render().unwrap();
        assert_eq!(erased.get_pixel(2, 4)[3], 0);
        assert_eq!(erased.get_pixel(15, 4)[3], 0);
        assert_eq!(erased.get_pixel(28, 4)[3], 0);
        assert_eq!(erased.get_pixel(15, 0)[3], 255);
        assert!(document.undo());
        assert_eq!(document.render().unwrap().get_pixel(15, 4)[3], 255);
        assert!(!document.undo());
    }

    #[test]
    fn undo_capacity_keeps_the_newest_checkpoint() {
        let mut document = Document::new(RgbaImage::new(100, 100));
        for x in 0..70 {
            document.add(
                Shape::Rectangle(Rect {
                    x: x as f32,
                    y: 1.0,
                    width: 2.0,
                    height: 3.0,
                }),
                [255, 0, 0, 255],
                1.0,
            );
        }
        assert!(document.undo());
        assert_eq!(document.layers.len(), 69);
        for _ in 1..64 {
            assert!(document.undo());
        }
        assert_eq!(document.layers.len(), 6);
        assert!(!document.undo());
    }

    #[test]
    fn hidden_top_layer_does_not_intercept_hit_testing() {
        let mut document = Document::new(RgbaImage::new(100, 100));
        let visible = document.add(
            Shape::Rectangle(Rect {
                x: 10.0,
                y: 10.0,
                width: 30.0,
                height: 30.0,
            }),
            [255, 0, 0, 255],
            4.0,
        );
        let hidden = document.add(
            Shape::Rectangle(Rect {
                x: 10.0,
                y: 10.0,
                width: 30.0,
                height: 30.0,
            }),
            [0, 255, 0, 255],
            4.0,
        );
        assert!(document.toggle_visibility(hidden));
        assert_ne!(visible, hidden);
        assert_eq!(
            document.hit_test(Point { x: 10.0, y: 20.0 }, 2.0),
            Some(visible)
        );
        assert!(document.undo());
        assert_eq!(
            document.hit_test(Point { x: 10.0, y: 20.0 }, 2.0),
            Some(hidden)
        );
        assert!(document.toggle_locked(hidden));
        assert_eq!(
            document.hit_test(Point { x: 10.0, y: 20.0 }, 2.0),
            Some(visible)
        );
    }

    #[test]
    fn layer_opacity_scales_fill_and_outline_alpha() {
        let mut document = Document::new(RgbaImage::new(80, 60));
        let id = document.add(
            Shape::Rectangle(Rect {
                x: 10.0,
                y: 10.0,
                width: 50.0,
                height: 35.0,
            }),
            [220, 40, 30, 240],
            4.0,
        );
        assert!(document.set_layer_fill(id, Some([20, 190, 60, 200])));
        assert!(document.set_layer_opacity(id, 128));
        let rendered = document.render().unwrap();
        let fill = rendered.get_pixel(35, 28);
        let outline = rendered.get_pixel(10, 25);
        assert!((98..=102).contains(&fill[3]), "fill alpha was {}", fill[3]);
        // At the edge the independently scaled fill (~100) and outline (~120)
        // composite to ~173; an unscaled fill would push this above 220.
        assert!(
            (171..=175).contains(&outline[3]),
            "outline alpha was {}",
            outline[3]
        );

        let mut image_document = Document::new(RgbaImage::new(20, 20));
        let mut pixels = RgbaImage::new(2, 2);
        pixels
            .pixels_mut()
            .for_each(|pixel| *pixel = image::Rgba([40, 80, 160, 200]));
        let image_id = image_document.add_image(pixels, 0, "alpha.png".into());
        assert!(image_document.set_layer_opacity(image_id, 128));
        let image = image_document.render().unwrap();
        let pixel = image.pixels().find(|pixel| pixel[3] > 0).unwrap();
        assert!(
            (98..=102).contains(&pixel[3]),
            "image alpha was {}",
            pixel[3]
        );
        assert!(image_document.set_layer_blend_mode(image_id, BlendMode::Multiply));
        assert_eq!(image_document.layers[0].blend_mode, BlendMode::Multiply);
        assert!(image_document.undo());
        assert_eq!(image_document.layers[0].blend_mode, BlendMode::Normal);
    }

    #[test]
    fn rotated_asymmetric_resize_preserves_rotation_and_is_one_undo_step() {
        let mut document = Document::new(RgbaImage::new(300, 200));
        let id = document.add(
            Shape::Rectangle(Rect {
                x: 20.0,
                y: 30.0,
                width: 80.0,
                height: 30.0,
            }),
            [12, 34, 56, 255],
            5.0,
        );
        assert!(document.set_layer_rotation(id, 30.0));
        let original = document.layers[0].clone();
        let corners = original.selection_corners().unwrap();
        let edited = resize_from_corner(
            &original,
            2,
            Point {
                x: corners[2].x + 47.0,
                y: corners[2].y + 19.0,
            },
        )
        .unwrap();
        assert_eq!(edited.rotation_degrees, 30.0);
        let resized = edited.geometry_bounds().unwrap();
        assert!((resized.width - resized.height).abs() > 25.0);
        let edited_corners = edited.selection_corners().unwrap();
        assert!((edited_corners[0].x - corners[0].x).abs() < 0.001);
        assert!((edited_corners[0].y - corners[0].y).abs() < 0.001);
        assert!(document.preview_layer(edited.clone()));
        assert!(document.commit_layer_preview(original.clone()));
        assert_eq!(document.layers[0], edited);
        assert!(document.undo());
        assert_eq!(document.layers[0], original);
        assert!(document.redo());
        assert_eq!(document.layers[0], edited);
    }

    #[test]
    fn horizontal_line_resizes_by_endpoint_and_crossing_keeps_anchor() {
        let layer = Layer {
            id: 9,
            name: "Line".into(),
            shape: Shape::Line(Point { x: 10.0, y: 20.0 }, Point { x: 90.0, y: 20.0 }),
            original_pixels: None,
            color: [1, 2, 3, 255],
            stroke: 4.0,
            fill: None,
            opacity: 255,
            blend_mode: BlendMode::Normal,
            rotation_degrees: 0.0,
            visible: true,
            locked: false,
        };
        assert_eq!(
            layer.resize_handles(),
            vec![
                (4, Point { x: 10.0, y: 20.0 }),
                (5, Point { x: 90.0, y: 20.0 })
            ]
        );
        let resized = resize_from_corner(&layer, 5, Point { x: 122.0, y: 47.0 }).unwrap();
        assert_eq!(
            resized.shape,
            Shape::Line(Point { x: 10.0, y: 20.0 }, Point { x: 122.0, y: 47.0 })
        );
        let crossed = resize_from_corner(&layer, 5, Point { x: -31.0, y: 53.0 }).unwrap();
        assert_eq!(
            crossed.shape,
            Shape::Line(Point { x: 10.0, y: 20.0 }, Point { x: -31.0, y: 53.0 })
        );
    }

    #[test]
    fn rotated_rectangle_crossing_keeps_physical_opposite_corner_fixed() {
        let layer = Layer {
            id: 4,
            name: "Rectangle".into(),
            shape: Shape::Rectangle(Rect {
                x: 20.0,
                y: 30.0,
                width: 80.0,
                height: 35.0,
            }),
            original_pixels: None,
            color: [4, 5, 6, 255],
            stroke: 3.0,
            fill: Some([7, 8, 9, 128]),
            opacity: 255,
            blend_mode: BlendMode::Normal,
            rotation_degrees: 31.0,
            visible: true,
            locked: false,
        };
        let original = layer.selection_corners().unwrap();
        let fixed = original[0];
        let pointer = rotate(
            Point {
                x: fixed.x - 27.0,
                y: fixed.y - 13.0,
            },
            fixed,
            layer.rotation_degrees,
        );
        let resized = resize_from_corner(&layer, 2, pointer).unwrap();
        let corners = resized.selection_corners().unwrap();
        assert!(
            corners
                .iter()
                .any(|point| { (point.x - fixed.x).hypot(point.y - fixed.y) < 0.001 })
        );
        assert!(
            corners
                .iter()
                .any(|point| { (point.x - pointer.x).hypot(point.y - pointer.y) < 0.001 })
        );
    }

    #[test]
    fn rotated_bounds_and_move_use_source_coordinates_and_undo_once() {
        let mut document = Document::new(RgbaImage::new(300, 200));
        let id = document.add(
            Shape::Rectangle(Rect {
                x: 10.0,
                y: 20.0,
                width: 70.0,
                height: 30.0,
            }),
            [30, 40, 50, 255],
            3.0,
        );
        assert!(document.set_layer_rotation(id, 90.0));
        let original = document.layers[0].clone();
        let corners = original.selection_corners().unwrap();
        assert!((corners[0].x - 60.0).abs() < 0.001);
        assert!((corners[0].y - 0.0).abs() < 0.001);
        assert!((corners[2].x - 30.0).abs() < 0.001);
        assert!((corners[2].y - 70.0).abs() < 0.001);

        let moved = original.translated(Point { x: 17.0, y: -9.0 });
        assert!(document.preview_layer(moved.clone()));
        assert!(document.commit_layer_preview(original.clone()));
        assert_eq!(
            moved.geometry_bounds().unwrap(),
            Rect {
                x: 27.0,
                y: 11.0,
                width: 70.0,
                height: 30.0,
            }
        );
        assert!(document.undo());
        assert_eq!(document.layers[0], original);
        assert!(document.redo());
        assert_eq!(document.layers[0], moved);
    }

    #[test]
    fn selected_properties_are_undoable_without_changing_geometry() {
        let mut document = Document::new(RgbaImage::new(100, 100));
        let id = document.add(
            Shape::Line(Point { x: 7.0, y: 9.0 }, Point { x: 80.0, y: 51.0 }),
            [1, 2, 3, 255],
            2.0,
        );
        let shape = document.layers[0].shape.clone();
        assert!(document.set_layer_color(id, [90, 80, 70, 255]));
        assert!(document.set_layer_stroke(id, 11.0));
        assert!(document.set_layer_fill(id, Some([4, 5, 6, 128])));
        assert_eq!(document.layers[0].shape, shape);
        assert!(document.undo());
        assert_eq!(document.layers[0].fill, None);
        assert!(document.undo());
        assert_eq!(document.layers[0].stroke, 2.0);
        assert!(document.undo());
        assert_eq!(document.layers[0].color, [1, 2, 3, 255]);
    }

    #[test]
    fn freehand_keeps_intermediate_samples_and_drops_pointer_noise() {
        let mut gesture = FreehandGesture::begin(Point { x: 1.0, y: 2.0 });
        gesture.sample(Point { x: 1.1, y: 2.1 });
        gesture.sample(Point { x: 8.0, y: 3.0 });
        gesture.sample(Point { x: 13.0, y: 9.0 });
        let points = gesture.finish(Point { x: 20.0, y: 7.0 });
        assert_eq!(points.len(), 4);
        assert_eq!(points[1], Point { x: 8.0, y: 3.0 });
        assert_eq!(points[2], Point { x: 13.0, y: 9.0 });
    }

    #[test]
    fn imported_image_preserves_aspect_and_is_undoable() {
        let mut document = Document::new(RgbaImage::new(400, 200));
        let mut imported = RgbaImage::new(80, 40);
        imported.put_pixel(79, 0, image::Rgba([17, 91, 203, 255]));

        let id = document.add_image(imported, 0, "asymmetric.png".into());
        let layer = document.layers.iter().find(|layer| layer.id == id).unwrap();
        let Shape::Image {
            origin,
            width,
            height,
            ..
        } = &layer.shape
        else {
            panic!("expected image layer");
        };
        assert_eq!((*width, *height), (80.0, 40.0));
        assert_eq!(*origin, Point { x: 160.0, y: 80.0 });
        assert!(
            document
                .hit_test(Point { x: 239.0, y: 81.0 }, 0.0)
                .is_some()
        );
        assert!(document.undo());
        assert!(document.layers.is_empty());
        assert!(document.redo());
        assert!(matches!(document.layers[0].shape, Shape::Image { .. }));
    }

    #[test]
    fn multi_image_import_is_one_undo_step_and_preserves_order() {
        let mut document = Document::new(RgbaImage::new(640, 360));
        let ids = document.add_images(
            vec![
                (RgbaImage::new(23, 41), "portrait.png".into()),
                (RgbaImage::new(91, 17), "banner.webp".into()),
                (RgbaImage::new(37, 29), "tile.jpg".into()),
            ],
            0,
        );
        assert_eq!(ids.len(), 3);
        assert_eq!(
            document
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect::<Vec<_>>(),
            ["portrait.png", "banner.webp", "tile.jpg"]
        );
        assert!(document.undo());
        assert!(document.layers.is_empty());
        assert!(!document.undo());
        assert!(document.redo());
        assert_eq!(document.layers.len(), 3);
    }

    #[test]
    fn lock_blocks_edits_but_unlock_and_undo_restore_state() {
        let mut document = Document::new(RgbaImage::new(200, 120));
        let id = document.add(
            Shape::Rectangle(Rect {
                x: 17.0,
                y: 29.0,
                width: 61.0,
                height: 33.0,
            }),
            [10, 20, 30, 255],
            4.0,
        );
        assert!(document.toggle_locked(id));
        assert!(!document.set_layer_opacity(id, 73));
        assert!(!document.delete(id));
        assert!(document.layers[0].locked);
        assert!(document.toggle_locked(id));
        assert!(document.set_layer_opacity(id, 73));
        assert_eq!(document.layers[0].opacity, 73);
        assert!(document.undo());
        assert_eq!(document.layers[0].opacity, 255);
        assert!(document.undo());
        assert!(document.layers[0].locked);
    }

    #[test]
    fn imported_image_resize_crossing_corner_normalizes_geometry() {
        let layer = Layer {
            id: 9,
            name: "Inset".into(),
            shape: Shape::Image {
                origin: Point { x: 10.0, y: 20.0 },
                width: 80.0,
                height: 40.0,
                pixels: Arc::new(RgbaImage::new(20, 10)),
            },
            original_pixels: None,
            color: [255; 4],
            stroke: 1.0,
            fill: None,
            opacity: 255,
            blend_mode: BlendMode::Normal,
            rotation_degrees: 0.0,
            visible: true,
            locked: false,
        };
        let resized = resize_from_corner(&layer, 0, Point { x: 110.0, y: 75.0 }).unwrap();
        let Shape::Image {
            origin,
            width,
            height,
            ..
        } = resized.shape
        else {
            panic!("expected image layer");
        };
        assert_eq!(origin, Point { x: 90.0, y: 60.0 });
        assert_eq!((width, height), (20.0, 15.0));
    }
}
