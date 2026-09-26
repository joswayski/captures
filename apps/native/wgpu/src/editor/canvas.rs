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
    editor_session::{
        DROP_IMAGE, DROP_UNSUPPORTED, ImageDropGuide, ImportPlacement, image_drop_guide,
        is_supported_image_path,
    },
};
use eframe::egui::{Color32, FontFamily, FontId, Stroke, StrokeKind, pos2, vec2};

/// Queued dropped files and the live hover state. Files import one at a time
/// through the worker, preserving the one-in-flight edit contract.
#[derive(Default)]
pub(super) struct DropState {
    pub hovering: bool,
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
    let (hovering, dropped, pointer) = ctx.input(|input| {
        (
            !input.raw.hovered_files.is_empty(),
            input.raw.dropped_files.clone(),
            input.pointer.latest_pos(),
        )
    });
    let accepting = view.presented.is_some() && !view.closed && !view.close_requested;
    view.drop.hovering = hovering && accepting;
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
    if view.pending || view.inline.is_some() || view.confirm_discard {
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
                paint_edge_bloom(&painter, tokens, target, guide.placement);
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

/// The bright edge bar with its outward bloom on the side the image joins.
fn paint_edge_bloom(
    painter: &egui::Painter,
    tokens: &Tokens,
    target: egui::Rect,
    placement: ImportPlacement,
) {
    let edge = match placement {
        ImportPlacement::Top => CanvasEdge::Top,
        ImportPlacement::Right => CanvasEdge::Right,
        ImportPlacement::Bottom => CanvasEdge::Bottom,
        ImportPlacement::Left => CanvasEdge::Left,
        ImportPlacement::Stack => return,
    };
    paint_edge_glow(painter, tokens, target, edge);
}

/// Shared by the drop guide and the Expand canvas hint: layered glow outward
/// from one edge (static; the shipping pulse is decorative).
fn paint_edge_glow(painter: &egui::Painter, tokens: &Tokens, target: egui::Rect, edge: CanvasEdge) {
    // Bands grow outward from the edge; the solid bar straddles it.
    let band = |outward: f32, inward: f32| match edge {
        CanvasEdge::Top => egui::Rect::from_min_max(
            pos2(target.left(), target.top() - outward),
            pos2(target.right(), target.top() + inward),
        ),
        CanvasEdge::Bottom => egui::Rect::from_min_max(
            pos2(target.left(), target.bottom() - inward),
            pos2(target.right(), target.bottom() + outward),
        ),
        CanvasEdge::Left => egui::Rect::from_min_max(
            pos2(target.left() - outward, target.top()),
            pos2(target.left() + inward, target.bottom()),
        ),
        CanvasEdge::Right => egui::Rect::from_min_max(
            pos2(target.right() - inward, target.top()),
            pos2(target.right() + outward, target.bottom()),
        ),
    };
    for (outward, alpha) in [(24., 0.05), (14., 0.08), (6., 0.14)] {
        painter.rect_filled(band(outward, 0.), 0., accent(tokens, alpha));
    }
    painter.rect_filled(band(2., 2.), 2., tokens.color("theme-accent"));
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
    if response.hovered() {
        // `.screenshot-canvas-expand-ghost`: dashed post-release canvas with
        // brighter crossed sides and their edge glow.
        let ghost = project_rect(preview, bounds, expand.rect);
        let corners = [
            ghost.left_top(),
            ghost.right_top(),
            ghost.right_bottom(),
            ghost.left_bottom(),
            ghost.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(
            &corners,
            Stroke::new(1.5, accent(tokens, 0.42)),
            6.,
            4.,
        ));
        for edge in &expand.edges {
            paint_edge_glow(&painter, tokens, ghost, *edge);
        }
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
        let response = ui.add(
            egui::Slider::new(&mut value, -100. ..=100.)
                .step_by(1.)
                .suffix("%")
                .show_value(true),
        );
        response.widget_info(|| egui::WidgetInfo::slider(true, value, canvas::CURVE_LABEL));
        ui.horizontal(|ui| {
            for (_, mark) in canvas::CURVE_MARKS {
                ui.small(mark);
            }
        });
        if response.changed() {
            view.curve_bend = Some((shape.base.id.clone(), value));
        }
        let released = response.drag_stopped() || (response.changed() && !response.dragged());
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
