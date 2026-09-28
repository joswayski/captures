//! Shipping canvas interactions (`ScreenshotEditor.tsx`, `styles/editor-image.css`):
//! image file drops with the placement guide, toast and snap light; the
//! "Expand canvas" overflow ghost and action; and line/arrow curve chrome.
//! Geometry and copy come from `captures_app::editor_canvas` and
//! `editor_session::image_drop_guide`, shared with the AppKit host.
use super::*;
use captures_app::{
    editor_canvas::{
        self as canvas, CanvasEdge, CanvasExpandPreview, CurveEdit, CurveHandle, CurveHandles,
    },
    editor_image_background::{self as image_background, WandLoupe},
    editor_session::{
        DROP_IMAGE, DROP_UNSUPPORTED, ImageDropGuide, ImportPlacement, image_drop_guide,
        is_supported_image_path,
    },
    motion::Motion,
};
use eframe::egui::{Color32, FontFamily, FontId, Stroke, StrokeKind, pos2, vec2};

/// Queued dropped files and the live hover state. Files import one at a time
/// through the worker, preserving the one-in-flight edit contract.
#[derive(Default)]
pub(super) struct DropState {
    pub hovering: bool,
    /// `input.time` when the current hover began: the drop guide's bloom,
    /// edge pulse and particles run from here.
    pub since: Option<f64>,
    pub queue: std::collections::VecDeque<(PathBuf, Option<Point>)>,
}

fn accent(tokens: &Tokens, alpha: f32) -> Color32 {
    tokens.color("theme-accent").gamma_multiply(alpha)
}

fn project(preview: egui::Rect, bounds: Rect, point: Point) -> egui::Pos2 {
    pos2(
        preview.left() + (point.x / bounds.width) as f32 * preview.width(),
        preview.top() + (point.y / bounds.height) as f32 * preview.height(),
    )
}

fn project_rect(preview: egui::Rect, bounds: Rect, rect: Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        project(
            preview,
            bounds,
            Point {
                x: rect.x,
                y: rect.y,
            },
        ),
        project(
            preview,
            bounds,
            Point {
                x: rect.x + rect.width,
                y: rect.y + rect.height,
            },
        ),
    )
}

fn document_bounds(view: &View) -> Option<Rect> {
    let presented = view.presented.as_ref()?;
    Some(Rect {
        x: 0.,
        y: 0.,
        width: f64::from(presented.pixels.width()),
        height: f64::from(presented.pixels.height()),
    })
}

/// Read egui's hovered/dropped files once per frame (first pass only: later
/// passes receive empty raw input). Drops land at the last pointer sample over
/// the canvas; without one, shipping's default guide places the image below.
pub(super) fn receive_drops(ctx: &egui::Context, view: &mut View, preview: Option<egui::Rect>) {
    if ctx.current_pass_index() != 0 {
        return;
    }
    let (hovering, dropped, pointer, now) = ctx.input(|input| {
        (
            !input.raw.hovered_files.is_empty(),
            input.raw.dropped_files.clone(),
            input.pointer.latest_pos(),
            input.time,
        )
    });
    let accepting = view.presented.is_some() && !view.closed && !view.close_requested;
    view.drop.hovering = hovering && accepting;
    view.drop.since = view.drop.hovering.then(|| view.drop.since.unwrap_or(now));
    if dropped.is_empty() || !accepting {
        return;
    }
    let point = preview
        .zip(document_bounds(view))
        .zip(pointer)
        .filter(|((preview, _), pointer)| preview.contains(*pointer))
        .map(|((preview, bounds), pointer)| image_point(pointer, preview, bounds));
    let paths: Vec<PathBuf> = dropped
        .into_iter()
        .map(|file| file.path().to_path_buf())
        .filter(|path| is_supported_image_path(path))
        .collect();
    if paths.is_empty() {
        view.error = Some(DROP_UNSUPPORTED.into());
        return;
    }
    view.drop.hovering = false;
    view.drop.since = None;
    for (index, path) in paths.into_iter().enumerate() {
        view.drop
            .queue
            .push_back((path, if index == 0 { point } else { None }));
    }
}

/// Submit the next dropped file once no other edit is in flight. Later files
/// in one drop stack below the layer the previous file created.
pub(super) fn drain_drops(view: &mut View, tx: &Sender<Job>) -> bool {
    if view.closed || view.close_requested {
        view.drop.queue.clear();
        return false;
    }
    if view.pending || view.inline.is_some() {
        return false;
    }
    let Some((path, point)) = view.drop.queue.pop_front() else {
        return false;
    };
    view.submit_job(
        tx,
        Job::Import {
            path,
            selected_id: view.selected_layer.clone(),
            point,
        },
    )
}

/// The live guide for the current pointer sample.
pub(super) fn drop_guide(view: &View, pointer: Option<Point>) -> Option<ImageDropGuide> {
    let presented = view.presented.as_ref()?;
    Some(image_drop_guide(
        &presented.document,
        view.selected_layer.as_deref(),
        pointer,
    ))
}

