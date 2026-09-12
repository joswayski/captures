use crate::geometry::{Point, Rect};
use image::RgbaImage;
use std::sync::Arc;

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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Stroke(Vec<Point>),
    Arrow(Point, Point),
    Line(Point, Point),
    Rectangle(Rect),
    Ellipse(Rect),
    Polygon(Vec<Point>),
    Text {
        origin: Point,
        value: String,
        font_size: f32,
        font_data: Arc<[u8]>,
    },
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
    pub shape: Shape,
    pub color: [u8; 4],
    pub stroke: f32,
    pub fill: Option<[u8; 4]>,
    pub rotation_degrees: f32,
    pub visible: bool,
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
    original: Arc<RgbaImage>,
    pub crop: Rect,
    pub layers: Vec<Layer>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    next_id: u64,
}

#[derive(Clone)]
struct Snapshot {
    crop: Rect,
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
        Self {
            original: Arc::new(image),
            crop,
            layers: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
        }
    }

    pub fn add(&mut self, shape: Shape, color: [u8; 4], stroke: f32) -> u64 {
        self.checkpoint();
        let id = self.next_id;
        self.next_id += 1;
        self.layers.push(Layer {
            id,
            shape,
            color,
            stroke: stroke.clamp(1.0, 48.0),
            fill: None,
            rotation_degrees: 0.0,
            visible: true,
        });
        id
    }

    pub fn delete(&mut self, id: u64) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
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
    }

    pub fn set_crop(&mut self, crop: Rect) -> bool {
        let right = (crop.x + crop.width).min(self.original.width() as f32);
        let bottom = (crop.y + crop.height).min(self.original.height() as f32);
        let crop = Rect {
            x: crop
                .x
                .clamp(0.0, self.original.width().saturating_sub(1) as f32),
            y: crop
                .y
                .clamp(0.0, self.original.height().saturating_sub(1) as f32),
            width: (right - crop.x.max(0.0)).max(1.0),
            height: (bottom - crop.y.max(0.0)).max(1.0),
        };
        if crop == self.crop {
            return false;
        }
        self.checkpoint();
        self.crop = crop;
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(previous);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            crop: self.crop,
            layers: self.layers.clone(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.crop = snapshot.crop;
        self.layers = snapshot.layers;
    }

    pub fn render(&self) -> Result<RgbaImage, String> {
        captures_image::render(&captures_image::Document {
            source: self.original.clone(),
            crop: Some(captures_image::PixelRect {
                x: self.crop.x.max(0.0).round() as u32,
                y: self.crop.y.max(0.0).round() as u32,
                width: self.crop.width.max(1.0).round() as u32,
                height: self.crop.height.max(1.0).round() as u32,
            }),
            layers: self
                .layers
                .iter()
                .filter(|layer| layer.visible)
                .map(to_raster_layer)
                .collect(),
        })
    }

    pub fn hit_test(&self, point: Point, tolerance: f32) -> Option<u64> {
        self.layers
            .iter()
            .rev()
            .filter(|layer| layer.visible)
            .find_map(|layer| {
                let raster = to_raster_layer(layer);
                raster
                    .hit_test(to_raster_point(point), tolerance)
                    .then_some(layer.id)
            })
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
    captures_image::Layer {
        id: layer.id,
        shape,
        color: layer.color,
        stroke_width: layer.stroke,
        fill: layer.fill,
        rotation_degrees: layer.rotation_degrees,
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
            shape: Shape::Line(Point { x: 10.0, y: 20.0 }, Point { x: 90.0, y: 20.0 }),
            color: [1, 2, 3, 255],
            stroke: 4.0,
            fill: None,
            rotation_degrees: 0.0,
            visible: true,
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
            shape: Shape::Rectangle(Rect {
                x: 20.0,
                y: 30.0,
                width: 80.0,
                height: 35.0,
            }),
            color: [4, 5, 6, 255],
            stroke: 3.0,
            fill: Some([7, 8, 9, 128]),
            rotation_degrees: 31.0,
            visible: true,
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
}
