//! Shared screenshot-editor document, geometry, and snapshot history.
//!
//! This is the persisted editor model, not the flattened bitmap renderer model.
//! Unknown JSON fields are retained so a native host can edit a version-1 draft
//! without discarding data owned by the shipping editor. Host-only selection,
//! crop-preview, and rendering state do not belong here.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const HISTORY_LIMIT: usize = 100;
pub const MAX_CANVAS_DIMENSION: f64 = 32_768.;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub width: f64,
    pub height: f64,
    pub background: Option<String>,
    pub elements: Vec<Element>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Element {
    Image(ImageElement),
    Text(TextElement),
    Shape(ShapeElement),
    Path(PathElement),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementBase {
    pub id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    pub locked: bool,
    pub visible: bool,
    pub opacity: f64,
    pub blend_mode: String,
}

impl ElementBase {
    #[must_use]
    pub fn rotation(&self) -> f64 {
        self.rotation.unwrap_or(0.)
    }

    fn translate(&mut self, delta_x: f64, delta_y: f64) {
        self.x += delta_x;
        self.y += delta_y;
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub source: String,
    pub src: String,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub original_src: OptionalNullable<String>,
    pub name: String,
    pub source_artifact_id: Option<String>,
    pub width: f64,
    pub height: f64,
    pub natural_width: f64,
    pub natural_height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<ImageOrientation>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub text: String,
    pub font_size: f64,
    pub width: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_width: Option<bool>,
    pub font_family: String,
    pub bold: bool,
    pub italic: bool,
    pub align: String,
    pub color: String,
    pub background: Option<String>,
    pub outlined: bool,
    pub rounded_background: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_shadow: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_shadow_style: Option<DropShadowStyle>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TextElement {
    #[must_use]
    pub fn uses_auto_width(&self) -> bool {
        self.auto_width.unwrap_or(false)
    }

    #[must_use]
    pub fn has_drop_shadow(&self) -> bool {
        self.drop_shadow.unwrap_or(false)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub shape: String,
    pub end_x: f64,
    pub end_y: f64,
    pub controls: Vec<Point>,
    pub style: ElementStyle,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathElement {
    #[serde(flatten)]
    pub base: ElementBase,
    pub points: Vec<Point>,
    pub style: ElementStyle,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementStyle {
    pub color: String,
    pub fill: Option<String>,
    pub stroke_width: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_shadow: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_shadow_style: Option<DropShadowStyle>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ElementStyle {
    #[must_use]
    pub fn has_stroke(&self) -> bool {
        self.stroke_enabled.unwrap_or(true)
    }

    #[must_use]
    pub fn has_drop_shadow(&self) -> bool {
        self.drop_shadow.unwrap_or(false)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropShadowStyle {
    pub color: String,
    pub opacity: f64,
    pub blur: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Distinguishes an omitted legacy field from an explicit JSON `null`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum OptionalNullable<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<T> OptionalNullable<T> {
    fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

impl<T: Serialize> Serialize for OptionalNullable<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing | Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OptionalNullable<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer)
            .map_or_else(Err, |value| Ok(value.map_or(Self::Null, Self::Value)))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ImageOrientation {
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "rotate-90")]
    Rotate90,
    #[serde(rename = "rotate-180")]
    Rotate180,
    #[serde(rename = "rotate-270")]
    Rotate270,
    #[serde(rename = "flip-horizontal")]
    FlipHorizontal,
    #[serde(rename = "flip-vertical")]
    FlipVertical,
    #[serde(rename = "transpose")]
    Transpose,
    #[serde(rename = "transverse")]
    Transverse,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageTransform {
    RotateClockwise,
    RotateCounterclockwise,
    FlipHorizontal,
    FlipVertical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrientationMatrix {
    pub a: i8,
    pub b: i8,
    pub c: i8,
    pub d: i8,
}

impl ImageOrientation {
    #[must_use]
    pub const fn matrix(self) -> OrientationMatrix {
        match self {
            Self::Normal => OrientationMatrix::new(1, 0, 0, 1),
            Self::Rotate90 => OrientationMatrix::new(0, 1, -1, 0),
            Self::Rotate180 => OrientationMatrix::new(-1, 0, 0, -1),
            Self::Rotate270 => OrientationMatrix::new(0, -1, 1, 0),
            Self::FlipHorizontal => OrientationMatrix::new(-1, 0, 0, 1),
            Self::FlipVertical => OrientationMatrix::new(1, 0, 0, -1),
            Self::Transpose => OrientationMatrix::new(0, 1, 1, 0),
            Self::Transverse => OrientationMatrix::new(0, -1, -1, 0),
        }
    }

    fn from_matrix(matrix: OrientationMatrix) -> Self {
        [
            Self::Normal,
            Self::Rotate90,
            Self::Rotate180,
            Self::Rotate270,
            Self::FlipHorizontal,
            Self::FlipVertical,
            Self::Transpose,
            Self::Transverse,
        ]
        .into_iter()
        .find(|orientation| orientation.matrix() == matrix)
        .expect("D4 composition must produce another D4 orientation")
    }
}

impl OrientationMatrix {
    const fn new(a: i8, b: i8, c: i8, d: i8) -> Self {
        Self { a, b, c, d }
    }

    const fn compose(self, current: Self) -> Self {
        Self {
            a: self.a * current.a + self.c * current.b,
            b: self.b * current.a + self.d * current.b,
            c: self.a * current.c + self.c * current.d,
            d: self.b * current.c + self.d * current.d,
        }
    }
}

impl ImageElement {
    #[must_use]
    pub fn resolved_orientation(&self) -> ImageOrientation {
        self.orientation.unwrap_or(ImageOrientation::Normal)
    }

    /// Left-compose a lossless transform in displayed axes and preserve center.
    pub fn transform(&mut self, action: ImageTransform) {
        let operation = match action {
            ImageTransform::RotateClockwise => ImageOrientation::Rotate90,
            ImageTransform::RotateCounterclockwise => ImageOrientation::Rotate270,
            ImageTransform::FlipHorizontal => ImageOrientation::FlipHorizontal,
            ImageTransform::FlipVertical => ImageOrientation::FlipVertical,
        };
        let orientation = ImageOrientation::from_matrix(
            operation
                .matrix()
                .compose(self.resolved_orientation().matrix()),
        );
        let rotates = matches!(
            action,
            ImageTransform::RotateClockwise | ImageTransform::RotateCounterclockwise
        );
        let (width, height) = if rotates {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let center_x = self.base.x + self.width / 2.;
        let center_y = self.base.y + self.height / 2.;
        self.orientation = (orientation != ImageOrientation::Normal).then_some(orientation);
        self.base.x = center_x - width / 2.;
        self.base.y = center_y - height / 2.;
        self.width = width;
        self.height = height;
    }
}

impl Element {
    #[must_use]
    pub fn base(&self) -> &ElementBase {
        match self {
            Self::Image(element) => &element.base,
            Self::Text(element) => &element.base,
            Self::Shape(element) => &element.base,
            Self::Path(element) => &element.base,
        }
    }

    pub fn translate(&mut self, delta_x: f64, delta_y: f64) {
        match self {
            Self::Image(element) => element.base.translate(delta_x, delta_y),
            Self::Text(element) => element.base.translate(delta_x, delta_y),
            Self::Shape(element) => {
                element.base.translate(delta_x, delta_y);
                element.end_x += delta_x;
                element.end_y += delta_y;
                translate_points(&mut element.controls, delta_x, delta_y);
            }
            Self::Path(element) => {
                element.base.translate(delta_x, delta_y);
                translate_points(&mut element.points, delta_x, delta_y);
            }
        }
    }
}

fn translate_points(points: &mut [Point], delta_x: f64, delta_y: f64) {
    for point in points {
        point.x += delta_x;
        point.y += delta_y;
    }
}

impl Document {
    #[must_use]
    pub fn new_capture(
        src: impl Into<String>,
        width: f64,
        height: f64,
        source_artifact_id: Option<String>,
    ) -> Self {
        let src = src.into();
        let base = ElementBase {
            id: "capture-background".into(),
            x: 0.,
            y: 0.,
            rotation: None,
            locked: true,
            visible: true,
            opacity: 100.,
            blend_mode: "source-over".into(),
        };
        Self {
            width: width.round().max(1.),
            height: height.round().max(1.),
            background: Some("#f7f7f5".into()),
            elements: vec![Element::Image(ImageElement {
                base,
                source: "background".into(),
                src,
                original_src: OptionalNullable::Null,
                name: "Original screenshot".into(),
                source_artifact_id,
                width,
                height,
                natural_width: width,
                natural_height: height,
                orientation: None,
                extra: Map::new(),
            })],
            extra: Map::new(),
        }
    }

    pub fn translate(&mut self, delta_x: f64, delta_y: f64) {
        for element in &mut self.elements {
            element.translate(delta_x, delta_y);
        }
    }

    pub fn resize_canvas(&mut self, width: f64, height: f64) {
        self.width = clamp(width.round(), 1., MAX_CANVAS_DIMENSION);
        self.height = clamp(height.round(), 1., MAX_CANVAS_DIMENSION);
    }

    pub fn crop(&mut self, crop: Rect) {
        let x = clamp(crop.x.round(), 0., (self.width - 1.).max(0.));
        let y = clamp(crop.y.round(), 0., (self.height - 1.).max(0.));
        let width = clamp(crop.width.round(), 1., self.width).min(self.width - x);
        let height = clamp(crop.height.round(), 1., self.height).min(self.height - y);
        self.width = width;
        self.height = height;
        self.translate(-x, -y);
    }
}

#[must_use]
pub fn bounded_crop_rect(start: Point, end: Point, bounds: Rect, aspect: Option<f64>) -> Rect {
    let start = Point {
        x: clamp(start.x, 0., bounds.width),
        y: clamp(start.y, 0., bounds.height),
    };
    let mut end = Point {
        x: clamp(end.x, 0., bounds.width),
        y: clamp(end.y, 0., bounds.height),
    };
    if let Some(aspect) = aspect.filter(|value| value.is_finite() && *value > 0.) {
        let direction_x = if end.x < start.x { -1. } else { 1. };
        let direction_y = if end.y < start.y { -1. } else { 1. };
        let mut width = (end.x - start.x).abs();
        let mut height = (end.y - start.y).abs();
        if height != 0. && width / height <= aspect {
            width = height * aspect;
        }
        width = width.min(if direction_x > 0. {
            bounds.width - start.x
        } else {
            start.x
        });
        height = width / aspect;
        let available_height = if direction_y > 0. {
            bounds.height - start.y
        } else {
            start.y
        };
        if height > available_height {
            height = available_height;
            width = height * aspect;
        }
        end = Point {
            x: start.x + width * direction_x,
            y: start.y + height * direction_y,
        };
    }
    let x = start.x.min(end.x);
    let y = start.y.min(end.y);
    Rect {
        x: x.round(),
        y: y.round(),
        width: (end.x - start.x).abs().round().max(1.),
        height: (end.y - start.y).abs().round().max(1.),
    }
}

fn clamp(value: f64, minimum: f64, maximum: f64) -> f64 {
    value.max(minimum).min(maximum)
}

#[derive(Clone, Debug)]
pub struct DocumentHistory {
    current: Document,
    undo: VecDeque<Document>,
    redo: VecDeque<Document>,
}

impl DocumentHistory {
    #[must_use]
    pub fn new(current: Document) -> Self {
        Self {
            current,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn current(&self) -> &Document {
        &self.current
    }

    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Commit a document snapshot. Exact no-ops do not alter either stack.
    pub fn commit(&mut self, next: Document) -> bool {
        if self.current == next {
            return false;
        }
        push_back_bounded(&mut self.undo, self.current.clone());
        self.redo.clear();
        self.current = next;
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        push_front_bounded(&mut self.redo, self.current.clone());
        self.current = previous;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop_front() else {
            return false;
        };
        push_back_bounded(&mut self.undo, self.current.clone());
        self.current = next;
        true
    }
}

fn push_back_bounded(stack: &mut VecDeque<Document>, document: Document) {
    if stack.len() == HISTORY_LIMIT {
        stack.pop_front();
    }
    stack.push_back(document);
}

fn push_front_bounded(stack: &mut VecDeque<Document>, document: Document) {
    if stack.len() == HISTORY_LIMIT {
        stack.pop_back();
    }
    stack.push_front(document);
}
