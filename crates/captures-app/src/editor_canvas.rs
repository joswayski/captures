//! Screenshot-editor canvas interactions shared by the native hosts.
//!
//! Ports the shipping editor's (`apps/desktop/ui/src/lib/screenshotEditor.ts`)
//! "Expand canvas" overflow geometry and line/arrow curve editing. Hosts own
//! pointer state, hover chrome and painting; these functions own the geometry,
//! copy and document transactions so AppKit and wgpu cannot drift.

use serde::{Deserialize, Serialize};

use crate::editor::{
    Document, Element, Point, Rect, ShapeElement, rect_center, rotate_point, sample_controlled_path,
};

/// Shipping `.screenshot-canvas-expand-action` label.
pub const EXPAND_CANVAS: &str = "Expand canvas";
/// Screen-pixel distance of the Expand canvas action outside the canvas edge.
pub const EXPAND_ACTION_INSET: f64 = 22.;

/// Shipping Curve inspector copy.
pub const CURVE_LABEL: &str = "Curve";
pub const CURVE_MARKS: [(f64, &str); 3] = [(-100., "Left"), (0., "Straight"), (100., "Right")];
pub const CURVE_HELP: &str = "Drag the curve dots to reshape. Double-click the path to add more \
points; double-click a point to remove it.";
/// Discovery tip over an unselected line/arrow path.
pub const CURVE_UNSELECTED_HINT: &str = "Click to select · double-click path to add curve points";

/// Canvas borders in the shipping `canvasOverflowEdges` order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CanvasEdge {
    Left,
    Top,
    Right,
    Bottom,
}

/// Idle overflow presentation for one layer that hangs past the canvas.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CanvasExpandPreview {
    /// Borders the layer crosses; these sides of the ghost glow.
    pub edges: Vec<CanvasEdge>,
    /// The post-expand canvas in current document coordinates (ghost outline).
    pub rect: Rect,
    /// Off-canvas remainder of the layer, one rectangle per crossed edge.
    pub gaps: Vec<Rect>,
    /// Painted layer bounds used for the preview.
    pub bounds: Rect,
    /// Expand canvas action center on the canvas border of the largest gap.
    /// Hosts push it [`EXPAND_ACTION_INSET`] screen pixels outward along
    /// `anchor_edge`, matching shipping's `22 / displayScale` inset.
    pub anchor: Point,
    pub anchor_edge: CanvasEdge,
}

const OVERFLOW_EPSILON: f64 = 0.5;

/// Which canvas borders a box crosses (partially outside the document).
#[must_use]
pub fn canvas_overflow_edges(bounds: Rect, width: f64, height: f64) -> Vec<CanvasEdge> {
    let mut edges = Vec::new();
    if bounds.x < -OVERFLOW_EPSILON {
        edges.push(CanvasEdge::Left);
    }
    if bounds.y < -OVERFLOW_EPSILON {
        edges.push(CanvasEdge::Top);
    }
    if bounds.x + bounds.width > width + OVERFLOW_EPSILON {
        edges.push(CanvasEdge::Right);
    }
    if bounds.y + bounds.height > height + OVERFLOW_EPSILON {
        edges.push(CanvasEdge::Bottom);
    }
    edges
}

/// Off-canvas remainder of `bounds`, one rectangle per overflowing edge.
#[must_use]
pub fn canvas_overflow_gaps(bounds: Rect, width: f64, height: f64) -> Vec<(CanvasEdge, Rect)> {
    canvas_overflow_edges(bounds, width, height)
        .into_iter()
        .filter_map(|edge| {
            let rect = match edge {
                CanvasEdge::Left => Rect {
                    width: bounds.width.min(-bounds.x),
                    ..bounds
                },
                CanvasEdge::Right => {
                    let x = bounds.x.max(width);
                    Rect {
                        x,
                        width: bounds.x + bounds.width - x,
                        ..bounds
                    }
                }
                CanvasEdge::Top => Rect {
                    height: bounds.height.min(-bounds.y),
                    ..bounds
                },
                CanvasEdge::Bottom => {
                    let y = bounds.y.max(height);
                    Rect {
                        y,
                        height: bounds.y + bounds.height - y,
                        ..bounds
                    }
                }
            };
            (rect.width > OVERFLOW_EPSILON && rect.height > OVERFLOW_EPSILON)
                .then_some((edge, rect))
        })
        .collect()
}

/// Canvas-border center of the largest overflow gap, before the screen inset.
#[must_use]
pub fn expand_action_anchor(bounds: Rect, width: f64, height: f64) -> Option<(Point, CanvasEdge)> {
    let (edge, rect) = canvas_overflow_gaps(bounds, width, height)
        .into_iter()
        .reduce(|best, gap| {
            if gap.1.width * gap.1.height > best.1.width * best.1.height {
                gap
            } else {
                best
            }
        })?;
    let point = match edge {
        CanvasEdge::Left => Point {
            x: 0.,
            y: rect.y + rect.height / 2.,
        },
        CanvasEdge::Right => Point {
            x: width,
            y: rect.y + rect.height / 2.,
        },
        CanvasEdge::Top => Point {
            x: rect.x + rect.width / 2.,
            y: 0.,
        },
        CanvasEdge::Bottom => Point {
            x: rect.x + rect.width / 2.,
            y: height,
        },
    };
    Some((point, edge))
}