/// Paint the shipping drop snap guide, edge bloom or stack light, and toast.
pub(super) fn paint_drop_guide(
    ui: &egui::Ui,
    tokens: &Tokens,
    view: &View,
    available: egui::Rect,
    preview: egui::Rect,
) {
    if !view.drop.hovering {
        return;
    }
    let Some(bounds) = document_bounds(view) else {
        return;
    };
    let pointer = ui
        .input(|input| input.pointer.latest_pos())
        .filter(|pointer| preview.contains(*pointer))
        .map(|pointer| image_point(pointer, preview, bounds));
    let painter = ui
        .painter()
        .with_clip_rect(available.intersect(ui.clip_rect()));
    let reduced = crate::motion::reduced(ui.ctx());
    let now = ui.input(|input| input.time);
    let elapsed = (now - view.drop.since.unwrap_or(now)) * 1000.;
    let label = match drop_guide(view, pointer) {
        Some(guide) => {
            let target = project_rect(preview, bounds, guide.target);
            if guide.placement == ImportPlacement::Stack {
                paint_stack_light(&painter, project_rect(preview, bounds, guide.focus));
            } else {
                painter.rect(
                    target,
                    3.,
                    accent(tokens, 0.1),
                    Stroke::new(1., accent(tokens, 0.78)),
                    StrokeKind::Inside,
                );
                if let Some(edge) = placement_edge(guide.placement) {
                    let across = match edge {
                        CanvasEdge::Top | CanvasEdge::Bottom => target.height(),
                        CanvasEdge::Left | CanvasEdge::Right => target.width(),
                    };
                    let glow = EdgeGlow {
                        depth: canvas::drop_bloom_depth(f64::from(across)) as f32,
                        overhang: canvas::SNAP_BLOOM_OVERHANG as f32,
                    };
                    glow.paint(&painter, tokens, target, edge, elapsed, reduced);
                    if !reduced {
                        ui.ctx().request_repaint();
                    }
                }
            }
            guide.label
        }
        None => DROP_IMAGE,
    };
    // `.screenshot-drop-overlay`: a glass toast with an accent rim.
    let font = FontId::new(
        tokens.number("text-sm"),
        FontFamily::Name("semibold".into()),
    );
    let text = painter.layout_no_wrap(label.to_owned(), font, tokens.color("glass-text"));
    let icon = 22.;
    let size = vec2(
        text.size().x + tokens.number("s-5") + 44.,
        text.size().y + 2. * tokens.number("s-4"),
    );
    let toast = egui::Rect::from_min_size(
        pos2(
            available.center().x - size.x / 2.,
            available.top() + tokens.number("s-5"),
        ),
        size,
    );
    painter.rect(
        toast,
        tokens.number("r-lg"),
        tokens.color("glass-strong"),
        Stroke::new(1., accent(tokens, 0.72)),
        StrokeKind::Inside,
    );
    super::chrome::icon(
        &painter,
        "image",
        pos2(toast.left() + 13. + icon / 2., toast.center().y),
        icon,
        1.7,
        tokens.color("theme-accent"),
    );
    painter.galley(
        pos2(toast.left() + 44., toast.center().y - text.size().y / 2.),
        text,
        tokens.color("glass-text"),
    );
}

/// The side of the drop target an image joins, or `None` for a stack.
fn placement_edge(placement: ImportPlacement) -> Option<CanvasEdge> {
    match placement {
        ImportPlacement::Top => Some(CanvasEdge::Top),
        ImportPlacement::Right => Some(CanvasEdge::Right),
        ImportPlacement::Bottom => Some(CanvasEdge::Bottom),
        ImportPlacement::Left => Some(CanvasEdge::Left),
        ImportPlacement::Stack => None,
    }
}

/// The looping poses shared by every glowing canvas edge: the bloom's
/// `drop-snap-bloom-breathe` (opacity, scale) and the bar's pulse. The loops
/// have no fill mode, so reduced motion rests on each element's own style.
fn bloom_pose(tokens: &Tokens, elapsed: f64, reduced: bool) -> (f64, f32) {
    if reduced {
        return (captures_app::motion::SNAP_BLOOM_REST_OPACITY, 1.);
    }
    let pose = tokens
        .motion(Motion::SnapBloomBreathe)
        .pose_repeating(elapsed, false);
    (pose.opacity, pose.scale as f32)
}

/// A looping bar pulse's `(opacity, brightness)`; reduced motion rests on
/// the element's own style (1, 1).
fn loop_pulse(tokens: &Tokens, motion: Motion, elapsed: f64, reduced: bool) -> (f64, f32) {
    if reduced {
        (1., 1.)
    } else {
        let pose = tokens.motion(motion).pose_repeating(elapsed, false);
        (pose.opacity, pose.brightness as f32)
    }
}

/// CSS `filter: brightness()`: scale the colour channels, clamped.
fn brighten(color: Color32, factor: f32) -> Color32 {
    if factor == 1. {
        return color;
    }
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let channel = |value: u8| (f32::from(value) * factor).round().clamp(0., 255.) as u8;
    Color32::from_rgba_unmultiplied(channel(r), channel(g), channel(b), a)
}

/// Shipping's accent edge snap (`.screenshot-drop-snap-guide.edge-*` and
/// `.screenshot-canvas-expand-edge`): a breathing outward bloom, a pulsing
/// bar straddling the edge, and `DROP_SNAP_PARTICLES` streaming outward.
/// Under reduced motion the bloom and bar hold still and no particles show.
struct EdgeGlow {
    /// Bloom depth outward from the edge, in points.
    depth: f32,
    /// Bloom overhang past each end of the edge, as a fraction of its length.
    overhang: f32,
}

impl EdgeGlow {
    fn paint(
        &self,
        painter: &egui::Painter,
        tokens: &Tokens,
        target: egui::Rect,
        edge: CanvasEdge,
        elapsed: f64,
        reduced: bool,
    ) {
        let color = |alpha: f64| accent(tokens, alpha.clamp(0., 1.) as f32);
        let (strength, scale) = bloom_pose(tokens, elapsed, reduced);
        paint_bloom(
            painter,
            &Bloom {
                rect: target,
                edge,
                depth: self.depth,
                overhang: self.overhang,
                stops: &canvas::SNAP_BLOOM_STOPS,
                strength,
                scale,
            },
            &color,
        );
        let bar = edge_strip(target, edge, canvas::SNAP_EDGE_BAR as f32);
        let pulse = loop_pulse(tokens, Motion::SnapEdgePulse, elapsed, reduced);
        // `0 0 8px .95, 0 0 20px .65, 0 0 36px .4` around the pill.
        paint_edge_bar(
            painter,
            bar,
            &[(16., 0.08), (9., 0.16), (3.5, 0.36)],
            pulse,
            &color,
        );
        paint_particles(painter, target, edge, elapsed, reduced, &color);
    }
}

