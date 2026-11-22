//! Shipping's selection chrome over the screenshot canvas
//! (`ScreenshotEditor.tsx` `drawEditorOverlays`, `hitTestSelectedAnnotation`):
//! the dashed bounds with resize grips, the rotation handle, line/arrow curve
//! dots, the Expand canvas overflow tint, ghost and edge bloom, and the Trim
//! edges hover preview. Geometry comes from `captures_app::editor` /
//! `editor_canvas`; this module only hit tests in document space and paints
//! small Cairo nodes in screen space.
use std::f64::consts::{PI, TAU};

use captures_app::editor::{
    Element, Point, ResizeHandle, TextMeasurer, rotation_angle, rotation_handle,
};
use captures_app::editor_canvas::{
    self, CanvasEdge, CanvasExpandPreview, CanvasTrimPreview, CurveHandle, SNAP_BLOOM,
    SNAP_BLOOM_OVERHANG, SNAP_BLOOM_STOPS, SNAP_EDGE_BAR, TRIM_BLOOM, TRIM_BLOOM_STOPS,
    TRIM_EDGE_BAR, TRIM_KEEP_ALPHA, TRIM_KEEP_GLOW_ALPHA, TRIM_KEEP_RADIUS, TRIM_KEEP_WIDTH,
    TRIM_REGION_ALPHA, TRIM_RGB,
};
use captures_app::editor_session::{ImageDropGuide, ImportPlacement};
use gtk::prelude::*;
use gtk::{cairo, gdk, graphene, gsk};

/// Shipping's pointer slop for grips: 10 screen pixels.
pub(super) const HIT_PX: f64 = 10.;
/// Shipping's Shift stop for rotation (`SHAPE_ROTATION_SNAP_DEGREES`).
pub(super) const ROTATION_SNAP_DEGREES: f64 = 15.;

/// A grip under the pointer on the selected layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Hit {
    Rotate,
    Resize(ResizeHandle),
    Curve(CurveHandle),
}

fn is_corner(handle: ResizeHandle) -> bool {
    matches!(
        handle,
        ResizeHandle::Nw | ResizeHandle::Ne | ResizeHandle::Se | ResizeHandle::Sw
    )
}

fn curveable(element: &Element) -> Option<&captures_app::editor::ShapeElement> {
    match element {
        Element::Shape(shape) if editor_canvas::is_curveable(shape) => Some(shape),
        _ => None,
    }
}

/// Shipping `hitTestSelectedAnnotation`: the rotation grip, then corner
/// grips (so shrinking an arrow scales its head too), then curve dots, then
/// edge grips. Locked and hidden layers have no grips.
pub(super) fn hit(
    element: &Element,
    outline: [Point; 4],
    point: Point,
    scale: f64,
    canvas: (f64, f64),
    measure: &mut TextMeasurer<'_>,
) -> Option<Hit> {
    let base = element.base();
    if base.locked || !base.visible {
        return None;
    }
    let radius = HIT_PX / scale;
    if let Some(grip) = rotation_handle(outline, base.rotation(), scale, canvas.0, canvas.1)
        && (point.x - grip.handle.x).hypot(point.y - grip.handle.y) <= grip.hit_radius
    {
        return Some(Hit::Rotate);
    }
    let handle = element
        .resize_handle_at_with(point, radius, measure)
        .ok()
        .flatten();
    if let Some(handle) = handle.filter(|handle| is_corner(*handle)) {
        return Some(Hit::Resize(handle));
    }
    if let Some(shape) = curveable(element)
        && let Some(handle) = editor_canvas::hit_test_curve_handle(shape, point, radius)
    {
        return Some(Hit::Curve(handle));
    }
    // Lines and arrows show corner grips only; their edges belong to the dots.
    handle
        .filter(|_| curveable(element).is_none())
        .map(Hit::Resize)
}

/// Shipping `resizeCursor`: the handle's direction turned with the layer.
pub(super) fn cursor(hit: Hit, rotation: f64) -> &'static str {
    match hit {
        Hit::Rotate => "grab",
        Hit::Curve(CurveHandle::Start | CurveHandle::End) => "move",
        Hit::Curve(_) => "grab",
        Hit::Resize(handle) => {
            let steps = (rotation / (PI / 4.)).round() as i64;
            match (handle as i64 + steps).rem_euclid(8) {
                0 | 4 => "nwse-resize",
                1 | 5 => "ns-resize",
                2 | 6 => "nesw-resize",
                _ => "ew-resize",
            }
        }
    }
}