/// Offset an anchor outward from its canvas edge by a document distance.
#[must_use]
pub fn inset_anchor(anchor: Point, edge: CanvasEdge, inset: f64) -> Point {
    match edge {
        CanvasEdge::Left => Point {
            x: anchor.x - inset,
            ..anchor
        },
        CanvasEdge::Right => Point {
            x: anchor.x + inset,
            ..anchor
        },
        CanvasEdge::Top => Point {
            y: anchor.y - inset,
            ..anchor
        },
        CanvasEdge::Bottom => Point {
            y: anchor.y + inset,
            ..anchor
        },
    }
}

/// Shipping `previewExpandedCanvasRect`: the grown canvas in current document
/// coordinates, or `None` when `bounds` already fits.
#[must_use]
pub fn preview_expanded_canvas_rect(bounds: Rect, width: f64, height: f64) -> Option<Rect> {
    let shift_x = (-bounds.x).ceil().max(0.);
    let shift_y = (-bounds.y).ceil().max(0.);
    let grown_width = (width + shift_x).max((bounds.x + shift_x + bounds.width).ceil());
    let grown_height = (height + shift_y).max((bounds.y + shift_y + bounds.height).ceil());
    if shift_x == 0. && shift_y == 0. && grown_width == width && grown_height == height {
        return None;
    }
    Some(Rect {
        x: if shift_x == 0. { 0. } else { -shift_x },
        y: if shift_y == 0. { 0. } else { -shift_y },
        width: grown_width,
        height: grown_height,
    })
}

/// Idle overflow preview for a visible layer that crosses a canvas border.
pub fn canvas_expand_preview(
    element: &Element,
    width: f64,
    height: f64,
) -> Result<Option<CanvasExpandPreview>, String> {
    if !element.base().visible {
        return Ok(None);
    }
    let bounds = element.painted_bounds()?;
    let edges = canvas_overflow_edges(bounds, width, height);
    if edges.is_empty() {
        return Ok(None);
    }
    let (Some(rect), Some((anchor, anchor_edge))) = (
        preview_expanded_canvas_rect(bounds, width, height),
        expand_action_anchor(bounds, width, height),
    ) else {
        return Ok(None);
    };
    Ok(Some(CanvasExpandPreview {
        edges,
        rect,
        gaps: canvas_overflow_gaps(bounds, width, height)
            .into_iter()
            .map(|(_, rect)| rect)
            .collect(),
        bounds,
        anchor,
        anchor_edge,
    }))
}

impl Document {
    /// Shipping `expandCanvasToFitElement`: grow (never shrink) the canvas so
    /// one layer's painted bounds fit, shifting every layer for negative
    /// overflow. Hosts submit it as one layer edit, so it is one undo step.
    pub fn expand_canvas_to_fit(&mut self, id: &str) -> Result<(), String> {
        let element = self
            .elements
            .iter()
            .find(|element| element.base().id == id)
            .ok_or("The selected layer no longer exists.")?;
        let bounds = element.painted_bounds()?;
        let Some(rect) = preview_expanded_canvas_rect(bounds, self.width, self.height) else {
            return Ok(());
        };
        if rect.width > crate::editor::MAX_CANVAS_DIMENSION
            || rect.height > crate::editor::MAX_CANVAS_DIMENSION
        {
            return Err("The expanded canvas would exceed 32768 pixels.".into());
        }
        self.width = rect.width;
        self.height = rect.height;
        self.translate(-rect.x, -rect.y);
        Ok(())
    }
}

/// Shipping `.screenshot-canvas-trim-hint` `--trim-rgb`: the red "negative"
/// of the accent expand glow. Shipping hard-codes it, so hosts do too.
pub const TRIM_RGB: [u8; 3] = [255, 92, 106];
/// `.screenshot-canvas-trim-region` fill alpha over the discarded margins.
pub const TRIM_REGION_ALPHA: f64 = 0.14;
/// `.screenshot-canvas-trim-keep`: `1.5px dashed` border at this alpha, 3 px
/// radius, with a 22 px glow at [`TRIM_KEEP_GLOW_ALPHA`].
pub const TRIM_KEEP_ALPHA: f64 = 0.72;
pub const TRIM_KEEP_WIDTH: f64 = 1.5;
pub const TRIM_KEEP_RADIUS: f64 = 3.;
pub const TRIM_KEEP_GLOW_ALPHA: f64 = 0.18;
/// `.screenshot-canvas-trim-edge::after`: a 4 px pill straddling each cut edge.
pub const TRIM_EDGE_BAR: f64 = 4.;
/// `.screenshot-canvas-trim-bloom`: a 96 px outward gradient from each cut
/// edge (0.5 → 0.2 at 38 % → 0.06 at 72 % → clear).
pub const TRIM_BLOOM: f64 = 96.;
pub const TRIM_BLOOM_STOPS: [(f64, f64); 4] = [(0., 0.5), (0.38, 0.2), (0.72, 0.06), (1., 0.)];

/// Pixel strips Trim edges removes from each side of the current canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct TrimMargins {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

