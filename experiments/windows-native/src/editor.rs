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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Stroke(Vec<Point>),
    Arrow(Point, Point),
    Line(Point, Point),
    Rectangle(Rect),
    Ellipse(Rect),
    Text {
        origin: Point,
        value: String,
        font_size: f32,
        font_data: Arc<[u8]>,
    },
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
    undo: Vec<Vec<Layer>>,
    redo: Vec<Vec<Layer>>,
    next_id: u64,
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

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.layers, previous));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.layers, next));
        true
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.layers.clone());
        self.undo.truncate(64);
        self.redo.clear();
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
        self.layers.iter().rev().find_map(|layer| {
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
        document.crop = Rect {
            x: 10.0,
            y: 20.0,
            width: 33.0,
            height: 17.0,
        };
        assert_eq!(document.render().unwrap().dimensions(), (33, 17));
    }
}