/// Shipping's hover tips over grips and line/arrow paths.
pub(super) fn hint(
    element: &Element,
    hit: Option<Hit>,
    point: Point,
    scale: f64,
) -> Option<String> {
    if hit == Some(Hit::Rotate) {
        return Some(format!(
            "Drag to rotate. Hold Shift to snap by {ROTATION_SNAP_DEGREES}°"
        ));
    }
    let shape = curveable(element)?;
    editor_canvas::curve_hover_hint(shape, point, HIT_PX / scale).map(str::to_owned)
}

/// The Shift-snapped rotation for a rotate drag, or `None` when unchanged.
pub(super) fn rotation(
    outline: [Point; 4],
    initial: f64,
    start: Point,
    current: Point,
    shift: bool,
) -> Option<f64> {
    let preview = captures_app::editor::preview_rotation(
        outline,
        initial,
        start,
        current,
        shift.then_some(ROTATION_SNAP_DEGREES),
    )?;
    rotation_angle(preview.radians, None)
}

fn cairo_for(snapshot: &gtk::Snapshot, points: &[(f64, f64)], pad: f64) -> Option<cairo::Context> {
    if points.is_empty() {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in points {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    Some(snapshot.append_cairo(&graphene::Rect::new(
        (x0 - pad) as f32,
        (y0 - pad) as f32,
        (x1 - x0 + 2. * pad) as f32,
        (y1 - y0 + 2. * pad) as f32,
    )))
}

fn source(cr: &cairo::Context, color: gdk::RGBA, alpha: f64) {
    cr.set_source_rgba(
        color.red().into(),
        color.green().into(),
        color.blue().into(),
        f64::from(color.alpha()) * alpha,
    );
}

/// An accent square grip with a white rim, turned with the layer.
fn grip(cr: &cairo::Context, (x, y): (f64, f64), angle: f64, accent: gdk::RGBA) {
    let size = 5.5;
    cr.save().ok();
    cr.translate(x, y);
    cr.rotate(angle);
    cr.rectangle(-size / 2., -size / 2., size, size);
    source(cr, accent, 0.88);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1., 1., 1., 0.9 * 0.88);
    cr.set_line_width(1.1);
    let _ = cr.stroke();
    cr.restore().ok();
}

fn midpoint(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    ((a.0 + b.0) / 2., (a.1 + b.1) / 2.)
}

/// The selected layer's dashed bounds and, when `grips`, its resize grips
/// (corners only on lines and arrows). `outline` is in screen pixels.
pub(super) fn selection(
    snapshot: &gtk::Snapshot,
    outline: &[(f64, f64); 4],
    curve: bool,
    grips: bool,
    accent: gdk::RGBA,
) {
    let Some(cr) = cairo_for(snapshot, outline, 8.) else {
        return;
    };
    cr.move_to(outline[0].0, outline[0].1);
    for point in &outline[1..] {
        cr.line_to(point.0, point.1);
    }
    cr.close_path();
    source(&cr, accent, if curve { 0.55 } else { 0.9 });
    cr.set_line_width(if curve { 1.25 } else { 2. });
    cr.set_dash(&[6., 4.], 0.);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.);
    if !grips {
        return;
    }
    let angle = (outline[1].1 - outline[0].1).atan2(outline[1].0 - outline[0].0);
    let mut points: Vec<(f64, f64)> = outline.to_vec();
    if !curve {
        for index in 0..4 {
            points.push(midpoint(outline[index], outline[(index + 1) % 4]));
        }
    }
    for point in points {
        grip(&cr, point, angle, accent);
    }
}

/// Shipping `drawShapeRotationHandle`: a stem from the bounds to a tinted
/// ring with a small rotate glyph.
pub(super) fn rotation_grip(
    snapshot: &gtk::Snapshot,
    anchor: (f64, f64),
    handle: (f64, f64),
    accent: gdk::RGBA,
) {
    let Some(cr) = cairo_for(snapshot, &[anchor, handle], 12.) else {
        return;
    };
    let radius = 8.;
    let (x, y) = handle;
    cr.arc(x, y, radius, 0., TAU);
    source(&cr, accent, 0.22);
    let _ = cr.fill();
    source(&cr, accent, 1.);
    cr.set_line_width(1.6);
    cr.move_to(anchor.0, anchor.1);
    cr.line_to(x, y);
    let _ = cr.stroke();
    cr.arc(x, y, radius, 0., TAU);
    let _ = cr.stroke();
    cr.set_line_width(1.35);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    let glyph = 3.15;
    cr.arc(x, y, glyph, PI * 0.35, PI * 1.75);
    let _ = cr.stroke();
    let tip = PI * 1.75;
    let (tx, ty) = (x + tip.cos() * glyph, y + tip.sin() * glyph);
    let arrow = 2.15;
    cr.move_to(tx, ty);
    cr.line_to(tx - arrow * 0.85, ty - arrow * 0.15);
    cr.line_to(tx - arrow * 0.1, ty + arrow * 0.9);
    cr.close_path();
    let _ = cr.fill();
}