/// Shipping `canvasTrimMarginPreview`: what the Trim edges hover preview
/// tints and outlines, in current document coordinates.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CanvasTrimPreview {
    /// The part of the current canvas that stays after the trim.
    pub keep: Rect,
    pub margins: TrimMargins,
    /// Edges with a positive margin, in shipping's top/right/bottom/left order;
    /// these glow and stream particles.
    pub edges: Vec<CanvasEdge>,
    /// Discarded strips: full-width top/bottom, then left/right between them.
    pub regions: Vec<(CanvasEdge, Rect)>,
}

/// The integer canvas frame Trim edges fits to `bounds` (shipping
/// `trimDocumentToContent` with no padding), in current document coordinates.
#[must_use]
pub fn trim_frame(bounds: Rect) -> Rect {
    let x = bounds.x.floor();
    let y = bounds.y.floor();
    let right = (bounds.x + bounds.width).ceil();
    let bottom = (bounds.y + bounds.height).ceil();
    Rect {
        x,
        y,
        width: (right - x).max(1.),
        height: (bottom - y).max(1.),
    }
}

impl Document {
    /// Shipping `canTrimEdges`: visible layers leave empty margin on the
    /// canvas or overhang it, so Trim edges would change the document. Hosts
    /// disable the button otherwise.
    #[must_use]
    pub fn can_trim_to_content(&self) -> bool {
        let Ok(Some(bounds)) = self.visible_content_bounds() else {
            return false;
        };
        let frame = trim_frame(bounds);
        frame.x != 0. || frame.y != 0. || frame.width != self.width || frame.height != self.height
    }

    /// Shipping `canvasTrimMarginPreview`: the margins Trim edges would cut
    /// from the current canvas. Overhanging content grows the canvas rather
    /// than being removed, so only positive interior margins count; `None`
    /// when the trim is a no-op or cuts nothing on-canvas.
    #[must_use]
    pub fn trim_preview(&self) -> Option<CanvasTrimPreview> {
        if !self.can_trim_to_content() {
            return None;
        }
        let frame = trim_frame(self.visible_content_bounds().ok()??);
        let left = frame.x.max(0.);
        let top = frame.y.max(0.);
        let right = (frame.x + frame.width).min(self.width);
        let bottom = (frame.y + frame.height).min(self.height);
        if right <= left || bottom <= top {
            return None;
        }
        let margins = TrimMargins {
            top,
            right: (self.width - right).max(0.),
            bottom: (self.height - bottom).max(0.),
            left,
        };
        let mut edges = Vec::new();
        let mut regions = Vec::new();
        let middle = self.height - margins.top - margins.bottom;
        for (edge, margin, rect) in [
            (
                CanvasEdge::Top,
                margins.top,
                Rect {
                    x: 0.,
                    y: 0.,
                    width: self.width,
                    height: margins.top,
                },
            ),
            (
                CanvasEdge::Right,
                margins.right,
                Rect {
                    x: self.width - margins.right,
                    y: margins.top,
                    width: margins.right,
                    height: middle,
                },
            ),
            (
                CanvasEdge::Bottom,
                margins.bottom,
                Rect {
                    x: 0.,
                    y: self.height - margins.bottom,
                    width: self.width,
                    height: margins.bottom,
                },
            ),
            (
                CanvasEdge::Left,
                margins.left,
                Rect {
                    x: 0.,
                    y: margins.top,
                    width: margins.left,
                    height: middle,
                },
            ),
        ] {
            if margin > 0. {
                edges.push(edge);
                regions.push((edge, rect));
            }
        }
        if edges.is_empty() {
            return None;
        }
        Some(CanvasTrimPreview {
            keep: Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            },
            margins,
            edges,
            regions,
        })
    }
}

/// Editable handle on a selected line/arrow.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurveHandle {
    Start,
    End,
    Control {
        index: usize,
    },
    /// One of three virtual on-stroke dots shown while a stroke is straight.
    StarterControl {
        index: usize,
    },
}

/// Curve edits submitted through [`crate::editor::LayerEdit::Curve`].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurveEdit {
    /// Curve slider (−1…1). Near zero straightens.
    Bend { bend: f64 },
    /// Double-click on the path: add a control nearest `point` (world space).
    Insert { point: Point },
    /// Double-click on a control dot.
    Remove { index: usize },
    /// Release after dragging a handle to `point` (world space).
    Move { handle: CurveHandle, point: Point },
    /// Straighten arrow/line.
    Straighten,
}

/// World-space curve chrome for a selected line/arrow.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CurveHandles {
    pub start: Point,
    pub end: Point,
    pub controls: Vec<Point>,
    /// Virtual starter dots; empty once the stroke has free controls.
    pub starters: Vec<Point>,
    /// Curve slider value in percent (−100…100), shown when `slider`.
    pub bend_percent: f64,
    /// Straight and single-control strokes show the Curve slider; multi-point
    /// strokes show the Straighten action instead.
    pub slider: bool,
    pub straighten_label: &'static str,
    /// Sampled world-space centerline, for hosts' path hover/hit feedback.
    pub path: Vec<Point>,
}

