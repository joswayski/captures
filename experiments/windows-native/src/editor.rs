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

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: u64,
    pub shape: Shape,
    pub color: [u8; 4],
    pub stroke: f32,
    pub fill: Option<[u8; 4]>,
    pub rotation_degrees: f32,
    pub visible: bool,
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