/// Shipping's line/arrow grips: square endpoints, round control dots (with
/// a dashed stem for a single bend), or three starter dots while straight.
/// Points are in screen pixels.
pub(super) fn curve(
    snapshot: &gtk::Snapshot,
    ends: [(f64, f64); 2],
    controls: &[(f64, f64)],
    starters: &[(f64, f64)],
    angle: f64,
    accent: gdk::RGBA,
) {
    let all: Vec<_> = ends
        .iter()
        .chain(controls)
        .chain(starters)
        .copied()
        .collect();
    let Some(cr) = cairo_for(snapshot, &all, 8.) else {
        return;
    };
    let dot = |point: (f64, f64)| {
        cr.arc(point.0, point.1, 4.5, 0., TAU);
        cr.set_source_rgba(1., 1., 1., 0.94 * 0.82);
        let _ = cr.fill_preserve();
        source(&cr, accent, 0.82);
        cr.set_line_width(1.15);
        let _ = cr.stroke();
    };
    if let [control] = controls {
        let from = midpoint(ends[0], ends[1]);
        source(&cr, accent, 0.45);
        cr.set_line_width(1.15);
        cr.set_dash(&[3., 3.], 0.);
        cr.move_to(from.0, from.1);
        cr.line_to(control.0, control.1);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.);
    }
    for end in ends {
        grip(&cr, end, angle, accent);
    }
    for point in controls.iter().chain(starters) {
        dot(*point);
    }
}

/// The Expand canvas overflow, in screen pixels: an accent tint over each
/// off-canvas gap and, when `armed` (the action is hovered), the dashed
/// ghost of the grown canvas with a bloom outside every crossed edge.
pub(super) fn expand(
    snapshot: &gtk::Snapshot,
    preview: &CanvasExpandPreview,
    to_screen: &dyn Fn(Point) -> Option<(f64, f64)>,
    armed: bool,
    accent: gdk::RGBA,
) {
    let rect = |r: captures_app::editor::Rect| {
        let a = to_screen(Point { x: r.x, y: r.y })?;
        let b = to_screen(Point {
            x: r.x + r.width,
            y: r.y + r.height,
        })?;
        Some((a.0, a.1, b.0 - a.0, b.1 - a.1))
    };
    for gap in preview.gaps.iter().filter_map(|gap| rect(*gap)) {
        let bounds = graphene::Rect::new(gap.0 as f32, gap.1 as f32, gap.2 as f32, gap.3 as f32);
        let mut tint = accent;
        tint.set_alpha(0.16);
        snapshot.append_color(&tint, &bounds);
        let mut border = accent;
        border.set_alpha(0.5);
        snapshot.append_border(
            &gtk::gsk::RoundedRect::from_rect(bounds, 0.),
            &[1.; 4],
            &[border; 4],
        );
    }
    if !armed {
        return;
    }
    let Some((x, y, w, h)) = rect(preview.rect) else {
        return;
    };
    let crossed = |edge| preview.edges.contains(&edge);
    let glow = Glow {
        color: accent,
        stops: SNAP_BLOOM_STOPS,
        depth: SNAP_BLOOM,
        overhang: 0.,
        bar: SNAP_EDGE_BAR,
        halo: SNAP_HALO,
    };
    for edge in &preview.edges {
        bloom(snapshot, (x, y, w, h), *edge, &glow);
    }
    let Some(cr) = cairo_for(snapshot, &[(x, y), (x + w, y + h)], 4.) else {
        return;
    };
    for (edge, from, to) in [
        (CanvasEdge::Top, (x, y), (x + w, y)),
        (CanvasEdge::Right, (x + w, y), (x + w, y + h)),
        (CanvasEdge::Bottom, (x + w, y + h), (x, y + h)),
        (CanvasEdge::Left, (x, y + h), (x, y)),
    ] {
        let on = crossed(edge);
        source(&cr, accent, if on { 0.82 } else { 0.42 });
        cr.set_line_width(if on { 2. } else { 1.5 });
        cr.set_dash(&[6., 4.], 0.);
        cr.move_to(from.0, from.1);
        cr.line_to(to.0, to.1);
        let _ = cr.stroke();
    }
}