#[must_use]
pub fn is_curveable(shape: &ShapeElement) -> bool {
    matches!(shape.shape.as_str(), "line" | "arrow")
}

#[must_use]
pub fn straighten_label(shape: &ShapeElement) -> &'static str {
    if shape.shape == "arrow" {
        "Straighten arrow"
    } else {
        "Straighten line"
    }
}

fn rotation_origin(shape: &ShapeElement) -> Point {
    Element::Shape(shape.clone()).selection_bounds().map_or(
        Point {
            x: shape.base.x,
            y: shape.base.y,
        },
        rect_center,
    )
}

fn local_point(shape: &ShapeElement, world: Point) -> Point {
    let rotation = shape.base.rotation();
    if rotation == 0. {
        return world;
    }
    rotate_point(world, rotation_origin(shape), -rotation)
}

fn world_point(shape: &ShapeElement, local: Point) -> Point {
    let rotation = shape.base.rotation();
    if rotation == 0. {
        return local;
    }
    rotate_point(local, rotation_origin(shape), rotation)
}

fn vertices(shape: &ShapeElement) -> Vec<Point> {
    std::iter::once(Point {
        x: shape.base.x,
        y: shape.base.y,
    })
    .chain(shape.controls.iter().copied())
    .chain(std::iter::once(Point {
        x: shape.end_x,
        y: shape.end_y,
    }))
    .collect()
}

/// Three evenly spaced virtual controls for a straight stroke (local space).
#[must_use]
pub fn starter_controls(shape: &ShapeElement) -> [Point; 3] {
    [0.25, 0.5, 0.75].map(|progress| Point {
        x: shape.base.x + (shape.end_x - shape.base.x) * progress,
        y: shape.base.y + (shape.end_y - shape.base.y) * progress,
    })
}

/// Closest sampled point on the path (world space), the control insert index
/// and the local-space distance, matching shipping `closestPointOnArrow`.
#[must_use]
pub fn closest_point_on_curve(shape: &ShapeElement, point: Point) -> (Point, usize, f64) {
    let local = local_point(shape, point);
    let samples = sample_controlled_path(&vertices(shape), 24);
    let mut best = (samples[0], 0_usize, f64::INFINITY);
    for (index, sample) in samples.iter().enumerate() {
        let distance = (local.x - sample.x).hypot(local.y - sample.y);
        if distance < best.2 {
            best = (*sample, index, distance);
        }
    }
    let progress = if samples.len() <= 1 {
        0.5
    } else {
        best.1 as f64 / (samples.len() - 1) as f64
    };
    let count = shape.controls.len();
    let insert = ((progress * (count + 1) as f64).floor() as usize).min(count);
    (world_point(shape, best.0), insert, best.2)
}

/// Hit-test curve handles: free controls, endpoints, then starter dots.
#[must_use]
pub fn hit_test_curve_handle(
    shape: &ShapeElement,
    point: Point,
    radius: f64,
) -> Option<CurveHandle> {
    if !is_curveable(shape) {
        return None;
    }
    let local = local_point(shape, point);
    let radius = radius.max(4.);
    let near =
        |target: Point, radius: f64| (local.x - target.x).hypot(local.y - target.y) <= radius;
    if let Some(index) = shape
        .controls
        .iter()
        .position(|control| near(*control, radius))
    {
        return Some(CurveHandle::Control { index });
    }
    if near(
        Point {
            x: shape.base.x,
            y: shape.base.y,
        },
        radius,
    ) {
        return Some(CurveHandle::Start);
    }
    if near(
        Point {
            x: shape.end_x,
            y: shape.end_y,
        },
        radius,
    ) {
        return Some(CurveHandle::End);
    }
    if shape.controls.is_empty() {
        return starter_controls(shape)
            .iter()
            .position(|starter| near(*starter, radius * 1.15))
            .map(|index| CurveHandle::StarterControl { index });
    }
    None
}

/// Path hit radius used for double-click insertion and hover tips.
#[must_use]
pub fn curve_path_hit_radius(shape: &ShapeElement, radius: f64) -> f64 {
    radius.max(shape.style.stroke_width * 2. + radius * 0.6)
}

/// Shipping `curveStrokeHoverHint` for a selected, unlocked line/arrow.
#[must_use]
pub fn curve_hover_hint(shape: &ShapeElement, point: Point, radius: f64) -> Option<&'static str> {
    if !is_curveable(shape) || shape.base.locked {
        return None;
    }
    match hit_test_curve_handle(shape, point, radius) {
        Some(CurveHandle::Control { .. }) => return Some("Double-click to remove curve point"),
        Some(CurveHandle::StarterControl { .. }) => {
            return Some("Drag a dot to curve · Double-click the path to add points");
        }
        Some(CurveHandle::Start | CurveHandle::End) => return Some("Drag to move endpoint"),
        None => {}
    }
    let (_, _, distance) = closest_point_on_curve(shape, point);
    (distance <= curve_path_hit_radius(shape, radius))
        .then_some("Double-click to add a curve point")
}