/// A bar `thickness` thick centered on one side of `rect`, running 1 point
/// past each end.
fn edge_strip(rect: egui::Rect, edge: CanvasEdge, thickness: f32) -> egui::Rect {
    let half = thickness / 2.;
    match edge {
        CanvasEdge::Top => egui::Rect::from_min_max(
            pos2(rect.left() - 1., rect.top() - half),
            pos2(rect.right() + 1., rect.top() + half),
        ),
        CanvasEdge::Bottom => egui::Rect::from_min_max(
            pos2(rect.left() - 1., rect.bottom() - half),
            pos2(rect.right() + 1., rect.bottom() + half),
        ),
        CanvasEdge::Left => egui::Rect::from_min_max(
            pos2(rect.left() - half, rect.top() - 1.),
            pos2(rect.left() + half, rect.bottom() + 1.),
        ),
        CanvasEdge::Right => egui::Rect::from_min_max(
            pos2(rect.right() - half, rect.top() - 1.),
            pos2(rect.right() + half, rect.bottom() + 1.),
        ),
    }
}

/// A pill with layered `(grow, alpha)` glows, all scaled by the pulse's
/// opacity and brightened by its `brightness()` (the filter covers the
/// element's glows too).
fn paint_edge_bar(
    painter: &egui::Painter,
    strip: egui::Rect,
    halos: &[(f32, f64)],
    (opacity, brightness): (f64, f32),
    color: &dyn Fn(f64) -> Color32,
) {
    let radius = strip.width().min(strip.height()) / 2.;
    for (grow, alpha) in halos {
        painter.rect_filled(
            strip.expand(*grow),
            radius + grow,
            brighten(color(alpha * opacity), brightness),
        );
    }
    painter.rect_filled(strip, radius, brighten(color(opacity), brightness));
}

/// `DROP_SNAP_PARTICLES` streaming outward from one side of `rect`.
fn paint_particles(
    painter: &egui::Painter,
    rect: egui::Rect,
    edge: CanvasEdge,
    elapsed: f64,
    reduced: bool,
    color: &dyn Fn(f64) -> Color32,
) {
    for particle in captures_app::motion::SNAP_PARTICLES {
        let Some(pose) = particle.pose(elapsed, reduced) else {
            continue;
        };
        let along = particle.along as f32;
        let outward = pose.outward as f32;
        let center = match edge {
            CanvasEdge::Top => pos2(rect.left() + along * rect.width(), rect.top() - outward),
            CanvasEdge::Bottom => pos2(rect.left() + along * rect.width(), rect.bottom() + outward),
            CanvasEdge::Left => pos2(rect.left() - outward, rect.top() + along * rect.height()),
            CanvasEdge::Right => pos2(rect.right() + outward, rect.top() + along * rect.height()),
        };
        let size = (particle.size * pose.scale) as f32;
        painter.circle_filled(center, size / 2. + 3., color(0.25 * pose.opacity));
        painter.circle_filled(center, size / 2., color(pose.opacity));
    }
}

/// `.screenshot-drop-snap-stack-light`: warm pool, contact shadow and white rim
/// under the drag preview footprint.
fn paint_stack_light(painter: &egui::Painter, focus: egui::Rect) {
    for (grow, alpha) in [(0.62, 0.05), (0.42, 0.09), (0.22, 0.14)] {
        painter.rect_filled(
            focus.expand2(vec2(focus.width() * grow, focus.height() * grow)),
            focus.height(),
            Color32::from_rgba_unmultiplied(255, 246, 232, (255. * alpha) as u8),
        );
    }
    for (offset, alpha) in [(18., 0.12), (8., 0.22), (2., 0.28)] {
        painter.rect_filled(
            focus.translate(vec2(0., offset)).expand(offset / 2.),
            8.,
            Color32::from_black_alpha((255. * alpha) as u8),
        );
    }
    painter.rect_stroke(
        focus,
        8.,
        Stroke::new(1., Color32::from_white_alpha(92)),
        StrokeKind::Outside,
    );
}

/// The selected layer's overflow preview while no gesture is active.
pub(super) fn expand_preview(view: &View) -> Option<(String, CanvasExpandPreview)> {
    if view.layer_gesture.is_some() || view.pending || view.inline.is_some() {
        return None;
    }
    let presented = view.presented.as_ref()?;
    let id = view.selected_layer.as_ref()?;
    let element = presented
        .document
        .elements
        .iter()
        .find(|element| &element.base().id == id)?;
    let preview =
        canvas::canvas_expand_preview(element, presented.document.width, presented.document.height)
            .ok()??;
    Some((id.clone(), preview))
}

/// Screen rect of the Expand canvas action, kept inside the canvas viewport.
pub(super) fn expand_button_rect(
    ui: &egui::Ui,
    tokens: &Tokens,
    view: &View,
    available: egui::Rect,
    preview: egui::Rect,
) -> Option<(String, CanvasExpandPreview, egui::Rect)> {
    let (id, expand) = expand_preview(view)?;
    let bounds = document_bounds(view)?;
    let scale = f64::from(preview.width()) / bounds.width;
    let anchor = project(
        preview,
        bounds,
        canvas::inset_anchor(
            expand.anchor,
            expand.anchor_edge,
            canvas::EXPAND_ACTION_INSET / scale.max(0.01),
        ),
    );
    let font = FontId::new(
        tokens.number("text-xs"),
        FontFamily::Name("semibold".into()),
    );
    let text = ui
        .painter()
        .layout_no_wrap(canvas::EXPAND_CANVAS.into(), font, Color32::WHITE);
    let size = text.size() + 2. * vec2(tokens.number("s-4"), tokens.number("s-2"));
    let inner = available.shrink(4.);
    if inner.width() < size.x || inner.height() < size.y {
        return None;
    }
    let center = pos2(
        anchor
            .x
            .clamp(inner.left() + size.x / 2., inner.right() - size.x / 2.),
        anchor
            .y
            .clamp(inner.top() + size.y / 2., inner.bottom() - size.y / 2.),
    );
    Some((id, expand, egui::Rect::from_center_size(center, size)))
}