/// An edge glow: shipping's `.screenshot-canvas-expand-edge` (accent) and
/// `.screenshot-canvas-trim-edge` (red) share this shape.
struct Glow {
    color: gdk::RGBA,
    stops: [(f64, f64); 4],
    /// The bloom's depth, and how far it overhangs each end of the edge as a
    /// fraction of its length.
    depth: f64,
    overhang: f64,
    /// The pill's thickness, straddling the edge.
    bar: f64,
    /// The pill's `box-shadow` glows: blur radius and alpha.
    halo: &'static [(f32, f32)],
}

/// `.screenshot-drop-snap-guide::after` / `.screenshot-canvas-expand-edge::after`
/// glows.
const SNAP_HALO: &[(f32, f32)] = &[(8., 0.95), (20., 0.65), (36., 0.4)];

/// The pill on a rectangle's `edge` and its outward bloom, faded at both ends.
fn bloom(
    snapshot: &gtk::Snapshot,
    (x, y, w, h): (f64, f64, f64, f64),
    edge: CanvasEdge,
    glow: &Glow,
) {
    let depth = glow.depth;
    let (ox, oy) = (w * glow.overhang, h * glow.overhang);
    let (area, from, to) = match edge {
        CanvasEdge::Top => (
            (x - ox, y - depth, w + 2. * ox, depth),
            (x, y),
            (x, y - depth),
        ),
        CanvasEdge::Bottom => (
            (x - ox, y + h, w + 2. * ox, depth),
            (x, y + h),
            (x, y + h + depth),
        ),
        CanvasEdge::Left => (
            (x - depth, y - oy, depth, h + 2. * oy),
            (x, y),
            (x - depth, y),
        ),
        CanvasEdge::Right => (
            (x + w, y - oy, depth, h + 2. * oy),
            (x + w, y),
            (x + w + depth, y),
        ),
    };
    let color = glow.color;
    let cr = snapshot.append_cairo(&graphene::Rect::new(
        area.0 as f32,
        area.1 as f32,
        area.2 as f32,
        area.3 as f32,
    ));
    let gradient = cairo::LinearGradient::new(from.0, from.1, to.0, to.1);
    for (offset, alpha) in glow.stops {
        gradient.add_color_stop_rgba(
            offset,
            color.red().into(),
            color.green().into(),
            color.blue().into(),
            alpha,
        );
    }
    // Fade along the edge: transparent, full from 12% to 88%, transparent.
    let horizontal = matches!(edge, CanvasEdge::Top | CanvasEdge::Bottom);
    let mask = if horizontal {
        cairo::LinearGradient::new(area.0, 0., area.0 + area.2, 0.)
    } else {
        cairo::LinearGradient::new(0., area.1, 0., area.1 + area.3)
    };
    for (offset, alpha) in [(0., 0.), (0.12, 1.), (0.88, 1.), (1., 0.)] {
        mask.add_color_stop_rgba(offset, 0., 0., 0., alpha);
    }
    let _ = cr.set_source(&gradient);
    let _ = cr.mask(&mask);
    drop(cr);
    // The pill straddling the edge, 1 px past each end, with its glows.
    let bar = glow.bar;
    let (bx, by, bw, bh) = if horizontal {
        (x - 1., from.1 - bar / 2., w + 2., bar)
    } else {
        (from.0 - bar / 2., y - 1., bar, h + 2.)
    };
    let radius = (bar / 2.) as f32;
    let pill = gsk::RoundedRect::from_rect(
        graphene::Rect::new(bx as f32, by as f32, bw as f32, bh as f32),
        radius,
    );
    for (blur, alpha) in glow.halo {
        let mut shadow = color;
        shadow.set_alpha(*alpha);
        snapshot.append_outset_shadow(&pill, &shadow, 0., 0., 0., *blur);
    }
    snapshot.push_rounded_clip(&pill);
    snapshot.append_color(&color, pill.bounds());
    snapshot.pop();
}