/// Single-control bend for the Curve slider (−1…1); zero when straight or
/// multi-point.
#[must_use]
pub fn curve_bend_amount(shape: &ShapeElement) -> f64 {
    if !is_curveable(shape) || shape.controls.len() != 1 {
        return 0.;
    }
    let delta_x = shape.end_x - shape.base.x;
    let delta_y = shape.end_y - shape.base.y;
    let length_squared = delta_x * delta_x + delta_y * delta_y;
    if length_squared < 1. {
        return 0.;
    }
    let control = shape.controls[0];
    let mid_x = (shape.base.x + shape.end_x) / 2.;
    let mid_y = (shape.base.y + shape.end_y) / 2.;
    (((control.x - mid_x) * -delta_y + (control.y - mid_y) * delta_x) / length_squared)
        .clamp(-1., 1.)
}

#[must_use]
pub fn curve_handles(shape: &ShapeElement) -> Option<CurveHandles> {
    if !is_curveable(shape) {
        return None;
    }
    let world = |point: Point| world_point(shape, point);
    let bend = curve_bend_amount(shape);
    Some(CurveHandles {
        start: world(Point {
            x: shape.base.x,
            y: shape.base.y,
        }),
        end: world(Point {
            x: shape.end_x,
            y: shape.end_y,
        }),
        controls: shape.controls.iter().copied().map(world).collect(),
        starters: if shape.controls.is_empty() {
            starter_controls(shape).into_iter().map(world).collect()
        } else {
            Vec::new()
        },
        bend_percent: (bend * 100.).round(),
        slider: shape.controls.len() <= 1,
        straighten_label: straighten_label(shape),
        path: sample_controlled_path(&vertices(shape), 24)
            .into_iter()
            .map(world)
            .collect(),
    })
}

fn arrow_chord_length(shape: &ShapeElement) -> f64 {
    (shape.end_x - shape.base.x).hypot(shape.end_y - shape.base.y)
}

/// Shipping `scaleArrowStrokeForLength`: shortening an arrow thins its stroke.
fn scale_arrow_stroke_for_length(initial: &ShapeElement, next: &mut ShapeElement) {
    if initial.shape != "arrow" {
        return;
    }
    let initial_length = arrow_chord_length(initial).max(1.);
    let next_length = arrow_chord_length(next);
    if next_length >= initial_length - 0.5 {
        return;
    }
    next.style.stroke_width = (initial.style.stroke_width * (next_length / initial_length))
        .clamp(1., 80.)
        .round();
}

/// Shipping `preserveShapeWorldPoint`: keep `anchor` (local) fixed in world
/// space after an edit moves the rotation pivot.
fn preserve_world_point(initial: &ShapeElement, next: &mut ShapeElement, anchor: Point) {
    if next.base.rotation() == 0. && initial.base.rotation() == 0. {
        return;
    }
    let before = world_point(initial, anchor);
    let after = world_point(next, anchor);
    let delta_x = before.x - after.x;
    let delta_y = before.y - after.y;
    if delta_x.abs() < 1e-9 && delta_y.abs() < 1e-9 {
        return;
    }
    let mut element = Element::Shape(next.clone());
    element.translate(delta_x, delta_y);
    if let Element::Shape(shape) = element {
        *next = shape;
    }
}