/// Paint the overflow tint, the armed ghost and the accent Expand canvas
/// action. A click submits one `expand_canvas` layer edit (one undo step).
pub(super) fn show_expand(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    available: egui::Rect,
    preview: egui::Rect,
) {
    let Some((id, expand, button)) = expand_button_rect(ui, tokens, view, available, preview)
    else {
        return;
    };
    let Some(bounds) = document_bounds(view) else {
        return;
    };
    let painter = ui
        .painter()
        .with_clip_rect(available.intersect(ui.clip_rect()));
    // `.screenshot-canvas-expand-overflow`: the hanging content reads as
    // outside the frame.
    for gap in &expand.gaps {
        painter.rect_filled(
            project_rect(preview, bounds, *gap),
            0.,
            accent(tokens, 0.12),
        );
    }
    let response = ui
        .interact(
            button,
            ui.scope_id().with("canvas-expand"),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, canvas::EXPAND_CANVAS)
    });
    let armed_id = ui.scope_id().with("canvas-expand-armed");
    if response.hovered() {
        // `.screenshot-canvas-expand-ghost`: dashed post-release canvas with
        // brighter crossed sides, breathing, plus each crossed side's glow.
        let now = ui.input(|input| input.time);
        let since = ui
            .ctx()
            .data_mut(|data| *data.get_temp_mut_or_insert_with(armed_id, || now));
        let elapsed = (now - since) * 1000.;
        let reduced = crate::motion::reduced(ui.ctx());
        let breathe = loop_pulse(tokens, Motion::ExpandGhostBreathe, elapsed, reduced).0 as f32;
        let ghost = project_rect(preview, bounds, expand.rect);
        let sides = [
            (CanvasEdge::Top, ghost.left_top(), ghost.right_top()),
            (CanvasEdge::Right, ghost.right_top(), ghost.right_bottom()),
            (
                CanvasEdge::Bottom,
                ghost.right_bottom(),
                ghost.left_bottom(),
            ),
            (CanvasEdge::Left, ghost.left_bottom(), ghost.left_top()),
        ];
        for (edge, from, to) in sides {
            let stroke = if expand.edges.contains(&edge) {
                Stroke::new(2., accent(tokens, 0.82 * breathe))
            } else {
                Stroke::new(1.5, accent(tokens, 0.42 * breathe))
            };
            painter.extend(egui::Shape::dashed_line(&[from, to], stroke, 6., 4.));
        }
        let glow = EdgeGlow {
            depth: canvas::SNAP_BLOOM as f32,
            overhang: 0.,
        };
        for edge in &expand.edges {
            glow.paint(&painter, tokens, ghost, *edge, elapsed, reduced);
        }
        if !reduced {
            ui.ctx().request_repaint();
        }
    } else {
        ui.ctx().data_mut(|data| data.remove::<f64>(armed_id));
    }
    let fill = if response.hovered() {
        tokens.color("theme-accent-hover")
    } else {
        tokens.color("theme-accent")
    };
    painter.rect_filled(
        button.translate(vec2(0., 2.)),
        tokens.number("r-sm"),
        Color32::from_black_alpha(40),
    );
    painter.rect_filled(button, tokens.number("r-sm"), fill);
    let font = FontId::new(
        tokens.number("text-xs"),
        FontFamily::Name("semibold".into()),
    );
    painter.text(
        button.center(),
        egui::Align2::CENTER_CENTER,
        canvas::EXPAND_CANVAS,
        font,
        tokens.color("theme-accent-ink"),
    );
    if response.clicked() {
        view.pending_layer_selection = Some(id.clone());
        view.invalidate_output();
        view.submit(
            tx,
            Request::Layer {
                id,
                edit: LayerEdit::ExpandCanvas,
            },
        );
    }
}

/// Curve chrome for a selected, visible and unlocked line/arrow.
pub(super) fn selected_curve(view: &View) -> Option<(ShapeElement, CurveHandles)> {
    let presented = view.presented.as_ref()?;
    let id = view.selected_layer.as_ref()?;
    presented
        .document
        .elements
        .iter()
        .find_map(|element| match element {
            Element::Shape(shape)
                if &shape.base.id == id && shape.base.visible && !shape.base.locked =>
            {
                canvas::curve_handles(shape).map(|handles| (shape.clone(), handles))
            }
            _ => None,
        })
}

/// Shipping double-click on a line/arrow: remove a control dot, add a point on
/// the selected path, or select an unselected path and add a point. Returns
/// true when the double-click was consumed.
pub(super) fn double_click(
    view: &mut View,
    tx: &Sender<Job>,
    document: &Document,
    point: Point,
    radius: f64,
) -> bool {
    if let Some((shape, _)) = selected_curve(view) {
        match canvas::hit_test_curve_handle(&shape, point, radius) {
            Some(CurveHandle::Control { index }) => {
                submit_curve(view, tx, shape.base.id, CurveEdit::Remove { index });
                return true;
            }
            Some(_) => return true,
            None => {}
        }
        let (on_path, _, distance) = canvas::closest_point_on_curve(&shape, point);
        if distance <= canvas::curve_path_hit_radius(&shape, radius) {
            submit_curve(
                view,
                tx,
                shape.base.id,
                CurveEdit::Insert { point: on_path },
            );
            return true;
        }
    }
    if let Ok(Some(Element::Shape(shape))) = document.hit_test(point, radius)
        && canvas::is_curveable(shape)
        && Some(&shape.base.id) != view.selected_layer.as_ref()
    {
        let (on_path, _, distance) = canvas::closest_point_on_curve(shape, point);
        if distance <= canvas::curve_path_hit_radius(shape, radius) {
            submit_curve(
                view,
                tx,
                shape.base.id.clone(),
                CurveEdit::Insert { point: on_path },
            );
            return true;
        }
    }
    false
}

pub(super) fn submit_curve(view: &mut View, tx: &Sender<Job>, id: String, edit: CurveEdit) {
    view.pending_layer_selection = Some(id.clone());
    view.invalidate_output();
    view.submit(
        tx,
        Request::Layer {
            id,
            edit: LayerEdit::Curve { edit },
        },
    );
}

