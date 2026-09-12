use crate::geometry::{Point, Rect};
use image::{Rgba, RgbaImage};

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
    Text { origin: Point, value: String },
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: u64,
    pub shape: Shape,
    pub color: [u8; 4],
    pub stroke: f32,
    pub visible: bool,
}

#[derive(Clone)]
pub struct Document {
    original: RgbaImage,
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
            original: image,
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

    pub fn render(&self) -> RgbaImage {
        let crop =
            self.crop
                .to_logical()
                .to_physical(1.0, self.original.width(), self.original.height());
        let mut output =
            image::imageops::crop_imm(&self.original, crop.x, crop.y, crop.width, crop.height)
                .to_image();
        for layer in self.layers.iter().filter(|layer| layer.visible) {
            draw_layer(
                &mut output,
                layer,
                Point {
                    x: self.crop.x,
                    y: self.crop.y,
                },
            );
        }
        output
    }
}

fn draw_layer(image: &mut RgbaImage, layer: &Layer, offset: Point) {
    let color = Rgba(layer.color);
    let line = |image: &mut RgbaImage, a: Point, b: Point| {
        draw_line(
            image,
            Point {
                x: a.x - offset.x,
                y: a.y - offset.y,
            },
            Point {
                x: b.x - offset.x,
                y: b.y - offset.y,
            },
            layer.stroke,
            color,
        )
    };
    match &layer.shape {
        Shape::Stroke(points) => {
            for pair in points.windows(2) {
                line(image, pair[0], pair[1]);
            }
        }
        Shape::Arrow(a, b) | Shape::Line(a, b) => line(image, *a, *b),
        Shape::Rectangle(rect) => {
            let a = Point {
                x: rect.x,
                y: rect.y,
            };
            let b = Point {
                x: rect.x + rect.width,
                y: rect.y,
            };
            let c = Point {
                x: b.x,
                y: rect.y + rect.height,
            };
            let d = Point { x: rect.x, y: c.y };
            line(image, a, b);
            line(image, b, c);
            line(image, c, d);
            line(image, d, a);
        }
        Shape::Ellipse(rect) => {
            let center = Point {
                x: rect.x + rect.width / 2.0,
                y: rect.y + rect.height / 2.0,
            };
            let mut previous = None;
            for step in 0..=64 {
                let angle = step as f32 * std::f32::consts::TAU / 64.0;
                let point = Point {
                    x: center.x + rect.width / 2.0 * angle.cos(),
                    y: center.y + rect.height / 2.0 * angle.sin(),
                };
                if let Some(last) = previous {
                    line(image, last, point);
                }
                previous = Some(point);
            }
        }
        Shape::Text { .. } => {} // DirectWrite owns live text; flattened by the Windows renderer.
    }
}

fn draw_line(image: &mut RgbaImage, a: Point, b: Point, width: f32, color: Rgba<u8>) {
    let distance = (b.x - a.x).abs().max((b.y - a.y).abs()).ceil().max(1.0) as u32;
    let radius = (width / 2.0).ceil() as i32;
    for step in 0..=distance {
        let t = step as f32 / distance as f32;
        let x = (a.x + (b.x - a.x) * t).round() as i32;
        let y = (a.y + (b.y - a.y) * t).round() as i32;
        for oy in -radius..=radius {
            for ox in -radius..=radius {
                if ox * ox + oy * oy <= radius * radius
                    && let (Ok(px), Ok(py)) = (u32::try_from(x + ox), u32::try_from(y + oy))
                    && px < image.width()
                    && py < image.height()
                {
                    image.put_pixel(px, py, color);
                }
            }
        }
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
        assert_eq!(document.render().dimensions(), (33, 17));
    }
}