fn finite(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

/// Apply one curve edit to a line/arrow. Non-curveable shapes are unchanged.
pub fn apply_curve_edit(shape: &mut ShapeElement, edit: CurveEdit) -> Result<(), String> {
    if !is_curveable(shape) {
        return Ok(());
    }
    let initial = shape.clone();
    let start_anchor = Point {
        x: initial.base.x,
        y: initial.base.y,
    };
    match edit {
        CurveEdit::Bend { bend } => {
            if !bend.is_finite() {
                return Err("Curve amount must be finite.".into());
            }
            let bend = bend.clamp(-1., 1.);
            if bend.abs() < 0.005 {
                shape.controls.clear();
            } else {
                let delta_x = shape.end_x - shape.base.x;
                let delta_y = shape.end_y - shape.base.y;
                shape.controls = vec![Point {
                    x: (shape.base.x + shape.end_x) / 2. - delta_y * bend,
                    y: (shape.base.y + shape.end_y) / 2. + delta_x * bend,
                }];
            }
            preserve_world_point(&initial, shape, start_anchor);
        }
        CurveEdit::Straighten => {
            shape.controls.clear();
            preserve_world_point(&initial, shape, start_anchor);
        }
        CurveEdit::Insert { point } => {
            if !finite(point) {
                return Err("Curve points must be finite.".into());
            }
            let (_, index, _) = closest_point_on_curve(shape, point);
            let local = local_point(shape, point);
            shape.controls.insert(index, local);
            preserve_world_point(&initial, shape, start_anchor);
        }
        CurveEdit::Remove { index } => {
            if index < shape.controls.len() {
                shape.controls.remove(index);
                preserve_world_point(&initial, shape, start_anchor);
            }
        }
        CurveEdit::Move { handle, point } => {
            if !finite(point) {
                return Err("Curve points must be finite.".into());
            }
            let local = local_point(&initial, point);
            match handle {
                CurveHandle::Start => {
                    shape.base.x = local.x;
                    shape.base.y = local.y;
                    scale_arrow_stroke_for_length(&initial, shape);
                    preserve_world_point(
                        &initial,
                        shape,
                        Point {
                            x: initial.end_x,
                            y: initial.end_y,
                        },
                    );
                }
                CurveHandle::End => {
                    shape.end_x = local.x;
                    shape.end_y = local.y;
                    scale_arrow_stroke_for_length(&initial, shape);
                    preserve_world_point(&initial, shape, start_anchor);
                }
                CurveHandle::StarterControl { index } => {
                    if !shape.controls.is_empty() || index >= 3 {
                        return Ok(());
                    }
                    let mut controls = starter_controls(shape).to_vec();
                    controls[index] = local;
                    shape.controls = controls;
                    preserve_world_point(&initial, shape, start_anchor);
                }
                CurveHandle::Control { index } => {
                    let Some(control) = shape.controls.get_mut(index) else {
                        return Ok(());
                    };
                    *control = local;
                    preserve_world_point(&initial, shape, start_anchor);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{ElementBase, ElementStyle, LayerEdit};

    fn line(shape: &str, controls: Vec<Point>) -> ShapeElement {
        ShapeElement {
            base: ElementBase {
                id: "line".into(),
                x: 10.,
                y: 20.,
                rotation: None,
                locked: false,
                visible: true,
                opacity: 100.,
                blend_mode: "source-over".into(),
            },
            shape: shape.into(),
            end_x: 110.,
            end_y: 20.,
            controls,
            style: ElementStyle::default(),
            extra: Default::default(),
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn overflow_edges_gaps_and_anchor_match_shipping() {
        let bounds = rect(-20., 10., 60., 30.);
        assert_eq!(canvas_overflow_edges(bounds, 100., 80.), [CanvasEdge::Left]);
        assert_eq!(
            canvas_overflow_gaps(bounds, 100., 80.),
            [(CanvasEdge::Left, rect(-20., 10., 20., 30.))]
        );
        let (anchor, edge) = expand_action_anchor(bounds, 100., 80.).unwrap();
        assert_eq!((anchor, edge), (Point { x: 0., y: 25. }, CanvasEdge::Left));
        assert_eq!(inset_anchor(anchor, edge, 22.), Point { x: -22., y: 25. });
        // Epsilon: half a pixel of overhang is not overflow.
        assert!(canvas_overflow_edges(rect(-0.4, 0., 100.8, 80.), 100., 80.).is_empty());
        // Largest gap wins for the action.
        let corner = rect(90., 70., 40., 12.);
        assert_eq!(
            canvas_overflow_edges(corner, 100., 80.),
            [CanvasEdge::Right, CanvasEdge::Bottom]
        );
        assert_eq!(
            expand_action_anchor(corner, 100., 80.).unwrap(),
            (Point { x: 100., y: 76. }, CanvasEdge::Right)
        );
    }

    #[test]
    fn trim_rule_and_margin_preview_match_shipping() {
        let mut document = Document::new_capture("fixture:base", 100., 80., None);
        assert!(!document.can_trim_to_content(), "a tight canvas has nothing to trim");
        assert_eq!(document.trim_preview(), None);

        document.resize_canvas(150., 100.);
        assert!(document.can_trim_to_content());
        let preview = document.trim_preview().unwrap();
        assert_eq!(preview.keep, rect(0., 0., 100., 80.));
        assert_eq!(
            preview.margins,
            TrimMargins {
                top: 0.,
                right: 50.,
                bottom: 20.,
                left: 0.,
            }
        );
        assert_eq!(preview.edges, [CanvasEdge::Right, CanvasEdge::Bottom]);
        assert_eq!(
            preview.regions,
            [
                (CanvasEdge::Right, rect(100., 0., 50., 80.)),
                (CanvasEdge::Bottom, rect(0., 80., 150., 20.)),
            ]
        );

        // Fractional content snaps outward, as the trim itself does.
        document.translate(10.4, 5.);
        let preview = document.trim_preview().unwrap();
        assert_eq!(preview.keep, rect(10., 5., 101., 80.));
        assert_eq!(
            preview.edges,
            [
                CanvasEdge::Top,
                CanvasEdge::Right,
                CanvasEdge::Bottom,
                CanvasEdge::Left
            ]
        );
        assert_eq!(preview.regions[0], (CanvasEdge::Top, rect(0., 0., 150., 5.)));
        assert_eq!(preview.regions[3], (CanvasEdge::Left, rect(0., 5., 10., 80.)));
        let mut trimmed = document.clone();
        trimmed.trim_to_content().unwrap();
        assert_eq!((trimmed.width, trimmed.height), (101., 80.));
        assert!(!trimmed.can_trim_to_content(), "trimming is idempotent");

        // Overhang only: the trim grows the canvas but cuts nothing on-canvas.
        document.translate(-10.4, -5.);
        document.resize_canvas(60., 40.);
        assert!(document.can_trim_to_content());
        assert_eq!(document.trim_preview(), None);

        // Hidden layers never hold the canvas open.
        document.resize_canvas(150., 100.);
        if let Element::Image(image) = &mut document.elements[0] {
            image.base.visible = false;
        }
        assert!(!document.can_trim_to_content());
        assert_eq!(document.trim_preview(), None);
    }

    #[test]
    fn expanded_rect_grows_without_shrinking_and_normalizes_zero() {
        assert_eq!(
            preview_expanded_canvas_rect(rect(10., 10., 20., 20.), 100., 80.),
            None
        );
        assert_eq!(
            preview_expanded_canvas_rect(rect(-10.5, 5., 40., 100.), 100., 80.),
            Some(rect(-11., 0., 111., 105.))
        );
        let preview = preview_expanded_canvas_rect(rect(50., 50., 80., 10.), 100., 80.).unwrap();
        assert_eq!(preview, rect(0., 0., 130., 80.));
        assert!(preview.x.is_sign_positive());
    }

    #[test]
    fn expand_canvas_to_fit_is_one_undoable_layer_edit() {
        let mut document = Document::new_capture("fixture:base", 100., 80., None);
        let mut shape = line("line", Vec::new());
        shape.base.x = -30.;
        shape.end_x = 50.;
        document.elements.push(Element::Shape(shape));
        let preview = canvas_expand_preview(&document.elements[1], 100., 80.)
            .unwrap()
            .unwrap();
        assert_eq!(preview.edges, [CanvasEdge::Left]);
        let before = document.clone();
        let mut history = crate::editor::DocumentHistory::new(document.clone());
        document
            .edit_layer("line", LayerEdit::ExpandCanvas)
            .unwrap();
        assert_eq!(document.width, 100. - preview.rect.x);
        assert!(
            canvas_expand_preview(&document.elements[1], document.width, document.height)
                .unwrap()
                .is_none()
        );
        // The base image moved with the shift, so nothing jumps on screen.
        assert_eq!(document.elements[0].base().x, -preview.rect.x);
        assert!(history.commit(document.clone()));
        assert!(history.undo());
        assert_eq!(history.current(), &before);
        // Fitting layers are a no-op; missing layers are an error.
        let mut fitted = document.clone();
        fitted.edit_layer("line", LayerEdit::ExpandCanvas).unwrap();
        assert_eq!(fitted, document);
        assert!(
            fitted
                .edit_layer("missing", LayerEdit::ExpandCanvas)
                .is_err()
        );
        // Hidden layers have no idle preview.
        let mut hidden = before.elements[1].clone();
        if let Element::Shape(shape) = &mut hidden {
            shape.base.visible = false;
        }
        assert!(canvas_expand_preview(&hidden, 100., 80.).unwrap().is_none());
    }

    #[test]
    fn curve_slider_matches_shipping_bend_and_straightens_near_zero() {
        let mut shape = line("arrow", Vec::new());
        apply_curve_edit(&mut shape, CurveEdit::Bend { bend: 0.25 }).unwrap();
        assert_eq!(shape.controls, [Point { x: 60., y: 45. }]);
        assert!((curve_bend_amount(&shape) - 0.25).abs() < 1e-12);
        let handles = curve_handles(&shape).unwrap();
        assert_eq!(handles.bend_percent, 25.);
        assert!(handles.slider && handles.starters.is_empty());
        assert_eq!(handles.straighten_label, "Straighten arrow");
        apply_curve_edit(&mut shape, CurveEdit::Bend { bend: 0.004 }).unwrap();
        assert!(shape.controls.is_empty());
        assert_eq!(curve_handles(&shape).unwrap().starters.len(), 3);
        assert!(apply_curve_edit(&mut shape, CurveEdit::Bend { bend: f64::NAN }).is_err());
    }

    #[test]
    fn curve_points_insert_remove_move_and_straighten() {
        let mut shape = line("line", Vec::new());
        // Dragging a starter dot materializes all three starters.
        apply_curve_edit(
            &mut shape,
            CurveEdit::Move {
                handle: CurveHandle::StarterControl { index: 1 },
                point: Point { x: 60., y: 50. },
            },
        )
        .unwrap();
        assert_eq!(
            shape.controls,
            [
                Point { x: 35., y: 20. },
                Point { x: 60., y: 50. },
                Point { x: 85., y: 20. }
            ]
        );
        let handles = curve_handles(&shape).unwrap();
        assert!(!handles.slider);
        assert_eq!(curve_bend_amount(&shape), 0.);
        // Insert lands in path order near the requested point.
        let (on_path, index, _) = closest_point_on_curve(&shape, Point { x: 100., y: 20. });
        assert_eq!(index, 3);
        apply_curve_edit(&mut shape, CurveEdit::Insert { point: on_path }).unwrap();
        assert_eq!(shape.controls.len(), 4);
        assert_eq!(shape.controls[3], on_path);
        apply_curve_edit(&mut shape, CurveEdit::Remove { index: 0 }).unwrap();
        assert_eq!(shape.controls.len(), 3);
        apply_curve_edit(&mut shape, CurveEdit::Remove { index: 9 }).unwrap();
        assert_eq!(shape.controls.len(), 3);
        apply_curve_edit(
            &mut shape,
            CurveEdit::Move {
                handle: CurveHandle::Control { index: 0 },
                point: Point { x: 61., y: 70. },
            },
        )
        .unwrap();
        assert_eq!(shape.controls[0], Point { x: 61., y: 70. });
        apply_curve_edit(&mut shape, CurveEdit::Straighten).unwrap();
        assert!(shape.controls.is_empty());
        // Closed shapes ignore curve edits.
        let mut rectangle = line("rectangle", Vec::new());
        apply_curve_edit(&mut rectangle, CurveEdit::Bend { bend: 0.5 }).unwrap();
        assert!(rectangle.controls.is_empty());
        assert!(curve_handles(&rectangle).is_none());
    }

    #[test]
    fn shortening_an_arrow_thins_it_and_endpoints_move() {
        let mut shape = line("arrow", Vec::new());
        shape.style.stroke_width = 10.;
        apply_curve_edit(
            &mut shape,
            CurveEdit::Move {
                handle: CurveHandle::End,
                point: Point { x: 60., y: 20. },
            },
        )
        .unwrap();
        assert_eq!((shape.end_x, shape.end_y), (60., 20.));
        assert_eq!(shape.style.stroke_width, 5.);
        let mut plain = line("line", Vec::new());
        plain.style.stroke_width = 10.;
        apply_curve_edit(
            &mut plain,
            CurveEdit::Move {
                handle: CurveHandle::Start,
                point: Point { x: 100., y: 20. },
            },
        )
        .unwrap();
        assert_eq!((plain.base.x, plain.style.stroke_width), (100., 10.));
    }

    #[test]
    fn handle_hits_and_hints_follow_shipping_priority() {
        let shape = line("line", Vec::new());
        assert_eq!(
            hit_test_curve_handle(&shape, Point { x: 11., y: 21. }, 6.),
            Some(CurveHandle::Start)
        );
        assert_eq!(
            hit_test_curve_handle(&shape, Point { x: 60., y: 26. }, 6.),
            Some(CurveHandle::StarterControl { index: 1 })
        );
        assert_eq!(
            curve_hover_hint(&shape, Point { x: 60., y: 22. }, 6.),
            Some("Drag a dot to curve · Double-click the path to add points")
        );
        assert_eq!(
            curve_hover_hint(&shape, Point { x: 47., y: 22. }, 6.),
            Some("Double-click to add a curve point")
        );
        assert_eq!(
            curve_hover_hint(&shape, Point { x: 110., y: 20. }, 6.),
            Some("Drag to move endpoint")
        );
        assert_eq!(curve_hover_hint(&shape, Point { x: 60., y: 80. }, 6.), None);
        let curved = line("line", vec![Point { x: 60., y: 50. }]);
        assert_eq!(
            curve_hover_hint(&curved, Point { x: 60., y: 50. }, 6.),
            Some("Double-click to remove curve point")
        );
        let mut locked = curved.clone();
        locked.base.locked = true;
        assert_eq!(
            curve_hover_hint(&locked, Point { x: 60., y: 50. }, 6.),
            None
        );
    }

    #[test]
    fn rotated_curves_hit_in_local_space_and_keep_the_start_in_place() {
        let mut shape = line("line", Vec::new());
        shape.base.rotation = Some(std::f64::consts::FRAC_PI_2);
        let handles = curve_handles(&shape).unwrap();
        // The middle starter sits near the pivot; the ends swing vertical.
        assert!((handles.starters[1].x - 60.).abs() < 1.);
        assert!((handles.start.x - handles.end.x).abs() < 1e-9);
        assert!((handles.end.y - handles.start.y - 100.).abs() < 1e-9);
        assert_eq!(
            hit_test_curve_handle(&shape, handles.start, 4.),
            Some(CurveHandle::Start)
        );
        let start_before = handles.start;
        apply_curve_edit(&mut shape, CurveEdit::Bend { bend: 0.5 }).unwrap();
        let start_after = curve_handles(&shape).unwrap().start;
        assert!((start_before.x - start_after.x).abs() < 1e-6);
        assert!((start_before.y - start_after.y).abs() < 1e-6);
    }

    #[test]
    fn curved_line_round_trips_through_layer_edit_and_json() {
        let mut document = Document::new_capture("fixture:base", 200., 120., None);
        document
            .elements
            .push(Element::Shape(line("arrow", Vec::new())));
        document
            .edit_layer(
                "line",
                LayerEdit::Curve {
                    edit: CurveEdit::Bend { bend: -0.4 },
                },
            )
            .unwrap();
        let json = serde_json::to_string(&document).unwrap();
        assert!(json.contains("\"controls\":[{\"x\":60.0,\"y\":-20.0}]"));
        let reopened: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(reopened, document);
        let edit: LayerEdit = serde_json::from_value(serde_json::json!({
            "action": "curve",
            "edit": {"kind": "move", "handle": {"kind": "starter_control", "index": 2},
                     "point": {"x": 1.0, "y": 2.0}}
        }))
        .unwrap();
        assert!(matches!(edit, LayerEdit::Curve { .. }));
        // Locked layers keep their geometry.
        let mut locked = reopened.clone();
        locked
            .edit_layer("line", LayerEdit::Lock { locked: true })
            .unwrap();
        let before = locked.clone();
        locked
            .edit_layer(
                "line",
                LayerEdit::Curve {
                    edit: CurveEdit::Straighten,
                },
            )
            .unwrap();
        assert_eq!(locked, before);
    }
}