/// Paint curve dots: endpoints, free controls and the virtual starters.
pub(super) fn paint_curve_handles(
    painter: &egui::Painter,
    tokens: &Tokens,
    handles: &CurveHandles,
    project: impl Fn(Point) -> egui::Pos2,
    show_path: bool,
) {
    let stroke = Stroke::new(2., tokens.color("theme-accent"));
    if show_path && handles.path.len() >= 2 {
        painter.add(egui::Shape::line(
            handles.path.iter().copied().map(&project).collect(),
            Stroke::new(1.5, accent(tokens, 0.85)),
        ));
    }
    for starter in &handles.starters {
        painter.circle(
            project(*starter),
            4.5,
            tokens.color("surface-raised"),
            Stroke::new(1.5, accent(tokens, 0.8)),
        );
    }
    for control in &handles.controls {
        painter.circle(project(*control), 5., tokens.color("theme-accent"), stroke);
    }
    for end in [handles.start, handles.end] {
        painter.circle(project(end), 4.5, tokens.color("surface-raised"), stroke);
    }
}

/// Floating curve tip near the pointer (`.screenshot-curve-hover-tip`).
pub(super) fn paint_hover_tip(ui: &egui::Ui, tokens: &Tokens, pointer: egui::Pos2, text: &str) {
    let painter = ui.painter();
    let font = FontId::new(tokens.number("text-xs"), FontFamily::Proportional);
    let galley = painter.layout_no_wrap(text.into(), font, tokens.color("glass-text"));
    let rect = egui::Rect::from_min_size(
        pointer + vec2(14., 18.),
        galley.size() + 2. * vec2(tokens.number("s-3"), tokens.number("s-2")),
    );
    painter.rect(
        rect,
        tokens.number("r-sm"),
        tokens.color("glass-strong"),
        Stroke::new(1., tokens.color("glass-border")),
        StrokeKind::Inside,
    );
    painter.galley(
        rect.min + vec2(tokens.number("s-3"), tokens.number("s-2")),
        galley,
        tokens.color("glass-text"),
    );
}

/// Hover copy for the pointer over the selected or an unselected line/arrow.
pub(super) fn hover_hint(
    view: &View,
    document: &Document,
    point: Point,
    radius: f64,
) -> Option<&'static str> {
    if let Some((shape, _)) = selected_curve(view)
        && let Some(hint) = canvas::curve_hover_hint(&shape, point, radius)
    {
        return Some(hint);
    }
    match document.hit_test(point, radius) {
        Ok(Some(Element::Shape(shape)))
            if canvas::is_curveable(shape)
                && Some(&shape.base.id) != view.selected_layer.as_ref() =>
        {
            let (_, _, distance) = canvas::closest_point_on_curve(shape, point);
            (distance <= canvas::curve_path_hit_radius(shape, radius))
                .then_some(canvas::CURVE_UNSELECTED_HINT)
        }
        _ => None,
    }
}

/// Live curve drag preview: the shape as it would commit at `current`.
pub(super) fn curve_drag_preview(
    shape: &ShapeElement,
    handle: CurveHandle,
    current: Point,
) -> Option<CurveHandles> {
    let mut next = shape.clone();
    canvas::apply_curve_edit(
        &mut next,
        CurveEdit::Move {
            handle,
            point: current,
        },
    )
    .ok()?;
    canvas::curve_handles(&next)
}

/// Inspector Curve section: slider for straight/single-control strokes,
/// Straighten for multi-point strokes, and the shipping help line. Slider
/// changes commit once on release.
pub(super) fn show_curve_controls(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    shape: &ShapeElement,
) {
    let Some(handles) = canvas::curve_handles(shape) else {
        return;
    };
    ui.separator();
    if handles.slider {
        let staged = view
            .curve_bend
            .as_ref()
            .filter(|(id, _)| *id == shape.base.id)
            .map_or(handles.bend_percent, |(_, value)| *value);
        let mut value = staged;
        ui.label(canvas::CURVE_LABEL);
        let marks =
            canvas::CURVE_MARKS.map(|(value, label)| crate::primitives::RangeMark { value, label });
        let response = crate::primitives::RangeSlider::new(
            ("curve-bend", shape.base.id.as_str()),
            canvas::CURVE_LABEL,
            ui.available_width().min(272.),
            -100. ..=100.,
            format!("{}%", value.round()),
        )
        .marks(&marks)
        .show(ui, tokens, &mut value);
        if response.changed() {
            view.curve_bend = Some((shape.base.id.clone(), value));
        }
        // Pointer drags commit once on release; keyboard steps commit at once.
        let released = response.drag_stopped()
            || response.clicked()
            || (response.changed() && !response.is_pointer_button_down_on());
        if released
            && let Some((id, value)) = view.curve_bend.take()
            && value != handles.bend_percent
        {
            submit_curve(view, tx, id, CurveEdit::Bend { bend: value / 100. });
        }
    } else if ui.button(handles.straighten_label).clicked() {
        submit_curve(view, tx, shape.base.id.clone(), CurveEdit::Straighten);
    }
    ui.small(canvas::CURVE_HELP);
}

/// `rgba(var(--trim-rgb), alpha)`.
fn trim_color(alpha: f64) -> Color32 {
    let [r, g, b] = canvas::TRIM_RGB;
    Color32::from_rgba_unmultiplied(r, g, b, (alpha.clamp(0., 1.) * 255.).round() as u8)
}

/// One edge bloom: a gradient strip outward from one side of `rect`.
struct Bloom<'a> {
    rect: egui::Rect,
    edge: CanvasEdge,
    depth: f32,
    /// Extra length past each end, as a fraction of the edge.
    overhang: f32,
    /// `(offset from the edge, alpha)` across the strip.
    stops: &'a [(f64, f64)],
    /// The breathing opacity.
    strength: f64,
    /// The breathing scale about the strip's center.
    scale: f32,
}