/// Shipping's Trim edges hover preview (`.screenshot-canvas-trim-hint`), in
/// screen pixels: the discarded margins tinted red, the dashed rectangle that
/// stays, and a red pill with an outward bloom on each cut edge.
pub(super) fn trim(
    snapshot: &gtk::Snapshot,
    preview: &CanvasTrimPreview,
    to_screen: &dyn Fn(Point) -> Option<(f64, f64)>,
) {
    let rect = |r: captures_app::editor::Rect| {
        let a = to_screen(Point { x: r.x, y: r.y })?;
        let b = to_screen(Point {
            x: r.x + r.width,
            y: r.y + r.height,
        })?;
        Some((a.0, a.1, b.0 - a.0, b.1 - a.1))
    };
    let [r, g, b] = TRIM_RGB.map(|c| f32::from(c) / 255.);
    let red = |alpha: f64| gdk::RGBA::new(r, g, b, alpha as f32);
    for (_, region) in &preview.regions {
        let Some((x, y, w, h)) = rect(*region) else {
            continue;
        };
        let bounds = graphene::Rect::new(x as f32, y as f32, w as f32, h as f32);
        snapshot.append_color(&red(TRIM_REGION_ALPHA), &bounds);
        // `box-shadow: inset 0 0 24px rgba(trim, .12)`.
        snapshot.append_inset_shadow(
            &gsk::RoundedRect::from_rect(bounds, 0.),
            &red(0.12),
            0.,
            0.,
            0.,
            24.,
        );
    }
    let Some((x, y, w, h)) = rect(preview.keep) else {
        return;
    };
    // `.screenshot-canvas-trim-keep`: `0 0 0 1px .1, 0 0 22px .18` glows
    // and a 1.5 px dashed border with a 3 px radius.
    let keep = gsk::RoundedRect::from_rect(
        graphene::Rect::new(x as f32, y as f32, w as f32, h as f32),
        TRIM_KEEP_RADIUS as f32,
    );
    snapshot.append_outset_shadow(&keep, &red(0.1), 0., 0., 1., 0.);
    snapshot.append_outset_shadow(&keep, &red(TRIM_KEEP_GLOW_ALPHA), 0., 0., 0., 22.);
    if let Some(cr) = cairo_for(snapshot, &[(x, y), (x + w, y + h)], 2.) {
        let inset = TRIM_KEEP_WIDTH / 2.;
        let (x, y, w, h) = (x + inset, y + inset, w - 2. * inset, h - 2. * inset);
        let radius = (TRIM_KEEP_RADIUS - inset).max(0.);
        cr.new_sub_path();
        cr.arc(x + w - radius, y + radius, radius, -PI / 2., 0.);
        cr.arc(x + w - radius, y + h - radius, radius, 0., PI / 2.);
        cr.arc(x + radius, y + h - radius, radius, PI / 2., PI);
        cr.arc(x + radius, y + radius, radius, PI, 3. * PI / 2.);
        cr.close_path();
        source(&cr, red(TRIM_KEEP_ALPHA), 1.);
        cr.set_line_width(TRIM_KEEP_WIDTH);
        // A CSS dashed border's dashes are about three widths long.
        cr.set_dash(&[3. * TRIM_KEEP_WIDTH, 3. * TRIM_KEEP_WIDTH], 0.);
        let _ = cr.stroke();
    }
    let glow = Glow {
        color: red(1.),
        stops: TRIM_BLOOM_STOPS,
        depth: TRIM_BLOOM,
        overhang: 0.,
        bar: TRIM_EDGE_BAR,
        halo: &[(8., 0.95), (20., 0.55), (32., 0.32)],
    };
    for edge in &preview.edges {
        bloom(snapshot, (x, y, w, h), *edge, &glow);
    }
}

