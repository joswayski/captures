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
/// Intrinsic document-space cutoff below which the tapered-arrow renderer paints
/// no arrow. Hosts additionally own the shipping `3 / displayScale` gesture
/// threshold and cancel before submitting the completed command.
pub const ARROW_MIN_DRAW_LENGTH: f64 = 1.5;
pub const ALIGNMENT_SNAP_SCREEN_PX: f64 = 10.;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[repr(u32)]
#[serde(rename_all = "lowercase")]
pub enum ResizeHandle {
    Nw = 0,
    N = 1,
    Ne = 2,
    E = 3,
    Se = 4,
    S = 5,
    Sw = 6,
    W = 7,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GuideOrientation {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct AlignmentGuide {
    pub orientation: GuideOrientation,
    pub position: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ResizePreview {
    pub element: Element,
    pub outline: [Point; 4],
    pub guides: Vec<AlignmentGuide>,
}

#[derive(Clone, Debug)]
pub struct ResizeDrag {
    element: Element,
    initial_bounds: Rect,
    handle: ResizeHandle,
    display_scale: f64,
    vertical_lines: Vec<f64>,
    horizontal_lines: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MovePreview {
    pub element: Element,
    pub outline: [Point; 4],
    pub guides: Vec<AlignmentGuide>,
}

#[derive(Clone, Debug)]
pub struct MoveDrag {
    element: Element,
    initial_bounds: Rect,
    display_scale: f64,
    vertical_lines: Vec<f64>,
    horizontal_lines: Vec<f64>,
}

const fn default_opacity() -> f64 {
    100.
}

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

/// Transient geometry for one interactive crop drag.
///
/// Hosts own gesture lifetime and commit [`Self::rect`] separately through the
/// existing crop command. `preset_aspect` is a positive width/height ratio;
/// `None` (or an invalid ratio) means a free crop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropDrag {
    origin: Point,
    bounds: Rect,
    last_rect: Rect,
    latched_shift_aspect: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RotationHandle {
    pub anchor: Point,
    pub handle: Point,
    pub hit_radius: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RotationPreview {
    pub radians: f64,
    pub outline: [Point; 4],
}

/// Shipping angle normalization and optional 15-degree Shift stops. JS rounds
/// negative half-ties toward positive infinity, unlike Rust's f64::round.
pub fn rotation_angle(radians: f64, snap: bool) -> Option<f64> {
    if !radians.is_finite() {
        return None;
    }
    let mut angle = radians % std::f64::consts::TAU;
    if angle <= -std::f64::consts::PI {
        angle += std::f64::consts::TAU;
    }
    if angle > std::f64::consts::PI {
        angle -= std::f64::consts::TAU;
    }
    if angle.abs() < 1e-10 {
        angle = 0.;
    }
    if snap {
        let step = std::f64::consts::PI / 12.;
        return rotation_angle((angle / step + 0.5).floor() * step, false);
    }
    Some(angle)
}

fn rotate_point(point: Point, origin: Point, radians: f64) -> Point {
    if radians == 0. {
        return point;
    }
    let (sin, cos) = radians.sin_cos();
    let dx = point.x - origin.x;
    let dy = point.y - origin.y;
    Point {
        x: origin.x + dx * cos - dy * sin,
        y: origin.y + dx * sin + dy * cos,
    }
}

fn midpoint(first: Point, second: Point) -> Point {
    Point {
        x: first.x / 2. + second.x / 2.,
        y: first.y / 2. + second.y / 2.,
    }
}

fn finite_outline(outline: &[Point; 4]) -> bool {
    outline
        .iter()
        .all(|point| point.x.is_finite() && point.y.is_finite())
}

/// Prefer the outside top, outside bottom, inside top, then inside bottom grip.
/// Hide it when none fits the bitmap, matching shipping selection chrome.
pub fn rotation_handle(
    outline: [Point; 4],
    radians: f64,
    display_scale: f64,
    width: f64,
    height: f64,
) -> Option<RotationHandle> {
    if !finite_outline(&outline)
        || !radians.is_finite()
        || !display_scale.is_finite()
        || display_scale <= 0.
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.
        || height <= 0.
    {
        return None;
    }
    let scale = display_scale.max(0.01);
    let offset = 28. / scale;
    let pad = 8. / scale;
    let (sin, cos) = radians.sin_cos();
    let top = midpoint(outline[0], outline[1]);
    let bottom = midpoint(outline[2], outline[3]);
    [
        (top, -offset),
        (bottom, offset),
        (top, offset),
        (bottom, -offset),
    ]
    .into_iter()
    .find_map(|(anchor, offset)| {
        let handle = Point {
            x: anchor.x - sin * offset,
            y: anchor.y + cos * offset,
        };
        (handle.x >= pad && handle.y >= pad && handle.x <= width - pad && handle.y <= height - pad)
            .then_some(RotationHandle {
                anchor,
                handle,
                hit_radius: 12.5 / scale,
            })
    })
}

/// Stateless gesture preview from the original world outline and press point.
/// Hosts retain those inputs, including when Shift changes without pointer motion.
pub fn preview_rotation(
    outline: [Point; 4],
    initial: f64,
    start: Point,
    current: Point,
    snap: bool,
) -> Option<RotationPreview> {
    if !finite_outline(&outline)
        || ![initial, start.x, start.y, current.x, current.y]
            .into_iter()
            .all(f64::is_finite)
    {
        return None;
    }
    let origin = midpoint(outline[0], outline[2]);
    let angle = (current.y - origin.y).atan2(current.x - origin.x)
        - (start.y - origin.y).atan2(start.x - origin.x);
    let radians = rotation_angle(initial + angle, snap)?;
    let outline = outline.map(|point| rotate_point(point, origin, radians - initial));
    finite_outline(&outline).then_some(RotationPreview { radians, outline })
}

impl CropDrag {
    #[must_use]
    pub fn new(origin: Point, bounds: Rect, preset_aspect: Option<f64>, shift_held: bool) -> Self {
        let mut drag = Self {
            origin,
            bounds,
            last_rect: bounded_crop_rect(origin, origin, bounds, None),
            latched_shift_aspect: None,
        };
        drag.update(origin, preset_aspect, shift_held);
        drag
    }

    /// Updates the crop preview and returns its canvas-clamped rectangle.
    pub fn update(&mut self, current: Point, preset_aspect: Option<f64>, shift_held: bool) -> Rect {
        let preset_aspect = valid_aspect(preset_aspect);
        let aspect = if let Some(preset_aspect) = preset_aspect {
            self.latched_shift_aspect = None;
            Some(preset_aspect)
        } else if !shift_held {
            self.latched_shift_aspect = None;
            None
        } else {
            let aspect = self.latched_shift_aspect.unwrap_or_else(|| {
                crop_aspect_from_live_rect(self.last_rect)
                    .unwrap_or_else(|| shift_locked_crop_aspect(self.origin, current, self.bounds))
            });
            self.latched_shift_aspect = Some(aspect);
            Some(aspect)
        };
        self.last_rect = bounded_crop_rect(self.origin, current, self.bounds, aspect);
        self.last_rect
    }

    #[must_use]
    pub fn rect(&self) -> Rect {
        self.last_rect
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClosedShapeKind {
    Rectangle,
    Ellipse,
    Triangle,
    Diamond,
    Star,
}

impl ClosedShapeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rectangle => "rectangle",
            Self::Ellipse => "ellipse",
            Self::Triangle => "triangle",
            Self::Diamond => "diamond",
            Self::Star => "star",
        }
    }

    /// Shared polygon vertices for rendering and transient host previews.
    /// Curved shapes have no polygon. Keep the renderer's f32 arithmetic so a
    /// preview and its committed shape use exactly the same normalized geometry.
    pub fn polygon(self, start: Point, end: Point) -> Option<Vec<Point>> {
        let left = start.x.min(end.x) as f32;
        let top = start.y.min(end.y) as f32;
        let width = (end.x - start.x).abs() as f32;
        let height = (end.y - start.y).abs() as f32;
        let cx = left + width / 2.;
        let cy = top + height / 2.;
        let points = match self {
            Self::Rectangle | Self::Ellipse => return None,
            Self::Triangle => vec![
                (cx, top),
                (left + width, top + height),
                (left, top + height),
            ],
            Self::Diamond => vec![
                (cx, top),
                (left + width, cy),
                (cx, top + height),
                (left, cy),
            ],
            Self::Star => (0..10)
                .map(|index| {
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.;
                    let radius = if index % 2 == 0 { 1. } else { 0.39 };
                    (
                        cx + angle.cos() * width / 2. * radius,
                        cy + angle.sin() * height / 2. * radius,
                    )
                })
                .collect(),
        };
        Some(
            points
                .into_iter()
                .map(|(x, y)| Point {
                    x: x as f64,
                    y: y as f64,
                })
                .collect(),
        )
    }
}

/// Inputs for one completed closed-shape gesture. Hosts keep transient
/// pointer state outside the document and submit this value on completion.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClosedShapeCreate {
    pub shape: ClosedShapeKind,
    pub start: Point,
    pub end: Point,
    #[serde(default)]
    pub style: ElementStyle,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenShapeKind {
    Line,
    Arrow,
}

/// Inputs for one completed straight line or arrow gesture. Hosts keep pointer
/// state, cancellation, and screen-scale minimum gesture policy outside the
/// document and submit only the final signed endpoints.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenShapeCreate {
    pub shape: OpenShapeKind,
    pub start: Point,
    pub end: Point,
    #[serde(default)]
    pub style: ElementStyle,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
}

/// Inputs for one completed freehand gesture. Hosts own pointer sampling and
/// cancellation and submit the accepted document-space samples in order.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FreehandPathCreate {
    pub points: Vec<Point>,
    #[serde(default)]
    pub style: ElementStyle,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
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

    #[must_use]
    pub fn resolved_drop_shadow_style(&self) -> DropShadowStyle {
        let width = self.stroke_width.max(1.);
        let fallback = DropShadowStyle {
            color: "#000000".into(),
            opacity: 45.,
            blur: (width * 0.85).max(6.),
            offset_x: 0.,
            offset_y: (width * 0.32).round().max(2.),
            extra: Map::new(),
        };
        let Some(custom) = self.drop_shadow_style.as_ref() else {
            return fallback;
        };
        let number = |value: f64, minimum: f64, maximum: f64, fallback: f64| {
            if value.is_finite() {
                value.clamp(minimum, maximum)
            } else {
                fallback
            }
        };
        DropShadowStyle {
            color: resolved_shadow_color(&custom.color).unwrap_or(fallback.color),
            opacity: number(custom.opacity, 0., 100., fallback.opacity),
            blur: number(custom.blur, 0., 100., fallback.blur),
            offset_x: number(custom.offset_x, -500., 500., fallback.offset_x),
            offset_y: number(custom.offset_y, -500., 500., fallback.offset_y),
            extra: custom.extra.clone(),
        }
    }
}

impl Default for ElementStyle {
    fn default() -> Self {
        Self {
            color: "#ff3b5c".into(),
            fill: Some("#ff3b5c".into()),
            stroke_width: 8.,
            stroke_enabled: Some(false),
            drop_shadow: Some(false),
            drop_shadow_style: None,
            extra: Map::new(),
        }
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

/// Partial annotation-style update used by native property controls. Fill and
/// stroke-enabled changes apply only to closed shapes; all other fields apply
/// to shapes and freehand paths without replacing omitted or unknown data.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationStylePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub fill: OptionalNullable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_shadow: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_shadow_style: Option<DropShadowStylePatch>,
}

/// Partial shadow customization. Applying any setting enables the shadow and
/// resolves omitted values through the shipping renderer defaults.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropShadowStylePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_y: Option<f64>,
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

    /// Map document coordinates through layer rotation and D4 orientation into
    /// natural-resolution pixels. Right/bottom edges are exclusive, unlike
    /// selection chrome; hidden/locked policy belongs to the caller.
    #[must_use]
    pub fn natural_pixel_at(&self, point: Point) -> Option<(u32, u32)> {
        if ![
            point.x,
            point.y,
            self.width,
            self.height,
            self.natural_width,
            self.natural_height,
        ]
        .into_iter()
        .all(f64::is_finite)
            || self.width <= 0.
            || self.height <= 0.
            || self.natural_width < 1.
            || self.natural_height < 1.
        {
            return None;
        }
        let local = rotate_point(
            point,
            Point {
                x: self.base.x + self.width / 2.,
                y: self.base.y + self.height / 2.,
            },
            -self.base.rotation(),
        );
        let x = local.x - self.base.x;
        let y = local.y - self.base.y;
        if x < 0. || y < 0. || x >= self.width || y >= self.height {
            return None;
        }
        let matrix = self.resolved_orientation().matrix();
        let (width, height) = if matrix.a == 0 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let dx = x - self.width / 2.;
        let dy = y - self.height / 2.;
        let source_x = f64::from(matrix.a) * dx + f64::from(matrix.b) * dy;
        let source_y = f64::from(matrix.c) * dx + f64::from(matrix.d) * dy;
        Some((
            (((source_x + width / 2.) / width * self.natural_width).floor())
                .clamp(0., self.natural_width - 1.) as u32,
            (((source_y + height / 2.) / height * self.natural_height).floor())
                .clamp(0., self.natural_height - 1.) as u32,
        ))
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

    /// Shipping unrotated selection bounds, including annotation padding, not
    /// painted-pixel bounds. The center is also the element's rotation pivot.
    /// Text follows shipping's estimated wrapping, not measured glyph ink.
    pub fn selection_bounds(&self) -> Result<Rect, String> {
        match self {
            Self::Image(image) => Ok(Rect {
                x: image.base.x,
                y: image.base.y,
                width: image.width,
                height: image.height,
            }),
            Self::Text(text) => crate::editor_text::selection_bounds(text),
            Self::Shape(shape) => match shape.shape.as_str() {
                "rectangle" | "ellipse" | "triangle" | "diamond" | "star" => {
                    Ok(closed_shape_bounds(shape))
                }
                "line" | "arrow" => {
                    let shadow_pad = annotation_drop_shadow_pad(&shape.style);
                    if shape.shape == "arrow" {
                        let polygon = arrow_fill_polygon(shape);
                        if polygon.len() >= 3 {
                            return Ok(bounds_from_points(&polygon, 1. + shadow_pad));
                        }
                    }
                    let vertices = std::iter::once(Point {
                        x: shape.base.x,
                        y: shape.base.y,
                    })
                    .chain(shape.controls.iter().copied())
                    .chain(std::iter::once(Point {
                        x: shape.end_x,
                        y: shape.end_y,
                    }))
                    .collect::<Vec<_>>();
                    let padding =
                        ((shape.style.stroke_width / 2.).ceil() + 1.).max(1.) + shadow_pad;
                    Ok(bounds_from_points(
                        &sample_controlled_path(&vertices, 48),
                        padding,
                    ))
                }
                _ => Err("Unsupported shape selection geometry.".into()),
            },
            Self::Path(path) if path.points.is_empty() => Ok(Rect {
                x: path.base.x,
                y: path.base.y,
                width: 1.,
                height: 1.,
            }),
            Self::Path(path) => Ok(freehand_path_bounds(path)),
        }
    }

    /// Document-space corners for native selection/move feedback. Hosts only
    /// translate/project these points; picking and rotation remain shared.
    pub fn selection_outline(&self) -> Result<[Point; 4], String> {
        let bounds = self.selection_bounds()?;
        let mut points = [
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
        ];
        let rotation = self.base().rotation();
        if rotation != 0. {
            let center = Point {
                x: bounds.x + bounds.width / 2.,
                y: bounds.y + bounds.height / 2.,
            };
            let (sin, cos) = rotation.sin_cos();
            for point in &mut points {
                let dx = point.x - center.x;
                let dy = point.y - center.y;
                *point = Point {
                    x: center.x + dx * cos - dy * sin,
                    y: center.y + dx * sin + dy * cos,
                };
            }
        }
        Ok(points)
    }

    /// Hit-test resize chrome in the element's unrotated local selection space.
    pub fn resize_handle_at(
        &self,
        point: Point,
        radius: f64,
    ) -> Result<Option<ResizeHandle>, String> {
        if !point.x.is_finite() || !point.y.is_finite() || !radius.is_finite() || radius < 0. {
            return Err(
                "Resize hit testing requires finite coordinates and nonnegative radius.".into(),
            );
        }
        let bounds = self.selection_bounds()?;
        let center = rect_center(bounds);
        let local = rotate_point(point, center, -self.base().rotation());
        Ok(hit_test_resize_handle(bounds, local, radius))
    }

    fn base_mut(&mut self) -> &mut ElementBase {
        match self {
            Self::Image(element) => &mut element.base,
            Self::Text(element) => &mut element.base,
            Self::Shape(element) => &mut element.base,
            Self::Path(element) => &mut element.base,
        }
    }

    fn translate_layer(&mut self, delta_x: f64, delta_y: f64) -> Result<(), String> {
        // Hidden layers also need serializable geometry: the renderer skips them,
        // but a saved draft must still be readable after any accepted movement.
        let finite = |x: f64, y: f64| (x + delta_x).is_finite() && (y + delta_y).is_finite();
        let points_finite = |points: &[Point]| points.iter().all(|point| finite(point.x, point.y));
        let base = self.base();
        let valid = finite(base.x, base.y)
            && match self {
                Self::Image(_) | Self::Text(_) => true,
                Self::Shape(element) => {
                    finite(element.end_x, element.end_y) && points_finite(&element.controls)
                }
                Self::Path(element) => points_finite(&element.points),
            };
        if !valid {
            return Err("Layer movement must keep geometry finite.".into());
        }
        self.translate(delta_x, delta_y);
        Ok(())
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

impl ResizeDrag {
    pub fn new(
        document: &Document,
        id: &str,
        handle: ResizeHandle,
        display_scale: f64,
    ) -> Result<Self, String> {
        if !display_scale.is_finite() || display_scale <= 0. {
            return Err("Resize display scale must be finite and positive.".into());
        }
        let element = document
            .elements
            .iter()
            .find(|element| element.base().id == id)
            .ok_or("The selected layer no longer exists.")?
            .clone();
        let initial_bounds = element.selection_bounds()?;
        let (vertical_lines, horizontal_lines) = collect_alignment_lines(document, id)?;
        Ok(Self {
            element,
            initial_bounds,
            handle,
            display_scale,
            vertical_lines,
            horizontal_lines,
        })
    }

    pub fn preview(&self, current: Point, lock_aspect: bool) -> Result<ResizePreview, String> {
        if !current.x.is_finite() || !current.y.is_finite() {
            return Err("Resize coordinates must be finite.".into());
        }
        let minimum = 8. / self.display_scale.max(0.01);
        let threshold = ALIGNMENT_SNAP_SCREEN_PX / self.display_scale.max(0.01);
        let center = rect_center(self.initial_bounds);
        let local_current = rotate_point(current, center, -self.element.base().rotation());
        let free = resize_bounds_from_handle(
            self.initial_bounds,
            self.handle,
            local_current,
            minimum,
            lock_aspect,
        );
        let (snapped, guides) = if self.element.base().rotation() == 0. {
            snap_resized_bounds(
                self.initial_bounds,
                self.handle,
                free,
                &self.vertical_lines,
                &self.horizontal_lines,
                threshold,
                minimum,
            )
        } else {
            (free, Vec::new())
        };
        let next_bounds = if lock_aspect {
            resize_bounds_from_handle(
                self.initial_bounds,
                self.handle,
                resize_handle_point(snapped, self.handle),
                minimum,
                true,
            )
        } else {
            snapped
        };
        let mut element = resize_element(&self.element, self.initial_bounds, next_bounds)?;
        if element.base().rotation() != 0. {
            let anchor =
                resize_handle_point(self.initial_bounds, opposite_resize_handle(self.handle));
            preserve_element_world_point(&self.element, &mut element, anchor)?;
        }
        let outline = element.selection_outline()?;
        if !finite_outline(&outline) {
            return Err("Resize must keep geometry finite.".into());
        }
        Ok(ResizePreview {
            element,
            outline,
            guides,
        })
    }
}

impl MoveDrag {
    pub fn new(document: &Document, id: &str, display_scale: f64) -> Result<Self, String> {
        if !display_scale.is_finite() || display_scale <= 0. {
            return Err("Move display scale must be finite and positive.".into());
        }
        let element = document
            .elements
            .iter()
            .find(|element| element.base().id == id)
            .ok_or("The selected layer no longer exists.")?
            .clone();
        let initial_bounds = painted_bounds(&element)?;
        let (vertical_lines, horizontal_lines) = collect_alignment_lines(document, id)?;
        Ok(Self {
            element,
            initial_bounds,
            display_scale,
            vertical_lines,
            horizontal_lines,
        })
    }

    pub fn preview(&self, delta: Point) -> Result<MovePreview, String> {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return Err("Move delta must be finite.".into());
        }
        let free = Rect {
            x: self.initial_bounds.x + delta.x,
            y: self.initial_bounds.y + delta.y,
            ..self.initial_bounds
        };
        let threshold = ALIGNMENT_SNAP_SCREEN_PX / self.display_scale.max(0.01);
        let (snapped, guides) = snap_translated_bounds(
            free,
            &self.vertical_lines,
            &self.horizontal_lines,
            threshold,
        );
        let mut element = self.element.clone();
        element.translate_layer(delta.x + snapped.x - free.x, delta.y + snapped.y - free.y)?;
        let outline = element.selection_outline()?;
        if !finite_outline(&outline) {
            return Err("Move must keep geometry finite.".into());
        }
        Ok(MovePreview {
            element,
            outline,
            guides,
        })
    }
}

fn collect_alignment_lines(
    document: &Document,
    exclude_id: &str,
) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut vertical = vec![0., document.width];
    let mut horizontal = vec![0., document.height];
    for element in &document.elements {
        if element.base().id == exclude_id || !element.base().visible {
            continue;
        }
        let bounds = painted_bounds(element)?;
        push_unique_number(&mut vertical, bounds.x);
        push_unique_number(&mut vertical, bounds.x + bounds.width);
        push_unique_number(&mut horizontal, bounds.y);
        push_unique_number(&mut horizontal, bounds.y + bounds.height);
    }
    Ok((vertical, horizontal))
}

fn push_unique_number(values: &mut Vec<f64>, value: f64) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn rect_center(bounds: Rect) -> Point {
    Point {
        x: bounds.x + bounds.width / 2.,
        y: bounds.y + bounds.height / 2.,
    }
}

fn resize_handle_point(bounds: Rect, handle: ResizeHandle) -> Point {
    let center = rect_center(bounds);
    match handle {
        ResizeHandle::Nw => Point {
            x: bounds.x,
            y: bounds.y,
        },
        ResizeHandle::N => Point {
            x: center.x,
            y: bounds.y,
        },
        ResizeHandle::Ne => Point {
            x: bounds.x + bounds.width,
            y: bounds.y,
        },
        ResizeHandle::E => Point {
            x: bounds.x + bounds.width,
            y: center.y,
        },
        ResizeHandle::Se => Point {
            x: bounds.x + bounds.width,
            y: bounds.y + bounds.height,
        },
        ResizeHandle::S => Point {
            x: center.x,
            y: bounds.y + bounds.height,
        },
        ResizeHandle::Sw => Point {
            x: bounds.x,
            y: bounds.y + bounds.height,
        },
        ResizeHandle::W => Point {
            x: bounds.x,
            y: center.y,
        },
    }
}

fn opposite_resize_handle(handle: ResizeHandle) -> ResizeHandle {
    match handle {
        ResizeHandle::Nw => ResizeHandle::Se,
        ResizeHandle::N => ResizeHandle::S,
        ResizeHandle::Ne => ResizeHandle::Sw,
        ResizeHandle::E => ResizeHandle::W,
        ResizeHandle::Se => ResizeHandle::Nw,
        ResizeHandle::S => ResizeHandle::N,
        ResizeHandle::Sw => ResizeHandle::Ne,
        ResizeHandle::W => ResizeHandle::E,
    }
}

fn is_corner(handle: ResizeHandle) -> bool {
    matches!(
        handle,
        ResizeHandle::Nw | ResizeHandle::Ne | ResizeHandle::Se | ResizeHandle::Sw
    )
}

fn hit_test_resize_handle(bounds: Rect, point: Point, radius: f64) -> Option<ResizeHandle> {
    let radius = radius.max(4.);
    for handle in [
        ResizeHandle::Nw,
        ResizeHandle::Ne,
        ResizeHandle::Se,
        ResizeHandle::Sw,
    ] {
        let grip = resize_handle_point(bounds, handle);
        if (point.x - grip.x).abs() <= radius && (point.y - grip.y).abs() <= radius {
            return Some(handle);
        }
    }
    for handle in [
        ResizeHandle::N,
        ResizeHandle::E,
        ResizeHandle::S,
        ResizeHandle::W,
    ] {
        let grip = resize_handle_point(bounds, handle);
        if (point.x - grip.x).abs() <= radius && (point.y - grip.y).abs() <= radius {
            return Some(handle);
        }
    }
    let right = bounds.x + bounds.width;
    let bottom = bounds.y + bounds.height;
    if point.x < bounds.x - radius
        || point.x > right + radius
        || point.y < bounds.y - radius
        || point.y > bottom + radius
    {
        return None;
    }
    let near_left = (point.x - bounds.x).abs() <= radius;
    let near_right = (point.x - right).abs() <= radius;
    let near_top = (point.y - bounds.y).abs() <= radius;
    let near_bottom = (point.y - bottom).abs() <= radius;
    if near_top && !near_left && !near_right {
        Some(ResizeHandle::N)
    } else if near_bottom && !near_left && !near_right {
        Some(ResizeHandle::S)
    } else if near_left && !near_top && !near_bottom {
        Some(ResizeHandle::W)
    } else if near_right && !near_top && !near_bottom {
        Some(ResizeHandle::E)
    } else {
        None
    }
}

/// Layer-panel actions shared by native hosts. Storage is back-to-front;
/// placement refers to the front-to-back order displayed by the layer panel.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum LayerEdit {
    Visibility {
        visible: bool,
    },
    Lock {
        locked: bool,
    },
    ImageTransform {
        transform: ImageTransform,
    },
    Opacity {
        opacity: f64,
    },
    AnnotationStyle {
        patch: AnnotationStylePatch,
    },
    Translate {
        delta_x: f64,
        delta_y: f64,
    },
    DragMove {
        delta_x: f64,
        delta_y: f64,
        display_scale: f64,
    },
    Rotate {
        radians: f64,
    },
    Resize {
        handle: ResizeHandle,
        current: Point,
        display_scale: f64,
        lock_aspect: bool,
    },
    Delete,
    Duplicate {
        new_id: String,
    },
    Reorder {
        target_id: String,
        placement: LayerPlacement,
    },
    Rename {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerPlacement {
    Before,
    After,
}

impl Document {
    /// Picks the frontmost visible, unlocked layer using the shipping editor's
    /// rotated local bounding box. Opacity and transparent pixels do not affect
    /// picking. Coordinates and nonnegative tolerance are in document pixels;
    /// hosts convert pointer/zoom coordinates and own selection/gesture state.
    ///
    /// A visible, unlocked unsupported element encountered before a hit returns
    /// an error rather than silently selecting through it. Hidden/locked layers
    /// do not block picking. This query never edits the document or history.
    pub fn hit_test(&self, point: Point, tolerance: f64) -> Result<Option<&Element>, String> {
        if !point.x.is_finite() || !point.y.is_finite() || !tolerance.is_finite() || tolerance < 0.
        {
            return Err(
                "Hit testing requires finite coordinates and nonnegative tolerance.".into(),
            );
        }
        for element in self.elements.iter().rev() {
            let base = element.base();
            if !base.visible || base.locked {
                continue;
            }
            let bounds = element.selection_bounds()?;
            let Point { mut x, mut y } = point;
            // Match the shipping zero-angle shortcut: subtracting/adding the
            // pivot can otherwise move an exact fractional edge outside.
            if base.rotation() != 0. {
                let center_x = bounds.x + bounds.width / 2.;
                let center_y = bounds.y + bounds.height / 2.;
                let (sin, cos) = (-base.rotation()).sin_cos();
                let delta_x = point.x - center_x;
                let delta_y = point.y - center_y;
                x = center_x + delta_x * cos - delta_y * sin;
                y = center_y + delta_x * sin + delta_y * cos;
            }
            if x >= bounds.x - tolerance
                && x <= bounds.x + bounds.width + tolerance
                && y >= bounds.y - tolerance
                && y <= bounds.y + bounds.height + tolerance
            {
                return Ok(Some(element));
            }
        }
        Ok(None)
    }

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

    /// Fit the canvas to visible layer geometry, including locked/transparent
    /// layers and off-canvas content. This does not scan flattened alpha pixels.
    pub fn trim_to_content(&mut self) -> Result<(), String> {
        let mut content: Option<Rect> = None;
        for element in self
            .elements
            .iter()
            .filter(|element| element.base().visible)
        {
            let bounds = painted_bounds(element)?;
            content = Some(match content {
                None => bounds,
                Some(previous) => {
                    let x = previous.x.min(bounds.x);
                    let y = previous.y.min(bounds.y);
                    Rect {
                        x,
                        y,
                        width: (previous.x + previous.width).max(bounds.x + bounds.width) - x,
                        height: (previous.y + previous.height).max(bounds.y + bounds.height) - y,
                    }
                }
            });
        }
        if let Some(mut bounds) = content {
            bounds.width = bounds.width.max(1.);
            bounds.height = bounds.height.max(1.);
            self.fit_canvas_to_bounds(bounds);
        }
        Ok(())
    }

    /// Append one completed closed shape using the shipping editor's
    /// layer defaults and fully-outside canvas expansion policy.
    pub fn create_closed_shape(&mut self, create: ClosedShapeCreate) -> Result<String, String> {
        if !create.start.x.is_finite()
            || !create.start.y.is_finite()
            || !create.end.x.is_finite()
            || !create.end.y.is_finite()
        {
            return Err("Shape coordinates must be finite.".into());
        }
        if !create.opacity.is_finite() || !(0. ..=100.).contains(&create.opacity) {
            return Err("Shape opacity must be between 0 and 100.".into());
        }
        if create.start.x == create.end.x || create.start.y == create.end.y {
            return Err("Closed shapes must have positive width and height.".into());
        }
        if !create.style.stroke_width.is_finite() {
            return Err("Shape stroke width must be finite.".into());
        }

        let id = loop {
            let candidate = uuid::Uuid::new_v4().to_string();
            if !self
                .elements
                .iter()
                .any(|element| element.base().id == candidate)
            {
                break candidate;
            }
        };
        let shape = create.shape.as_str();
        let element = ShapeElement {
            base: ElementBase {
                id: id.clone(),
                x: create.start.x,
                y: create.start.y,
                rotation: None,
                locked: false,
                visible: true,
                opacity: create.opacity,
                blend_mode: "source-over".into(),
            },
            shape: shape.into(),
            end_x: create.end.x,
            end_y: create.end.y,
            controls: Vec::new(),
            style: create.style,
            extra: Map::new(),
        };
        let bounds = closed_shape_bounds(&element);
        self.elements.push(Element::Shape(element));
        if fully_outside_canvas(bounds, self.width, self.height) {
            self.expand_canvas_to_bounds(bounds);
        }
        Ok(id)
    }

    /// Append one completed straight line or arrow using the shipping editor's
    /// layer defaults and fully-outside painted-bounds expansion policy.
    pub fn create_open_shape(&mut self, create: OpenShapeCreate) -> Result<String, String> {
        if !create.start.x.is_finite()
            || !create.start.y.is_finite()
            || !create.end.x.is_finite()
            || !create.end.y.is_finite()
        {
            return Err("Shape coordinates must be finite.".into());
        }
        if !create.opacity.is_finite() || !(0. ..=100.).contains(&create.opacity) {
            return Err("Shape opacity must be between 0 and 100.".into());
        }
        if !create.style.stroke_width.is_finite() {
            return Err("Shape stroke width must be finite.".into());
        }
        let length = (create.end.x - create.start.x).hypot(create.end.y - create.start.y);
        if create.shape == OpenShapeKind::Arrow && length < ARROW_MIN_DRAW_LENGTH {
            return Err(format!(
                "Arrows must be at least {ARROW_MIN_DRAW_LENGTH} document pixels long."
            ));
        }

        let id = loop {
            let candidate = uuid::Uuid::new_v4().to_string();
            if !self
                .elements
                .iter()
                .any(|element| element.base().id == candidate)
            {
                break candidate;
            }
        };
        let mut style = create.style;
        style.fill = None;
        let element = ShapeElement {
            base: ElementBase {
                id: id.clone(),
                x: create.start.x,
                y: create.start.y,
                rotation: None,
                locked: false,
                visible: true,
                opacity: create.opacity,
                blend_mode: "source-over".into(),
            },
            shape: match create.shape {
                OpenShapeKind::Line => "line",
                OpenShapeKind::Arrow => "arrow",
            }
            .into(),
            end_x: create.end.x,
            end_y: create.end.y,
            controls: Vec::new(),
            style,
            extra: Map::new(),
        };
        let bounds = open_shape_bounds(&element);
        self.elements.push(Element::Shape(element));
        if fully_outside_canvas(bounds, self.width, self.height) {
            self.expand_canvas_to_bounds(bounds);
        }
        Ok(id)
    }

    /// Append one completed freehand path using the shipping editor's layer
    /// defaults and fully-outside painted-bounds expansion policy.
    pub fn create_freehand_path(&mut self, create: FreehandPathCreate) -> Result<String, String> {
        let Some(first) = create.points.first().copied() else {
            return Err("Freehand paths require at least one point.".into());
        };
        if !create.points.iter().all(|point| {
            point.x.is_finite()
                && point.y.is_finite()
                && (point.x as f32).is_finite()
                && (point.y as f32).is_finite()
        }) {
            return Err("Freehand path coordinates must be finite renderer values.".into());
        }
        if !create.opacity.is_finite() || !(0. ..=100.).contains(&create.opacity) {
            return Err("Path opacity must be between 0 and 100.".into());
        }
        if !create.style.stroke_width.is_finite()
            || !(create.style.stroke_width as f32).is_finite()
            || create.style.stroke_width <= 0.
        {
            return Err("Path stroke width must be finite and positive.".into());
        }

        let id = loop {
            let candidate = uuid::Uuid::new_v4().to_string();
            if !self
                .elements
                .iter()
                .any(|element| element.base().id == candidate)
            {
                break candidate;
            }
        };
        let mut style = create.style;
        style.fill = None;
        let element = PathElement {
            base: ElementBase {
                id: id.clone(),
                x: first.x,
                y: first.y,
                rotation: None,
                locked: false,
                visible: true,
                opacity: create.opacity,
                blend_mode: "source-over".into(),
            },
            points: create.points,
            style,
            extra: Map::new(),
        };
        let bounds = freehand_path_bounds(&element);
        self.elements.push(Element::Path(element));
        if fully_outside_canvas(bounds, self.width, self.height) {
            self.expand_canvas_to_bounds(bounds);
        }
        Ok(id)
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

    /// Match the shipping layer panel: locking prevents deletion, nudging and
    /// reordering, but not image transforms, visibility, opacity, rename or
    /// duplication. Hidden layers remain editable from the panel. Rejected
    /// requests do not mutate.
    pub fn edit_layer(&mut self, id: &str, edit: LayerEdit) -> Result<(), String> {
        let Some(index) = self
            .elements
            .iter()
            .position(|element| element.base().id == id)
        else {
            return Err("The selected layer no longer exists.".into());
        };
        let locked = self.elements[index].base().locked;
        match edit {
            LayerEdit::Visibility { visible } => self.elements[index].base_mut().visible = visible,
            LayerEdit::Lock { locked } => self.elements[index].base_mut().locked = locked,
            LayerEdit::ImageTransform { transform } => {
                let Element::Image(image) = &self.elements[index] else {
                    return Ok(());
                };
                let rotates = matches!(
                    transform,
                    ImageTransform::RotateClockwise | ImageTransform::RotateCounterclockwise
                );
                let fills_canvas = image.base.visible
                    && self
                        .elements
                        .iter()
                        .filter(|element| element.base().visible)
                        .count()
                        == 1
                    && image.base.x.abs() < 0.01
                    && image.base.y.abs() < 0.01
                    && (image.width - self.width).abs() < 0.01
                    && (image.height - self.height).abs() < 0.01;
                let Element::Image(image) = &mut self.elements[index] else {
                    unreachable!()
                };
                image.transform(transform);
                let bounds = image_bounds(image);

                // Match the shipping action rather than only its D4 primitive:
                // a fresh full-canvas photo rotates the canvas with the bitmap,
                // layered overhang remains clipped, and a fully lost layer grows
                // the canvas back around itself.
                if rotates && fills_canvas {
                    self.fit_canvas_to_bounds(bounds);
                } else if fully_outside_canvas(bounds, self.width, self.height) {
                    self.expand_canvas_to_bounds(bounds);
                }
            }
            LayerEdit::Opacity { opacity } => {
                if !opacity.is_finite() || !(0. ..=100.).contains(&opacity) {
                    return Err("Layer opacity must be between 0 and 100.".into());
                }
                self.elements[index].base_mut().opacity = opacity;
            }
            LayerEdit::AnnotationStyle { patch } => {
                let (style, closed) = match &mut self.elements[index] {
                    Element::Shape(element) => (
                        &mut element.style,
                        matches!(
                            element.shape.as_str(),
                            "rectangle" | "ellipse" | "triangle" | "diamond" | "star"
                        ),
                    ),
                    Element::Path(element) => (&mut element.style, false),
                    Element::Image(_) | Element::Text(_) => return Ok(()),
                };
                patch.apply(style, closed)?;
            }
            LayerEdit::Translate { delta_x, delta_y } => {
                if !delta_x.is_finite() || !delta_y.is_finite() {
                    return Err("Layer movement must be finite.".into());
                }
                if !locked {
                    self.elements[index].translate_layer(delta_x, delta_y)?;
                }
            }
            LayerEdit::DragMove {
                delta_x,
                delta_y,
                display_scale,
            } => {
                if !delta_x.is_finite() || !delta_y.is_finite() {
                    return Err("Move delta must be finite.".into());
                }
                if !display_scale.is_finite() || display_scale <= 0. {
                    return Err("Move display scale must be finite and positive.".into());
                }
                if !locked {
                    let preview = MoveDrag::new(self, id, display_scale)?.preview(Point {
                        x: delta_x,
                        y: delta_y,
                    })?;
                    let bounds = painted_bounds(&preview.element)?;
                    self.elements[index] = preview.element;
                    if fully_outside_canvas(bounds, self.width, self.height) {
                        self.expand_canvas_to_bounds(bounds);
                    }
                }
            }
            LayerEdit::Rotate { radians } => {
                let radians =
                    rotation_angle(radians, false).ok_or("Layer rotation must be finite.")?;
                if !locked {
                    self.elements[index].selection_bounds()?;
                    self.elements[index].base_mut().rotation = (radians != 0.).then_some(radians);
                    let bounds = painted_bounds(&self.elements[index])?;
                    if fully_outside_canvas(bounds, self.width, self.height) {
                        self.expand_canvas_to_bounds(bounds);
                    }
                }
            }
            LayerEdit::Resize {
                handle,
                current,
                display_scale,
                lock_aspect,
            } => {
                if !current.x.is_finite() || !current.y.is_finite() {
                    return Err("Resize coordinates must be finite.".into());
                }
                if !display_scale.is_finite() || display_scale <= 0. {
                    return Err("Resize display scale must be finite and positive.".into());
                }
                if !locked {
                    let drag = ResizeDrag::new(self, id, handle, display_scale)?;
                    let preview = drag.preview(current, lock_aspect)?;
                    let mut next = self.clone();
                    next.elements[index] = preview.element;
                    let bounds = painted_bounds(&next.elements[index])?;
                    if fully_outside_canvas(bounds, next.width, next.height) {
                        next.expand_canvas_to_bounds(bounds);
                    }
                    *self = next;
                }
            }
            LayerEdit::Delete => {
                if !locked {
                    self.elements.remove(index);
                }
            }
            LayerEdit::Duplicate { new_id } => {
                if new_id.is_empty()
                    || self
                        .elements
                        .iter()
                        .any(|element| element.base().id == new_id)
                {
                    return Err("A duplicate layer needs a new nonempty identifier.".into());
                }
                let mut duplicate = self.elements[index].clone();
                let base = duplicate.base_mut();
                base.id = new_id;
                base.locked = false;
                base.visible = true;
                if let Element::Image(image) = &mut duplicate {
                    image.source = "imported".into();
                    image.name.push_str(" copy");
                }
                duplicate.translate_layer(24., 24.)?;
                self.elements.insert(index + 1, duplicate);
            }
            LayerEdit::Reorder {
                target_id,
                placement,
            } => {
                if locked || target_id == id {
                    return Ok(());
                }
                let Some(target) = self
                    .elements
                    .iter()
                    .position(|element| element.base().id == target_id)
                else {
                    return Err("The target layer no longer exists.".into());
                };
                let minimum = self.elements[..index]
                    .iter()
                    .rposition(|element| element.base().locked)
                    .map_or(0, |position| position + 1);
                let maximum = self.elements[index + 1..]
                    .iter()
                    .position(|element| element.base().locked)
                    .map_or(self.elements.len() - 1, |position| index + position);
                let target_after_removal = target - usize::from(target > index);
                let desired =
                    target_after_removal + usize::from(matches!(placement, LayerPlacement::Before));
                let destination = desired.clamp(minimum, maximum);
                let moved = self.elements.remove(index);
                self.elements.insert(destination, moved);
            }
            LayerEdit::Rename { name } => {
                if let Element::Image(image) = &mut self.elements[index] {
                    let name = name.trim();
                    if !name.is_empty() {
                        image.name = name.into();
                    }
                }
            }
        }
        Ok(())
    }

    fn fit_canvas_to_bounds(&mut self, bounds: Rect) {
        let x = bounds.x.floor();
        let y = bounds.y.floor();
        let right = (bounds.x + bounds.width).ceil();
        let bottom = (bounds.y + bounds.height).ceil();
        self.width = (right - x).max(1.);
        self.height = (bottom - y).max(1.);
        self.translate(-x, -y);
    }

    fn expand_canvas_to_bounds(&mut self, bounds: Rect) {
        let shift_x = (-bounds.x).ceil().max(0.);
        let shift_y = (-bounds.y).ceil().max(0.);
        let fitted_x = bounds.x + shift_x;
        let fitted_y = bounds.y + shift_y;
        self.width = (self.width + shift_x).max((fitted_x + bounds.width).ceil());
        self.height = (self.height + shift_y).max((fitted_y + bounds.height).ceil());
        self.translate(shift_x, shift_y);
    }
}

fn closed_shape_bounds(shape: &ShapeElement) -> Rect {
    let left = shape.base.x.min(shape.end_x);
    let top = shape.base.y.min(shape.end_y);
    let stroke_extent = ((shape.style.stroke_width / 2.).ceil() + 1.).max(1.)
        + annotation_drop_shadow_pad(&shape.style);
    Rect {
        x: left - stroke_extent,
        y: top - stroke_extent,
        width: (shape.base.x - shape.end_x).abs().max(1.) + stroke_extent * 2.,
        height: (shape.base.y - shape.end_y).abs().max(1.) + stroke_extent * 2.,
    }
}

fn open_shape_bounds(shape: &ShapeElement) -> Rect {
    let shadow_pad = annotation_drop_shadow_pad(&shape.style);
    if shape.shape == "arrow" {
        let polygon = arrow_fill_polygon(shape);
        if polygon.len() >= 3 {
            return bounds_from_points(&polygon, 1. + shadow_pad);
        }
    }
    let stroke_extent = ((shape.style.stroke_width / 2.).ceil() + 1.).max(1.) + shadow_pad;
    bounds_from_points(
        &[
            Point {
                x: shape.base.x,
                y: shape.base.y,
            },
            Point {
                x: shape.end_x,
                y: shape.end_y,
            },
        ],
        stroke_extent,
    )
}

fn freehand_path_bounds(path: &PathElement) -> Rect {
    let padding = path.style.stroke_width.max(4.) + annotation_drop_shadow_pad(&path.style);
    bounds_from_points(&path.points, padding)
}

fn bounds_from_points(points: &[Point], padding: f64) -> Rect {
    let first = points[0];
    let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
    for point in &points[1..] {
        left = left.min(point.x);
        top = top.min(point.y);
        right = right.max(point.x);
        bottom = bottom.max(point.y);
    }
    let padding = padding.max(0.);
    Rect {
        x: left - padding,
        y: top - padding,
        width: (right - left).max(1.) + padding * 2.,
        height: (bottom - top).max(1.) + padding * 2.,
    }
}

fn resize_bounds_from_handle(
    initial: Rect,
    handle: ResizeHandle,
    current: Point,
    minimum: f64,
    lock_aspect: bool,
) -> Rect {
    let min = minimum.max(1.);
    if lock_aspect && is_corner(handle) && initial.width >= 1. && initial.height >= 1. {
        let aspect = initial.width / initial.height;
        let anchor = resize_handle_point(initial, opposite_resize_handle(handle));
        let mut width = (current.x - anchor.x).abs().max(1e-6);
        let mut height = (current.y - anchor.y).abs().max(1e-6);
        if width / height > aspect {
            height = width / aspect;
        } else {
            width = height * aspect;
        }
        if width < min || height < min {
            if aspect >= 1. {
                width = width.max(min);
                height = width / aspect;
                if height < min {
                    height = min;
                    width = height * aspect;
                }
            } else {
                height = height.max(min);
                width = height * aspect;
                if width < min {
                    width = min;
                    height = width / aspect;
                }
            }
        }
        return Rect {
            x: if current.x >= anchor.x {
                anchor.x
            } else {
                anchor.x - width
            },
            y: if current.y >= anchor.y {
                anchor.y
            } else {
                anchor.y - height
            },
            width,
            height,
        };
    }
    let mut left = if matches!(
        handle,
        ResizeHandle::W | ResizeHandle::Nw | ResizeHandle::Sw
    ) {
        current.x
    } else {
        initial.x
    };
    let mut right = if matches!(
        handle,
        ResizeHandle::E | ResizeHandle::Ne | ResizeHandle::Se
    ) {
        current.x
    } else {
        initial.x + initial.width
    };
    let mut top = if matches!(
        handle,
        ResizeHandle::N | ResizeHandle::Nw | ResizeHandle::Ne
    ) {
        current.y
    } else {
        initial.y
    };
    let mut bottom = if matches!(
        handle,
        ResizeHandle::S | ResizeHandle::Sw | ResizeHandle::Se
    ) {
        current.y
    } else {
        initial.y + initial.height
    };
    if left > right {
        std::mem::swap(&mut left, &mut right);
    }
    if top > bottom {
        std::mem::swap(&mut top, &mut bottom);
    }
    let move_left = matches!(
        handle,
        ResizeHandle::W | ResizeHandle::Nw | ResizeHandle::Sw
    );
    let move_right = matches!(
        handle,
        ResizeHandle::E | ResizeHandle::Ne | ResizeHandle::Se
    );
    let move_top = matches!(
        handle,
        ResizeHandle::N | ResizeHandle::Nw | ResizeHandle::Ne
    );
    let move_bottom = matches!(
        handle,
        ResizeHandle::S | ResizeHandle::Sw | ResizeHandle::Se
    );
    if right - left < min {
        if move_left && !move_right {
            left = right - min;
        } else if move_right && !move_left {
            right = left + min;
        } else {
            let center = (left + right) / 2.;
            left = center - min / 2.;
            right = center + min / 2.;
        }
    }
    if bottom - top < min {
        if move_top && !move_bottom {
            top = bottom - min;
        } else if move_bottom && !move_top {
            bottom = top + min;
        } else {
            let center = (top + bottom) / 2.;
            top = center - min / 2.;
            bottom = center + min / 2.;
        }
    }
    Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

fn closest_snap(value: f64, lines: &[f64], threshold: f64) -> Option<f64> {
    let mut best = None;
    let mut distance = threshold;
    for &line in lines {
        let next = (line - value).abs();
        if next <= distance {
            distance = next;
            best = Some(line);
        }
    }
    best
}

fn snap_translated_bounds(
    bounds: Rect,
    vertical: &[f64],
    horizontal: &[f64],
    threshold: f64,
) -> (Rect, Vec<AlignmentGuide>) {
    if threshold <= 0. {
        return (bounds, Vec::new());
    }
    let pick_axis = |edges: [f64; 2], lines: &[f64]| {
        let mut best = None;
        let mut best_abs = threshold;
        for edge in edges {
            if let Some(position) = closest_snap(edge, lines, threshold) {
                let delta = position - edge;
                let distance = delta.abs();
                if distance < best_abs - 1e-9 {
                    best_abs = distance;
                    best = Some(delta);
                }
            }
        }
        best
    };
    let delta_x = pick_axis([bounds.x, bounds.x + bounds.width], vertical);
    let delta_y = pick_axis([bounds.y, bounds.y + bounds.height], horizontal);
    let next = Rect {
        x: bounds.x + delta_x.unwrap_or(0.),
        y: bounds.y + delta_y.unwrap_or(0.),
        ..bounds
    };
    let mut guides = Vec::new();
    let mut push_guide = |orientation: GuideOrientation, position: f64| {
        if !guides.iter().any(|guide: &AlignmentGuide| {
            guide.orientation == orientation && (guide.position - position).abs() <= 1e-6
        }) {
            guides.push(AlignmentGuide {
                orientation,
                position,
            });
        }
    };
    if delta_x.is_some() {
        for edge in [next.x, next.x + next.width] {
            if vertical.iter().any(|line| (line - edge).abs() <= 1e-6) {
                push_guide(GuideOrientation::Vertical, edge);
            }
        }
    }
    if delta_y.is_some() {
        for edge in [next.y, next.y + next.height] {
            if horizontal.iter().any(|line| (line - edge).abs() <= 1e-6) {
                push_guide(GuideOrientation::Horizontal, edge);
            }
        }
    }
    (next, guides)
}

fn snap_resized_bounds(
    initial: Rect,
    handle: ResizeHandle,
    mut next: Rect,
    vertical: &[f64],
    horizontal: &[f64],
    threshold: f64,
    minimum: f64,
) -> (Rect, Vec<AlignmentGuide>) {
    if threshold <= 0. {
        return (next, Vec::new());
    }
    let min = minimum.max(1.);
    let anchor = resize_handle_point(initial, opposite_resize_handle(handle));
    let mut guides = Vec::new();
    if (next.x - anchor.x).abs() > 0.5
        && let Some(position) = closest_snap(next.x, vertical, threshold)
        && next.width + next.x - position >= min
    {
        next.width += next.x - position;
        next.x = position;
        guides.push(AlignmentGuide {
            orientation: GuideOrientation::Vertical,
            position,
        });
    }
    if (next.x + next.width - anchor.x).abs() > 0.5 {
        let right = next.x + next.width;
        if let Some(position) = closest_snap(right, vertical, threshold)
            && position - next.x >= min
        {
            next.width = position - next.x;
            guides.push(AlignmentGuide {
                orientation: GuideOrientation::Vertical,
                position,
            });
        }
    }
    if (next.y - anchor.y).abs() > 0.5
        && let Some(position) = closest_snap(next.y, horizontal, threshold)
        && next.height + next.y - position >= min
    {
        next.height += next.y - position;
        next.y = position;
        guides.push(AlignmentGuide {
            orientation: GuideOrientation::Horizontal,
            position,
        });
    }
    if (next.y + next.height - anchor.y).abs() > 0.5 {
        let bottom = next.y + next.height;
        if let Some(position) = closest_snap(bottom, horizontal, threshold)
            && position - next.y >= min
        {
            next.height = position - next.y;
            guides.push(AlignmentGuide {
                orientation: GuideOrientation::Horizontal,
                position,
            });
        }
    }
    (next, guides)
}

fn resize_element(element: &Element, initial: Rect, next: Rect) -> Result<Element, String> {
    let scale_x = next.width / initial.width.max(1.);
    let scale_y = next.height / initial.height.max(1.);
    let map = |point: Point| Point {
        x: next.x + (point.x - initial.x) * scale_x,
        y: next.y + (point.y - initial.y) * scale_y,
    };
    let mut resized = element.clone();
    match &mut resized {
        Element::Image(image) => {
            let origin = map(Point {
                x: image.base.x,
                y: image.base.y,
            });
            image.base.x = origin.x;
            image.base.y = origin.y;
            image.width = (image.width * scale_x).max(1.);
            image.height = (image.height * scale_y).max(1.);
        }
        Element::Text(text) => *text = crate::editor_text::resize(text, initial, next)?,
        Element::Shape(shape) => {
            let start = map(Point {
                x: shape.base.x,
                y: shape.base.y,
            });
            let end = map(Point {
                x: shape.end_x,
                y: shape.end_y,
            });
            shape.base.x = start.x;
            shape.base.y = start.y;
            shape.end_x = end.x;
            shape.end_y = end.y;
            for point in &mut shape.controls {
                *point = map(*point);
            }
            if matches!(shape.shape.as_str(), "line" | "arrow") {
                let stroke_scale = scale_x.abs().min(scale_y.abs()).max(0.05);
                if stroke_scale != 1. {
                    shape.style.stroke_width =
                        clamp(shape.style.stroke_width * stroke_scale, 1., 80.).round();
                }
            }
        }
        Element::Path(path) => {
            let origin = map(Point {
                x: path.base.x,
                y: path.base.y,
            });
            path.base.x = origin.x;
            path.base.y = origin.y;
            for point in &mut path.points {
                *point = map(*point);
            }
        }
    }
    Ok(resized)
}

fn preserve_element_world_point(
    initial: &Element,
    next: &mut Element,
    anchor: Point,
) -> Result<(), String> {
    let before = rotate_point(
        anchor,
        rect_center(initial.selection_bounds()?),
        initial.base().rotation(),
    );
    let after = rotate_point(
        anchor,
        rect_center(next.selection_bounds()?),
        next.base().rotation(),
    );
    let delta_x = before.x - after.x;
    let delta_y = before.y - after.y;
    if delta_x.abs() >= 1e-9 || delta_y.abs() >= 1e-9 {
        next.translate(delta_x, delta_y);
    }
    Ok(())
}

/// Sample the exact midpoint-quadratic centerline used to render a freehand
/// path. Native hosts use this for transient previews from accepted samples.
#[must_use]
pub fn smooth_path_centerline(points: &[Point]) -> Vec<Point> {
    let points = points
        .iter()
        .map(|point| captures_image::Point {
            x: point.x as f32,
            y: point.y as f32,
        })
        .collect::<Vec<_>>();
    captures_image::smooth_path_samples(&points)
        .into_iter()
        .map(|point| Point {
            x: f64::from(point.x),
            y: f64::from(point.y),
        })
        .collect()
}

/// Shipping tapered-arrow outline in document coordinates. Native hosts use
/// this for transient previews; committed rendering consumes the same polygon.
#[must_use]
pub fn arrow_fill_polygon(shape: &ShapeElement) -> Vec<Point> {
    const HEAD_LENGTH_RATIO: f64 = 3.5;
    const HEAD_WIDTH_RATIO: f64 = 3.1;
    const TAIL_WIDTH_RATIO: f64 = 0.18;
    const NECK_WIDTH_RATIO: f64 = 1.12;
    const HEAD_SHAFT_FRACTION: f64 = 0.36;
    const FULL_STROKE_LENGTH_RATIO: f64 = 7.;
    const TAIL_CAP_SEGMENTS: usize = 7;

    if shape.shape != "arrow" {
        return Vec::new();
    }
    let vertices = std::iter::once(Point {
        x: shape.base.x,
        y: shape.base.y,
    })
    .chain(shape.controls.iter().copied())
    .chain(std::iter::once(Point {
        x: shape.end_x,
        y: shape.end_y,
    }))
    .collect::<Vec<_>>();
    let samples = sample_controlled_path(&vertices, 28);
    let mut cumulative = Vec::with_capacity(samples.len());
    cumulative.push(0.);
    for index in 1..samples.len() {
        cumulative.push(
            cumulative[index - 1]
                + (samples[index].x - samples[index - 1].x)
                    .hypot(samples[index].y - samples[index - 1].y),
        );
    }
    let path_length = cumulative.last().copied().unwrap_or(0.);
    if path_length < ARROW_MIN_DRAW_LENGTH {
        return Vec::new();
    }
    let authored_stroke = shape.style.stroke_width;
    let full_at = 28_f64.max(authored_stroke * FULL_STROKE_LENGTH_RATIO);
    let stroke = authored_stroke.min(authored_stroke * path_length / full_at);
    if stroke <= 0. {
        return Vec::new();
    }
    let head_length = (stroke * HEAD_LENGTH_RATIO).min(path_length * HEAD_SHAFT_FRACTION);
    let shaft_end = path_length - head_length;
    let head_half = stroke * HEAD_WIDTH_RATIO / 2.;
    let tail_half = stroke * TAIL_WIDTH_RATIO / 2.;
    let neck_half = stroke * NECK_WIDTH_RATIO / 2.;
    let offset_at = |point: Point, tangent: Point, half: f64| {
        (
            Point {
                x: point.x - tangent.y * half,
                y: point.y + tangent.x * half,
            },
            Point {
                x: point.x + tangent.y * half,
                y: point.y - tangent.x * half,
            },
        )
    };
    let shaft_steps = samples.len().max(8);
    let mut left = Vec::with_capacity(shaft_steps + 1);
    let mut right = Vec::with_capacity(shaft_steps + 1);
    for step in 0..=shaft_steps {
        let distance = shaft_end * step as f64 / shaft_steps as f64;
        let (point, tangent) = point_and_tangent_at_length(&samples, &cumulative, distance);
        let mix = if shaft_end > 0. {
            distance / shaft_end
        } else {
            0.
        };
        let half = tail_half + (neck_half - tail_half) * mix;
        let (left_point, right_point) = offset_at(point, tangent, half);
        left.push(left_point);
        right.push(right_point);
    }
    let (neck, neck_tangent) = point_and_tangent_at_length(&samples, &cumulative, shaft_end);
    let (shoulder_left, shoulder_right) = offset_at(neck, neck_tangent, head_half);
    let tip = *samples
        .last()
        .expect("sampled arrow has at least two points");
    let (tail, tail_tangent) = point_and_tangent_at_length(&samples, &cumulative, 0.);
    let tail_normal = Point {
        x: -tail_tangent.y,
        y: tail_tangent.x,
    };
    let cap = (0..=TAIL_CAP_SEGMENTS)
        .map(|step| {
            let angle = std::f64::consts::PI * step as f64 / TAIL_CAP_SEGMENTS as f64;
            Point {
                x: tail.x
                    - tail_normal.x * tail_half * angle.cos()
                    - tail_tangent.x * tail_half * angle.sin(),
                y: tail.y
                    - tail_normal.y * tail_half * angle.cos()
                    - tail_tangent.y * tail_half * angle.sin(),
            }
        })
        .collect::<Vec<_>>();
    left.into_iter()
        .chain([shoulder_left, tip, shoulder_right])
        .chain(right.into_iter().rev())
        .chain(cap[1..cap.len() - 1].iter().copied())
        .collect()
}

fn quadratic_point(from: Point, control: Point, to: Point, t: f64) -> Point {
    let inverse = 1. - t;
    Point {
        x: inverse * inverse * from.x + 2. * inverse * t * control.x + t * t * to.x,
        y: inverse * inverse * from.y + 2. * inverse * t * control.y + t * t * to.y,
    }
}

pub(crate) fn sample_controlled_path(vertices: &[Point], steps: usize) -> Vec<Point> {
    if vertices.len() < 2 {
        return vertices.to_vec();
    }
    let steps = steps.max(4);
    if vertices.len() == 2 {
        let [start, end] = [vertices[0], vertices[1]];
        return (0..=steps)
            .map(|index| {
                let t = index as f64 / steps as f64;
                Point {
                    x: start.x + (end.x - start.x) * t,
                    y: start.y + (end.y - start.y) * t,
                }
            })
            .collect();
    }
    if vertices.len() == 3 {
        return (0..=steps)
            .map(|index| {
                quadratic_point(
                    vertices[0],
                    vertices[1],
                    vertices[2],
                    index as f64 / steps as f64,
                )
            })
            .collect();
    }
    let mut samples = vec![vertices[0]];
    for index in 1..vertices.len() - 2 {
        let from = *samples
            .last()
            .expect("controlled path starts with one sample");
        let to = Point {
            x: (vertices[index].x + vertices[index + 1].x) / 2.,
            y: (vertices[index].y + vertices[index + 1].y) / 2.,
        };
        samples
            .extend((1..=steps).map(|step| {
                quadratic_point(from, vertices[index], to, step as f64 / steps as f64)
            }));
    }
    let from = *samples
        .last()
        .expect("controlled path starts with one sample");
    let control = vertices[vertices.len() - 2];
    let end = vertices[vertices.len() - 1];
    samples.extend(
        (1..=steps).map(|step| quadratic_point(from, control, end, step as f64 / steps as f64)),
    );
    samples
}

fn point_and_tangent_at_length(
    samples: &[Point],
    cumulative: &[f64],
    target: f64,
) -> (Point, Point) {
    let first = samples[0];
    let last = samples[samples.len() - 1];
    let unit = |from: Point, to: Point| {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let length = dx.hypot(dy);
        if length < 1e-6 {
            Point { x: 1., y: 0. }
        } else {
            Point {
                x: dx / length,
                y: dy / length,
            }
        }
    };
    if target <= 0. {
        return (first, unit(first, samples[1]));
    }
    let total = *cumulative
        .last()
        .expect("cumulative arrow lengths are nonempty");
    if target >= total {
        return (last, unit(samples[samples.len() - 2], last));
    }
    for index in 1..samples.len() {
        if cumulative[index] >= target {
            let span = cumulative[index] - cumulative[index - 1];
            let t = if span > 0. {
                (target - cumulative[index - 1]) / span
            } else {
                1.
            };
            let from = samples[index - 1];
            let to = samples[index];
            return (
                Point {
                    x: from.x + (to.x - from.x) * t,
                    y: from.y + (to.y - from.y) * t,
                },
                unit(from, to),
            );
        }
    }
    (last, unit(samples[samples.len() - 2], last))
}

pub(crate) fn annotation_drop_shadow_pad(style: &ElementStyle) -> f64 {
    if !style.has_drop_shadow() {
        return 0.;
    }
    let width = style.stroke_width.max(1.);
    let fallback_opacity = 45.;
    let fallback_blur = 6_f64.max(width * 0.85);
    let fallback_offset_x = 0.;
    let fallback_offset_y = 2_f64.max((width * 0.32).round());
    let resolved = style.drop_shadow_style.as_ref();
    let resolve = |value: Option<f64>, minimum: f64, maximum: f64, fallback: f64| {
        value
            .filter(|value| value.is_finite())
            .map_or(fallback, |value| value.clamp(minimum, maximum))
    };
    let opacity = resolve(
        resolved.map(|shadow| shadow.opacity),
        0.,
        100.,
        fallback_opacity,
    );
    if opacity <= 0. {
        return 0.;
    }
    let blur = resolve(resolved.map(|shadow| shadow.blur), 0., 100., fallback_blur);
    let offset_x = resolve(
        resolved.map(|shadow| shadow.offset_x),
        -500.,
        500.,
        fallback_offset_x,
    );
    let offset_y = resolve(
        resolved.map(|shadow| shadow.offset_y),
        -500.,
        500.,
        fallback_offset_y,
    );
    (blur * 2. + offset_x.abs().max(offset_y.abs())).ceil()
}

impl AnnotationStylePatch {
    pub(crate) fn apply(self, style: &mut ElementStyle, closed: bool) -> Result<(), String> {
        if self
            .stroke_width
            .is_some_and(|stroke_width| !stroke_width.is_finite())
        {
            return Err("Annotation stroke width must be finite.".into());
        }
        if let Some(color) = self.color {
            style.color = color;
        }
        if closed {
            match self.fill {
                OptionalNullable::Missing => {}
                OptionalNullable::Null => style.fill = None,
                OptionalNullable::Value(fill) => style.fill = Some(fill),
            }
            if let Some(stroke_enabled) = self.stroke_enabled {
                style.stroke_enabled = Some(stroke_enabled);
            }
        }
        if let Some(stroke_width) = self.stroke_width {
            style.stroke_width = stroke_width;
        }
        if let Some(drop_shadow) = self.drop_shadow {
            style.drop_shadow = Some(drop_shadow);
        }
        if let Some(patch) = self.drop_shadow_style.filter(|patch| !patch.is_empty()) {
            let mut shadow = style.resolved_drop_shadow_style();
            if let Some(color) = patch.color {
                shadow.color = color;
            }
            if let Some(opacity) = patch.opacity {
                shadow.opacity = opacity;
            }
            if let Some(blur) = patch.blur {
                shadow.blur = blur;
            }
            if let Some(offset_x) = patch.offset_x {
                shadow.offset_x = offset_x;
            }
            if let Some(offset_y) = patch.offset_y {
                shadow.offset_y = offset_y;
            }
            style.drop_shadow = Some(true);
            style.drop_shadow_style = Some(shadow);
            style.drop_shadow_style = Some(style.resolved_drop_shadow_style());
        }
        Ok(())
    }
}

impl DropShadowStylePatch {
    fn is_empty(&self) -> bool {
        self.color.is_none()
            && self.opacity.is_none()
            && self.blur.is_none()
            && self.offset_x.is_none()
            && self.offset_y.is_none()
    }
}

fn resolved_shadow_color(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let raw = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if !raw.is_ascii() {
        return None;
    }
    let digits = match raw.len() {
        3 => 3,
        6.. => 6,
        _ => return None,
    };
    raw.chars()
        .take(digits)
        .all(|character| character.is_ascii_hexdigit())
        .then(|| value.chars().take(7).collect())
}

pub(crate) fn image_bounds(image: &ImageElement) -> Rect {
    let local = Rect {
        x: image.base.x,
        y: image.base.y,
        width: image.width,
        height: image.height,
    };
    let rotation = image.base.rotation();
    if rotation == 0. {
        return local;
    }
    let center_x = local.x + local.width / 2.;
    let center_y = local.y + local.height / 2.;
    let (sin, cos) = rotation.sin_cos();
    let rotate = |x: f64, y: f64| Point {
        x: center_x + (x - center_x) * cos - (y - center_y) * sin,
        y: center_y + (x - center_x) * sin + (y - center_y) * cos,
    };
    let points = [
        rotate(local.x, local.y),
        rotate(local.x + local.width, local.y),
        rotate(local.x + local.width, local.y + local.height),
        rotate(local.x, local.y + local.height),
    ];
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    Rect {
        x: min_x,
        y: min_y,
        width: (max_x - min_x).max(1.),
        height: (max_y - min_y).max(1.),
    }
}

fn painted_bounds(element: &Element) -> Result<Rect, String> {
    if let Element::Image(image) = element {
        return Ok(image_bounds(image));
    }
    let rotation = element.base().rotation();
    if rotation == 0. {
        return element.selection_bounds();
    }
    if let Element::Path(path) = element
        && !path.points.is_empty()
    {
        let origin = rect_center(element.selection_bounds()?);
        return Ok(bounds_from_points(
            &path
                .points
                .iter()
                .map(|point| rotate_point(*point, origin, rotation))
                .collect::<Vec<_>>(),
            path.style.stroke_width.max(4.) + annotation_drop_shadow_pad(&path.style),
        ));
    }
    Ok(bounds_from_points(&element.selection_outline()?, 0.))
}

fn fully_outside_canvas(bounds: Rect, width: f64, height: f64) -> bool {
    const EPSILON: f64 = 0.5;
    !(bounds.x + bounds.width > EPSILON
        && width > bounds.x + EPSILON
        && bounds.y + bounds.height > EPSILON
        && height > bounds.y + EPSILON)
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
        x: js_round(x),
        y: js_round(y),
        width: js_round((end.x - start.x).abs()).max(1.),
        height: js_round((end.y - start.y).abs()).max(1.),
    }
}

const CROP_SHIFT_LOCK_MIN_SIZE: f64 = 8.;

fn valid_aspect(aspect: Option<f64>) -> Option<f64> {
    aspect.filter(|value| value.is_finite() && *value > 0.)
}

fn crop_aspect_from_live_rect(rect: Rect) -> Option<f64> {
    (rect.width >= CROP_SHIFT_LOCK_MIN_SIZE && rect.height >= CROP_SHIFT_LOCK_MIN_SIZE)
        .then_some(rect.width / rect.height)
}

fn shift_locked_crop_aspect(origin: Point, current: Point, bounds: Rect) -> f64 {
    crop_aspect_from_live_rect(bounded_crop_rect(origin, current, bounds, None)).unwrap_or(1.)
}

fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor < 0.5 {
        floor
    } else {
        floor + 1.
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