/// Paint `bloom`: `stops` fade across it (0 at the edge) and shipping's
/// 12 %/88 % mask fades it along the edge.
fn paint_bloom(painter: &egui::Painter, bloom: &Bloom, color: &dyn Fn(f64) -> Color32) {
    let Bloom {
        rect,
        edge,
        depth,
        overhang,
        stops,
        strength,
        scale,
    } = *bloom;
    let along = [(0., 0.), (0.12, 1.), (0.88, 1.), (1., 0.)];
    // Scale about the strip's center: along its length and across its depth.
    let span = |s: f32| 0.5 + ((s * (1. + 2. * overhang) - overhang) - 0.5) * scale;
    let deep = |t: f32| depth * (0.5 + (t - 0.5) * scale);
    let place = |s: f32, t: f32| match edge {
        CanvasEdge::Top => pos2(rect.left() + span(s) * rect.width(), rect.top() - deep(t)),
        CanvasEdge::Bottom => pos2(
            rect.left() + span(s) * rect.width(),
            rect.bottom() + deep(t),
        ),
        CanvasEdge::Left => pos2(rect.left() - deep(t), rect.top() + span(s) * rect.height()),
        CanvasEdge::Right => pos2(rect.right() + deep(t), rect.top() + span(s) * rect.height()),
    };
    let mut mesh = egui::Mesh::default();
    let columns = along.len() as u32;
    for (across, alpha) in stops {
        for (s, mask) in along {
            mesh.colored_vertex(
                place(s as f32, *across as f32),
                color(alpha * mask * strength),
            );
        }
    }
    for row in 0..stops.len().saturating_sub(1) as u32 {
        for column in 0..columns - 1 {
            let a = row * columns + column;
            mesh.add_triangle(a, a + 1, a + columns);
            mesh.add_triangle(a + 1, a + columns + 1, a + columns);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Shipping `.screenshot-canvas-trim-hint`, shown while Trim edges is hovered
/// or focused: the discarded margins tinted red, a dashed outline of the kept
/// area, and pulsing cut edges with blooms and particles streaming into the
/// discard strips. Under reduced motion the hint is static and particle-free.
pub(super) fn paint_trim_preview(
    ui: &egui::Ui,
    tokens: &Tokens,
    view: &View,
    available: egui::Rect,
    preview: egui::Rect,
) {
    let Some(since) = view.trim_hover_since else {
        return;
    };
    let (Some(presented), Some(bounds)) = (view.presented.as_ref(), document_bounds(view)) else {
        return;
    };
    let Some(trim) = presented.document.trim_preview() else {
        return;
    };
    let reduced = crate::motion::reduced(ui.ctx());
    let elapsed = (ui.input(|input| input.time) - since) * 1000.;
    // No fill mode: reduced motion rests on each element's own opacity (1).
    let breathe = |motion: Motion| {
        if reduced {
            1.
        } else {
            tokens.motion(motion).pose_repeating(elapsed, false).opacity
        }
    };
    let (region, keep_alpha) = (
        breathe(Motion::TrimRegionBreathe),
        breathe(Motion::TrimKeepBreathe),
    );
    let edge_pulse = loop_pulse(tokens, Motion::TrimEdgePulse, elapsed, reduced);
    let painter = ui
        .painter()
        .with_clip_rect(available.intersect(ui.clip_rect()));
    for (_, rect) in &trim.regions {
        let rect = project_rect(preview, bounds, *rect);
        painter.rect_filled(rect, 0., trim_color(canvas::TRIM_REGION_ALPHA * region));
        // `inset 0 0 24px rgba(trim, 0.12)`.
        for (width, alpha) in [(12., 0.04), (5., 0.05)] {
            painter.rect_stroke(
                rect,
                0.,
                Stroke::new(width, trim_color(alpha * region)),
                StrokeKind::Inside,
            );
        }
    }
    let keep = project_rect(preview, bounds, trim.keep);
    let radius = canvas::TRIM_KEEP_RADIUS as f32;
    for (grow, alpha) in [(11., 0.03), (6., 0.06), (1., 0.1)] {
        painter.rect_stroke(
            keep.expand(grow / 2.),
            radius + grow / 2.,
            Stroke::new(grow, trim_color(alpha * keep_alpha)),
            StrokeKind::Middle,
        );
    }
    let corners = [
        keep.left_top(),
        keep.right_top(),
        keep.right_bottom(),
        keep.left_bottom(),
        keep.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(
        &corners,
        Stroke::new(
            canvas::TRIM_KEEP_WIDTH as f32,
            trim_color(canvas::TRIM_KEEP_ALPHA * keep_alpha),
        ),
        4.5,
        3.,
    ));
    let (bloom_strength, bloom_scale) = bloom_pose(tokens, elapsed, reduced);
    for edge in &trim.edges {
        paint_bloom(
            &painter,
            &Bloom {
                rect: keep,
                edge: *edge,
                depth: canvas::TRIM_BLOOM as f32,
                overhang: 0.,
                stops: &canvas::TRIM_BLOOM_STOPS,
                strength: bloom_strength,
                scale: bloom_scale,
            },
            &trim_color,
        );
        let strip = edge_strip(keep, *edge, canvas::TRIM_EDGE_BAR as f32);
        // `0 0 8px .95, 0 0 20px .55, 0 0 32px .32` around the pill.
        paint_edge_bar(
            &painter,
            strip,
            &[(14., 0.06), (8., 0.12), (3., 0.3)],
            edge_pulse,
            &trim_color,
        );
        paint_particles(&painter, keep, *edge, elapsed, reduced, &trim_color);
    }
    if !reduced {
        ui.ctx().request_repaint();
    }
}

/// The Wand loupe's circular magnifier, rendered once per sampled pixel.
pub(super) struct LoupeTexture {
    key: (String, (u32, u32), u32),
    texture: egui::TextureHandle,
}

/// Rasterize `paintWandColorLoupe` into a circle: checkerboard, nearest-
/// neighbour tiles, the source-pixel grid and the highlighted center sample.
pub(super) fn loupe_image(loupe: &WandLoupe, pixels_per_point: f32) -> egui::ColorImage {
    let size_points = image_background::WAND_LOUPE_SIZE as f32;
    let side = (size_points * pixels_per_point).round().max(1.) as usize;
    let scale = side as f32 / size_points;
    let (dark, light, check) = image_background::WAND_LOUPE_CHECKER;
    let extent = loupe.extent.max(1) as usize;
    let cell = size_points / extent as f32;
    let half = (extent / 2) as f32;
    let center = half * cell;
    let grid = (image_background::WAND_LOUPE_GRID_ALPHA * 255.) as u16;
    let mut image = egui::ColorImage::filled([side, side], Color32::TRANSPARENT);
    let radius = side as f32 / 2.;
    let over = |base: [u8; 3], top: [u8; 4]| {
        let alpha = u16::from(top[3]);
        std::array::from_fn::<u8, 3, _>(|index| {
            ((u16::from(top[index]) * alpha + u16::from(base[index]) * (255 - alpha)) / 255) as u8
        })
    };
    for py in 0..side {
        for px in 0..side {
            let distance =
                ((px as f32 + 0.5 - radius).powi(2) + (py as f32 + 0.5 - radius).powi(2)).sqrt();
            let coverage = (radius - distance + 0.5).clamp(0., 1.);
            if coverage <= 0. {
                continue;
            }
            let (x, y) = ((px as f32 + 0.5) / scale, (py as f32 + 0.5) / scale);
            let checker =
                if ((x / check as f32) as usize + (y / check as f32) as usize).is_multiple_of(2) {
                    dark
                } else {
                    light
                };
            let column = ((x / cell) as usize).min(extent - 1);
            let row = ((y / cell) as usize).min(extent - 1);
            let mut rgb = match loupe.tiles.get(row * extent + column).copied().flatten() {
                Some(tile) => over(checker, tile),
                None => checker,
            };
            // One device pixel of grid at each interior tile boundary.
            let on_grid = |value: f32, device: usize| {
                let index = (value / cell).round();
                index >= 1. && index < extent as f32 && ((index * cell * scale) as usize == device)
            };
            if on_grid(x, px) || on_grid(y, py) {
                rgb = over(rgb, [0, 0, 0, grid as u8]);
            }
            // The keyed sample: a white 1.5 pt ring inside a black 1 pt ring.
            let inside = |inset: f32| {
                x >= center + inset
                    && x <= center + cell - inset
                    && y >= center + inset
                    && y <= center + cell - inset
            };
            if inside(-0.5) && !inside(0.5) {
                rgb = over(rgb, [0, 0, 0, 140]);
            } else if inside(0.5) && !inside(2.) {
                rgb = over(rgb, [255, 255, 255, 242]);
            }
            let alpha = (coverage * 255.).round() as u8;
            image.pixels[py * side + px] =
                Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], alpha);
        }
    }
    image
}

/// Shipping `.screenshot-brush-cursor` (`editor_chrome::brush_cursor`): a
/// white ring with a dark halo outside and a faint dark line inside over a 4 %
/// white fill; Restore dashes the ring over an 8 % accent fill.
pub(super) fn paint_brush_cursor(
    painter: &egui::Painter,
    center: egui::Pos2,
    diameter: f32,
    restore: bool,
    accent: Color32,
) {
    use captures_app::editor_chrome::brush_cursor as b;
    let rgba = |[r, g, b, a]: [f64; 4]| {
        let channel = |value: f64| (value * 255.).round().clamp(0., 255.) as u8;
        Color32::from_rgba_unmultiplied(channel(r), channel(g), channel(b), channel(a))
    };
    let radius = diameter / 2.;
    let ring = b::RING_WIDTH as f32;
    let fill = if restore {
        accent.gamma_multiply(b::RESTORE_FILL_ACCENT_ALPHA as f32)
    } else {
        rgba(b::ERASE_FILL_RGBA)
    };
    painter.circle_filled(center, radius, fill);
    let halo = b::HALO_WIDTH as f32;
    painter.circle_stroke(
        center,
        radius + halo / 2.,
        Stroke::new(halo, rgba(b::HALO_RGBA)),
    );
    let inset = b::INSET_WIDTH as f32;
    let inset_radius = radius - ring - inset / 2.;
    if inset_radius > 0. {
        painter.circle_stroke(
            center,
            inset_radius,
            Stroke::new(inset, rgba(b::INSET_RGBA)),
        );
    }
    // CSS draws the border inside the box: centre the stroke half a width in.
    let border_radius = (radius - ring / 2.).max(ring / 2.);
    let stroke = Stroke::new(ring, rgba(b::RING_RGBA));
    if restore {
        let segments =
            ((border_radius * std::f32::consts::TAU / 2.).ceil() as usize).clamp(16, 512);
        let points = (0..=segments)
            .map(|index| {
                let angle = index as f32 / segments as f32 * std::f32::consts::TAU;
                center + border_radius * vec2(angle.cos(), angle.sin())
            })
            .collect::<Vec<_>>();
        painter.extend(egui::Shape::dashed_line(
            &points,
            stroke,
            b::RESTORE_DASH[0] as f32,
            b::RESTORE_DASH[1] as f32,
        ));
    } else {
        painter.circle_stroke(center, border_radius, stroke);
    }
}

/// Shipping `WandColorLoupe`: while the Wand hovers an image, a magnified
/// circle of natural pixels beside the crosshair plus a swatch and hex pill.
/// Returns false (and hides the loupe) when the pointer is off every image.
pub(super) fn show_wand_loupe(
    ui: &egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    preview: egui::Rect,
    pointer: egui::Pos2,
) -> bool {
    let (Some(presented), Some(bounds)) = (view.presented.as_ref(), document_bounds(view)) else {
        view.wand_loupe = None;
        return false;
    };
    let point = image_point(pointer, preview, bounds);
    let Some(loupe) = image_background::wand_loupe(
        &presented.document,
        |src| presented.image_assets.get(src).map(|asset| &**asset),
        point,
    ) else {
        view.wand_loupe = None;
        return false;
    };
    let ppp = ui.ctx().pixels_per_point();
    let src = image_background::wand_target(&presented.document, point)
        .map(|(_, image, _)| image.src.clone())
        .unwrap_or_default();
    let key = (src, loupe.pixel, ppp.to_bits());
    if view
        .wand_loupe
        .as_ref()
        .is_none_or(|cached| cached.key != key)
    {
        let texture = ui.ctx().load_texture(
            "wand-loupe",
            loupe_image(&loupe, ppp),
            egui::TextureOptions::NEAREST,
        );
        view.wand_loupe = Some(LoupeTexture { key, texture });
    }
    let Some(cached) = view.wand_loupe.as_ref() else {
        return true;
    };
    let viewport = ui.ctx().content_rect();
    let size = image_background::WAND_LOUPE_SIZE as f32;
    let origin = image_background::wand_loupe_position(
        Point {
            x: f64::from(pointer.x - viewport.left()),
            y: f64::from(pointer.y - viewport.top()),
        },
        f64::from(viewport.width()),
        f64::from(viewport.height()),
    );
    let circle = egui::Rect::from_min_size(
        pos2(
            viewport.left() + origin.x as f32,
            viewport.top() + origin.y as f32,
        ),
        vec2(size, size),
    );
    let id = egui::Id::unique("screenshot-wand-loupe");
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Tooltip, id));
    // `drop-shadow(0 8px 18px rgba(0, 0, 0, 0.42))`.
    for (grow, alpha) in [(9., 0.06), (5., 0.1), (2., 0.14)] {
        painter.circle_filled(
            circle.center() + vec2(0., 8.),
            size / 2. + grow,
            Color32::from_black_alpha((255. * alpha) as u8),
        );
    }
    painter.image(
        cached.texture.id(),
        circle,
        egui::Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
        Color32::WHITE,
    );
    // `.screenshot-wand-loupe-rim`: white 2 pt border, black hairlines.
    let center = circle.center();
    painter.circle_stroke(
        center,
        size / 2. - 1.,
        Stroke::new(2., Color32::from_white_alpha(235)),
    );
    painter.circle_stroke(
        center,
        size / 2. + 0.5,
        Stroke::new(1., Color32::from_black_alpha(140)),
    );
    painter.circle_stroke(
        center,
        size / 2. - 2.5,
        Stroke::new(1., Color32::from_black_alpha(71)),
    );
    // `.screenshot-wand-loupe-meta`: swatch and hex in a glass pill below.
    let font = FontId::new(tokens.number("text-sm"), FontFamily::Proportional);
    let text = painter.layout_no_wrap(loupe.text.clone(), font, tokens.color("glass-text"));
    let swatch = 14.;
    let gap = tokens.number("s-3");
    let pill_size = vec2(
        4. + swatch + gap + text.size().x + 7.,
        swatch.max(text.size().y) + 6.,
    );
    let pill = egui::Rect::from_min_size(
        pos2(
            center.x - pill_size.x / 2.,
            circle.bottom() + image_background::WAND_LOUPE_META_GAP as f32,
        ),
        pill_size,
    );
    painter.rect(
        pill,
        pill.height() / 2.,
        tokens.color("glass-strong"),
        Stroke::new(1., tokens.color("glass-border")),
        StrokeKind::Inside,
    );
    let swatch_center = pos2(pill.left() + 4. + swatch / 2., pill.center().y);
    if loupe.transparent {
        let (dark, light, _) = image_background::WAND_LOUPE_CHECKER;
        painter.circle_filled(
            swatch_center,
            swatch / 2.,
            Color32::from_rgb(light[0], light[1], light[2]),
        );
        for (dx, dy) in [(-1., -1.), (1., 1.)] {
            painter.circle_filled(
                swatch_center + vec2(dx * swatch / 4.5, dy * swatch / 4.5),
                swatch / 5.,
                Color32::from_rgb(dark[0], dark[1], dark[2]),
            );
        }
    } else {
        let [r, g, b, _] = loupe.color;
        painter.circle_filled(swatch_center, swatch / 2., Color32::from_rgb(r, g, b));
    }
    painter.circle_stroke(
        swatch_center,
        swatch / 2. - 0.5,
        Stroke::new(1., Color32::from_white_alpha(140)),
    );
    painter.circle_stroke(
        swatch_center,
        swatch / 2. - 1.5,
        Stroke::new(1., Color32::from_black_alpha(89)),
    );
    painter.galley(
        pos2(
            swatch_center.x + swatch / 2. + gap,
            pill.center().y - text.size().y / 2.,
        ),
        text,
        tokens.color("glass-text"),
    );
    // `role="tooltip"` with the sample's `aria-label`, in a non-interactive
    // layer so it never takes the canvas pointer.
    egui::Area::new(id.with("label"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(circle.min)
        .show(ui.ctx(), |ui| {
            let (_, response) = ui.allocate_exact_size(circle.size(), egui::Sense::hover());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &loupe.accessible_label)
            });
        });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_pulse_brightens_the_bar_like_the_shipping_filter() {
        let accent = Color32::from_rgb(200, 100, 20);
        assert_eq!(brighten(accent, 1.), accent);
        assert_eq!(brighten(accent, 1.15), Color32::from_rgb(230, 115, 23));
        assert_eq!(
            brighten(Color32::from_rgb(250, 0, 0), 1.15).r(),
            255,
            "channels clamp"
        );
        let faded = Color32::from_rgba_unmultiplied(200, 100, 20, 128);
        assert_eq!(brighten(faded, 1.15).a(), faded.a(), "alpha is untouched");
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let (opacity, brightness) = loop_pulse(&tokens, Motion::SnapEdgePulse, 700., false);
        assert!((opacity - 0.88).abs() < 1e-6 && (brightness - 1.15).abs() < 1e-6);
        assert_eq!(
            loop_pulse(&tokens, Motion::TrimEdgePulse, 700., true),
            (1., 1.),
            "reduced motion rests on the element's own style"
        );
    }
}