/// Shipping's image drop guide (`.screenshot-drop-snap-guide`), in screen
/// pixels: the accent target an edge drop joins, with a glowing pill and
/// bloom on that edge, or the stack light where a stacked image lands.
pub(super) fn drop_guide(
    snapshot: &gtk::Snapshot,
    guide: &ImageDropGuide,
    to_screen: &dyn Fn(Point) -> Option<(f64, f64)>,
    accent: gdk::RGBA,
    radius: f32,
) {
    let rect = |r: captures_app::editor::Rect| {
        let a = to_screen(Point { x: r.x, y: r.y })?;
        let b = to_screen(Point {
            x: r.x + r.width,
            y: r.y + r.height,
        })?;
        Some(graphene::Rect::new(
            a.0 as f32,
            a.1 as f32,
            (b.0 - a.0) as f32,
            (b.1 - a.1) as f32,
        ))
    };
    let tint = |alpha: f32| {
        let mut color = accent;
        color.set_alpha(alpha);
        color
    };
    let edge = match guide.placement {
        ImportPlacement::Top => CanvasEdge::Top,
        ImportPlacement::Right => CanvasEdge::Right,
        ImportPlacement::Bottom => CanvasEdge::Bottom,
        ImportPlacement::Left => CanvasEdge::Left,
        ImportPlacement::Stack => {
            if let Some(focus) = rect(guide.focus) {
                stack_light(snapshot, focus, radius);
            }
            return;
        }
    };
    let Some(target) = rect(guide.target) else {
        return;
    };
    // `border: 1px .78; background: .1; box-shadow: 0 0 0 1px rgba(0,0,0,.35),
    // 0 0 22px .28, inset 0 0 28px .08`, with a 3 px radius.
    let outline = gsk::RoundedRect::from_rect(target, 3.);
    snapshot.append_outset_shadow(&outline, &gdk::RGBA::new(0., 0., 0., 0.35), 0., 0., 1., 0.);
    snapshot.append_outset_shadow(&outline, &tint(0.28), 0., 0., 0., 22.);
    snapshot.push_rounded_clip(&outline);
    snapshot.append_color(&tint(0.1), &target);
    snapshot.pop();
    snapshot.append_inset_shadow(&outline, &tint(0.08), 0., 0., 0., 28.);
    snapshot.append_border(&outline, &[1.; 4], &[tint(0.78); 4]);
    let across = match edge {
        CanvasEdge::Top | CanvasEdge::Bottom => target.height(),
        CanvasEdge::Left | CanvasEdge::Right => target.width(),
    };
    let glow = Glow {
        color: accent,
        stops: SNAP_BLOOM_STOPS,
        depth: editor_canvas::drop_bloom_depth(f64::from(across)),
        overhang: SNAP_BLOOM_OVERHANG,
        bar: SNAP_EDGE_BAR,
        halo: SNAP_HALO,
    };
    bloom(
        snapshot,
        (
            f64::from(target.x()),
            f64::from(target.y()),
            f64::from(target.width()),
            f64::from(target.height()),
        ),
        edge,
        &glow,
    );
}

/// `StackDropLight`: a warm pool, a contact shadow and a thin white rim on
/// the footprint a stacked image will cover.
fn stack_light(snapshot: &gtk::Snapshot, focus: graphene::Rect, radius: f32) {
    let (w, h) = (focus.width(), focus.height());
    // `.screenshot-drop-snap-stack-pool`: inset -48% -62% -72% -62%, an
    // ellipse at 50% 40% at .86 opacity.
    let pool = graphene::Rect::new(
        focus.x() - 0.62 * w,
        focus.y() - 0.48 * h,
        2.24 * w,
        2.2 * h,
    );
    let warm = |alpha: f32| gdk::RGBA::new(1., 0.957, 0.894, alpha * 0.86);
    snapshot.append_radial_gradient(
        &pool,
        &graphene::Point::new(pool.x() + pool.width() / 2., pool.y() + pool.height() * 0.4),
        pool.width() / 2. * std::f32::consts::SQRT_2,
        pool.height() / 2. * std::f32::consts::SQRT_2,
        0.,
        1.,
        &[
            gsk::ColorStop::new(0., gdk::RGBA::new(1., 0.98, 0.941, 0.3 * 0.86)),
            gsk::ColorStop::new(0.34, warm(0.14)),
            gsk::ColorStop::new(0.56, warm(0.05)),
            gsk::ColorStop::new(0.74, warm(0.)),
        ],
    );
    let plate = gsk::RoundedRect::from_rect(focus, radius);
    // `.screenshot-drop-snap-stack-shadow` at .94 opacity.
    for (y, blur, alpha) in [
        (2., 4., 0.2),
        (8., 18., 0.3),
        (18., 40., 0.24),
        (28., 56., 0.12),
    ] {
        let shadow = gdk::RGBA::new(0., 0., 0., alpha * 0.94);
        snapshot.append_outset_shadow(&plate, &shadow, 0., y, 0., blur);
    }
    // `.screenshot-drop-snap-stack-rim` at .9 opacity.
    let white = |alpha: f32| gdk::RGBA::new(1., 1., 1., alpha * 0.9);
    snapshot.append_outset_shadow(&plate, &white(0.1), 0., 0., 0., 10.);
    snapshot.append_outset_shadow(&plate, &white(0.36), 0., 0., 1., 0.);
    snapshot.append_inset_shadow(&plate, &white(0.08), 0., 0., 1., 0.);
}
