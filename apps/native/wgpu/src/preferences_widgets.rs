//! Shipping Preferences controls drawn from design tokens: the sidebar,
//! switches, segmented control, selects, buttons, accent chips and the mini
//! preview corner picker (`styles/windows.css`, `styles/primitives.css`).

use eframe::egui::{
    self, Color32, FontId, Rect, Response, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

use crate::primitives::focus_ring;
use crate::tokens::Tokens;

/// `--shadow-sm` for the current appearance.
pub fn shadow_sm(dark: bool) -> egui::Shadow {
    if dark {
        egui::Shadow {
            offset: [0, 2],
            blur: 6,
            spread: 0,
            color: Color32::from_black_alpha(82),
        }
    } else {
        egui::Shadow {
            offset: [0, 1],
            blur: 3,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(19, 19, 24, 20),
        }
    }
}

/// `--shadow-xs` as a painted shape under `rect`.
fn shadow_xs(ui: &egui::Ui, rect: Rect, radius: f32) -> egui::Shape {
    let color = if ui.visuals().dark_mode {
        Color32::from_black_alpha(89)
    } else {
        Color32::from_rgba_unmultiplied(19, 19, 24, 15)
    };
    egui::Shadow {
        offset: [0, 1],
        blur: 2,
        spread: 0,
        color,
    }
    .as_shape(rect, radius)
    .into()
}

fn text(ui: &egui::Ui, value: &str, size: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    ui.painter()
        .layout_no_wrap(value.to_owned(), FontId::proportional(size), color)
}

/// Stroke a shared shipping icon (24-unit grid) into `rect`.
pub fn icon(painter: &egui::Painter, name: &str, rect: Rect, color: Color32) {
    let scale = rect.width() / 24.;
    let stroke = Stroke::new((1.8 * scale).max(1.), color);
    for line in captures_app::icons::polylines(name).unwrap_or_default() {
        let points = line
            .iter()
            .map(|[x, y]| rect.min + vec2(x * scale, y * scale))
            .collect();
        painter.add(egui::Shape::line(points, stroke));
    }
}

/// Shipping `.preferences-nav-brand`: a 26 px accent tile and the product name.
pub fn brand(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.), Sense::hover());
    let tile = Rect::from_min_size(rect.min + vec2(t.number("s-3"), 0.), Vec2::splat(26.));
    let painter = ui.painter();
    painter.rect_filled(tile, t.number("r-md"), t.color("theme-accent"));
    icon(
        painter,
        "capture",
        Rect::from_center_size(tile.center(), Vec2::splat(16.)),
        t.color("theme-accent-ink"),
    );
    let name = text(ui, "Captures", t.number("text-md"), t.color("text"));
    painter.galley(
        pos2(
            tile.right() + t.number("s-4"),
            tile.center().y - name.size().y / 2.,
        ),
        name,
        t.color("text"),
    );
}

/// Shipping `.preferences-nav nav button`: left-aligned, hover and active fills.
pub fn nav_item(ui: &mut egui::Ui, t: &Tokens, label: &str, active: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), t.number("h-md")), Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, label));
    let radius = t.number("r-md");
    let fill = if active {
        t.color("surface-active")
    } else if response.hovered() {
        t.color("surface-hover")
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, radius, fill);
    let color = t.color(if active || response.hovered() {
        "text"
    } else {
        "text-subtle"
    });
    let galley = text(ui, label, t.number("text-md"), color);
    ui.painter().galley(
        pos2(
            rect.left() + t.number("s-4"),
            rect.center().y - galley.size().y / 2.,
        ),
        galley,
        color,
    );
    if response.has_focus() {
        focus_ring(ui, t, rect, radius);
    }
    response
}

/// Shipping `.check-row.switch-row` switch (32×19, accent when on), painted
/// into `rect` for a row whose whole area is the clickable `response`.
/// Disabled rows are faded by the Ui itself.
pub fn paint_switch(
    ui: &egui::Ui,
    t: &Tokens,
    rect: Rect,
    response: &Response,
    value: bool,
    label: &str,
) {
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, value, label)
    });
    let radius = rect.height() / 2.;
    let (fill, border, knob) = if value {
        (
            t.color("theme-accent"),
            Color32::TRANSPARENT,
            t.color("theme-accent-ink"),
        )
    } else {
        let border = if enabled && response.hovered() {
            "text-subtle"
        } else {
            "border-strong"
        };
        (
            t.color("surface-sunken"),
            t.color(border),
            t.color("text-subtle"),
        )
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        radius,
        fill,
        Stroke::new(1., border),
        StrokeKind::Inside,
    );
    let x = rect.left() + 2. + 6.5 + if value { 13. } else { 0. };
    painter.circle_filled(pos2(x, rect.center().y), 6.5, knob);
    if response.has_focus() {
        focus_ring(ui, t, rect, radius);
    }
}

/// Shipping `.ui-segmented`: a sunken track with a raised active segment.
/// Returns the chosen value when a different segment is clicked.
pub fn segmented(
    ui: &mut egui::Ui,
    t: &Tokens,
    label: &str,
    options: &[(&'static str, &'static str)],
    selected: &str,
) -> Option<&'static str> {
    let size = t.number("text-sm");
    let padding = t.number("s-5");
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, name)| text(ui, name, size, t.color("text")))
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + padding * 2.).collect();
    let height = t.number("h-sm") + 8.;
    let (rect, _) = ui.allocate_exact_size(
        vec2(widths.iter().sum::<f32>() + 8., height),
        Sense::hover(),
    );
    let radius = t.number("r-lg");
    ui.painter().rect(
        rect,
        radius,
        t.color("surface-sunken"),
        Stroke::new(1., t.color("border-subtle")),
        StrokeKind::Inside,
    );
    // Shipping `.capture-segmented-indicator` slides over `--dur-4`.
    let indicator = crate::motion::SlidingIndicator::begin(
        ui,
        ui.scope_id().with((label, "indicator")),
        rect.min,
    );
    let mut active_segment = None;
    let mut chosen = None;
    let mut x = rect.left() + 4.;
    for (((value, name), galley), width) in options.iter().zip(galleys).zip(widths) {
        let segment = Rect::from_min_size(pos2(x, rect.top() + 4.), vec2(width, t.number("h-sm")));
        x += width;
        let active = *value == selected;
        let response = ui
            .interact(segment, ui.scope_id().with((label, *value)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, *name)
        });
        let inner = t.number("r-sm");
        if active {
            active_segment = Some(segment);
        }
        let color = t.color(if active || response.hovered() {
            "text"
        } else {
            "text-subtle"
        });
        ui.painter()
            .galley(segment.center() - galley.size() / 2., galley, color);
        if response.has_focus() {
            focus_ring(ui, t, segment, inner);
        }
        if response.clicked() && !active {
            chosen = Some(*value);
        }
    }
    if let Some(segment) = active_segment {
        let inner = t.number("r-sm");
        let fill = t.color("surface-raised");
        let border = Stroke::new(1., t.color("border-subtle"));
        let tween = t.transition(captures_app::motion::Transition::SegmentedIndicator);
        indicator.finish(ui, segment, &tween, |rect| {
            egui::Shape::Vec(vec![
                shadow_xs(ui, rect, inner),
                egui::Shape::Rect(egui::epaint::RectShape::new(
                    rect,
                    inner,
                    fill,
                    border,
                    StrokeKind::Inside,
                )),
            ])
        });
    }
    chosen
}

/// Shipping `.ui-btn` (and `.preferences-history-button` when `raised`).
pub fn button(ui: &mut egui::Ui, t: &Tokens, label: &str, raised: bool) -> Response {
    button_sized(ui, t, label, raised, t.number("h-md"))
}

pub fn button_sized(
    ui: &mut egui::Ui,
    t: &Tokens,
    label: &str,
    raised: bool,
    height: f32,
) -> Response {
    let enabled = ui.is_enabled();
    let galley = text(ui, label, t.number("text-sm"), t.color("text"));
    let size = vec2(galley.size().x + t.number("s-5") * 2., height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let hovered = enabled && response.hovered();
    let radius = t.number("r-md");
    let (fill, border) = if raised {
        (
            if hovered {
                "surface-hover"
            } else {
                "surface-raised"
            },
            "border",
        )
    } else {
        (
            if hovered { "control-hover" } else { "control" },
            if hovered {
                "border-strong"
            } else {
                "control-border"
            },
        )
    };
    if !raised {
        ui.painter().add(shadow_xs(ui, rect, radius));
    }
    ui.painter().rect(
        rect,
        radius,
        t.color(fill),
        Stroke::new(1., t.color(border)),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2., galley, t.color("text"));
    if response.has_focus() {
        focus_ring(ui, t, rect, radius);
    }
    response
}

/// Square find-bar step/close button with a drawn glyph.
pub fn icon_button(ui: &mut egui::Ui, t: &Tokens, glyph: &str, label: &str) -> Response {
    let enabled = ui.is_enabled();
    let side = t.number("h-md");
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let response = response.on_hover_text(label);
    let hovered = enabled && response.hovered();
    let radius = t.number("r-md");
    ui.painter().add(shadow_xs(ui, rect, radius));
    ui.painter().rect(
        rect,
        radius,
        t.color(if hovered { "control-hover" } else { "control" }),
        Stroke::new(
            1.,
            t.color(if hovered {
                "border-strong"
            } else {
                "control-border"
            }),
        ),
        StrokeKind::Inside,
    );
    let color = t.color(if hovered { "text" } else { "text-subtle" });
    let glyph_rect = Rect::from_center_size(rect.center(), Vec2::splat(14.));
    icon(ui.painter(), glyph, glyph_rect, color);
    if response.has_focus() {
        focus_ring(ui, t, rect, radius);
    }
    response
}

/// One `.theme-option` chip. `palette` is (accent, signal); None paints the
/// custom rainbow swatch.
pub fn theme_chip(
    ui: &mut egui::Ui,
    t: &Tokens,
    width: f32,
    name: &str,
    accessibility: &str,
    palette: Option<(Color32, Color32)>,
    active: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 34.), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, active, accessibility)
    });
    let radius = t.number("r-md");
    let painter = ui.painter();
    if active {
        painter.add(shadow_xs(ui, rect, radius));
        painter.rect(
            rect,
            radius,
            t.color("surface-raised"),
            Stroke::new(1., t.color("border")),
            StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, radius, t.color("surface-hover"));
    }
    let gap = t.number("s-3");
    let swatch = Rect::from_min_size(
        pos2(rect.left() + gap, rect.center().y - 9.),
        Vec2::splat(18.),
    );
    let swatch_radius = t.number("r-sm");
    match palette {
        Some((accent, signal)) => {
            painter.rect_filled(swatch, swatch_radius, accent);
            // The signal hue is a wedge in the right half's lower corner.
            let half = swatch.center().x;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(swatch.right(), swatch.top() + swatch_radius * 0.3),
                    pos2(swatch.right(), swatch.bottom() - swatch_radius * 0.3),
                    pos2(swatch.right() - swatch_radius * 0.3, swatch.bottom()),
                    pos2(half, swatch.bottom()),
                ],
                signal,
                Stroke::NONE,
            ));
        }
        None => rainbow(painter, swatch),
    }
    painter.rect_stroke(
        swatch,
        swatch_radius,
        Stroke::new(1., Color32::from_black_alpha(41)),
        StrokeKind::Inside,
    );
    let color = t.color(if active { "text" } else { "text-muted" });
    let label = text(ui, name, t.number("text-sm"), color);
    painter.galley(
        pos2(swatch.right() + gap, rect.center().y - label.size().y / 2.),
        label,
        color,
    );
    if active {
        let check = t.color(if ui.visuals().dark_mode {
            "theme-accent"
        } else {
            "theme-accent-readable"
        });
        icon(
            painter,
            "check",
            Rect::from_center_size(
                pos2(rect.right() - gap - 5., rect.center().y),
                Vec2::splat(11.),
            ),
            check,
        );
    }
    if response.has_focus() {
        focus_ring(ui, t, rect, radius);
    }
    response
}

/// The custom-theme swatch gradient (`.theme-option-custom`).
fn rainbow(painter: &egui::Painter, rect: Rect) {
    let stops = [
        Color32::from_rgb(255, 71, 87),
        Color32::from_rgb(255, 166, 61),
        Color32::from_rgb(245, 225, 61),
        Color32::from_rgb(69, 219, 124),
        Color32::from_rgb(64, 205, 237),
        Color32::from_rgb(192, 82, 245),
        Color32::from_rgb(255, 82, 180),
    ];
    let inner = rect.shrink(1.);
    let mut mesh = egui::Mesh::default();
    let step = inner.width() / (stops.len() - 1) as f32;
    for (index, color) in stops.iter().enumerate() {
        let x = inner.left() + step * index as f32;
        mesh.colored_vertex(pos2(x, inner.top()), *color);
        mesh.colored_vertex(pos2(x, inner.bottom()), *color);
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 2, base + 3);
        }
    }
    painter.add(mesh);
}

/// Shipping `MiniPreviewPlacementPicker`: a small screen with four corner
/// targets and the selected corner's name. Returns a newly chosen value.
pub fn corner_picker(
    ui: &mut egui::Ui,
    t: &Tokens,
    placements: &[(&'static str, &'static str)],
    selected: &str,
    selected_name: &str,
) -> Option<&'static str> {
    let enabled = ui.is_enabled();
    let mut chosen = None;
    let name = text(
        ui,
        selected_name,
        t.number("text-sm"),
        t.color("text-subtle"),
    );
    let gap = t.number("s-2");
    let (bounds, _) = ui.allocate_exact_size(vec2(108., 72. + gap + name.size().y), Sense::hover());
    let screen = Rect::from_min_size(bounds.min, vec2(108., 72.));
    {
        let radius = t.number("r-md");
        ui.painter().add(shadow_xs(ui, screen, radius));
        ui.painter().rect(
            screen,
            radius,
            t.color("surface-sunken"),
            Stroke::new(1., t.color("border-subtle")),
            StrokeKind::Inside,
        );
        let inner = screen.shrink(7.);
        for (value, name) in placements {
            let x = if value.ends_with("left") {
                inner.left()
            } else {
                inner.right() - 22.
            };
            let y = if value.starts_with("top") {
                inner.top()
            } else {
                inner.bottom() - 22.
            };
            let corner = Rect::from_min_size(pos2(x, y), Vec2::splat(22.));
            let active = *value == selected;
            let response = ui.interact(
                corner,
                ui.scope_id().with(("mini-preview-corner", *value)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::RadioButton, enabled, active, *name)
            });
            let (fill, border) = if active {
                (t.color("theme-accent"), Color32::TRANSPARENT)
            } else if enabled && response.hovered() {
                (t.color("surface-raised"), t.color("theme-accent"))
            } else {
                (t.color("surface-raised"), t.color("border-strong"))
            };
            let corner_radius = t.number("r-sm");
            ui.painter().rect(
                corner,
                corner_radius,
                fill,
                Stroke::new(1., border),
                StrokeKind::Inside,
            );
            if response.has_focus() {
                crate::primitives::focus_indicated(ui.ctx());
                ui.painter().rect_stroke(
                    corner.expand(2.),
                    corner_radius + 2.,
                    Stroke::new(2., t.color("theme-accent")),
                    StrokeKind::Outside,
                );
            }
            if response.clicked() && !active {
                chosen = Some(*value);
            }
        }
        ui.painter().galley(
            pos2(bounds.right() - name.size().x, screen.bottom() + gap),
            name,
            t.color("text-subtle"),
        );
    }
    chosen
}

/// Shipping `.preferences-save-status` pill. `kind` is saving/saved/error.
/// `pose` is its `ui-pop-in` entrance: opacity and offset apply to the whole
/// pill and the scale to its capsule.
pub fn status_pill(
    ui: &mut egui::Ui,
    t: &Tokens,
    kind: &str,
    message: &str,
    pose: captures_app::motion::Pose,
) -> Response {
    let (fill, border, color) = match kind {
        "saved" => ("positive-surface", None, "positive-text"),
        "error" => ("danger-surface", None, "danger-text"),
        _ => ("surface-raised", Some("border"), "text-muted"),
    };
    let galley = ui.painter().layout(
        message.to_owned(),
        FontId::proportional(t.number("text-xs")),
        t.color(color),
        360.,
    );
    let marker = 13.;
    let size = vec2(
        galley.size().x + marker + t.number("s-3") + t.number("s-4") * 2.,
        (galley.size().y + 4.).max(t.number("h-sm")),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, message));
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(pose.opacity.clamp(0., 1.) as f32);
    let rect = rect.translate(vec2(0., pose.translate_y as f32));
    let painter = &painter;
    painter.rect(
        rect.scale_from_center(pose.scale as f32),
        rect.height() / 2.,
        t.color(fill),
        border.map_or(Stroke::NONE, |border| Stroke::new(1., t.color(border))),
        StrokeKind::Inside,
    );
    let dot = Rect::from_center_size(
        pos2(rect.left() + t.number("s-4") + marker / 2., rect.center().y),
        Vec2::splat(marker),
    );
    match kind {
        "saved" => {
            painter.circle_filled(dot.center(), marker / 2., t.color("positive"));
            icon(painter, "check", dot.shrink(2.5), t.color("positive-ink"));
        }
        "error" => {
            let mark = text(ui, "!", 9., t.color("danger-text"));
            painter.galley(
                dot.center() - mark.size() / 2.,
                mark,
                t.color("danger-text"),
            );
        }
        _ => {
            // A static spinner frame; the save completes within a few frames.
            painter.circle_stroke(
                dot.center(),
                marker / 2. - 1.,
                Stroke::new(2., t.color("border-strong")),
            );
            let arc: Vec<_> = (0..=8)
                .map(|step| {
                    let angle = -std::f32::consts::FRAC_PI_2 + step as f32 * 0.2;
                    dot.center() + (marker / 2. - 1.) * vec2(angle.cos(), angle.sin())
                })
                .collect();
            painter.add(egui::Shape::line(
                arc,
                Stroke::new(2., t.color("theme-accent")),
            ));
        }
    }
    painter.galley(
        pos2(
            dot.right() + t.number("s-3"),
            rect.center().y - galley.size().y / 2.,
        ),
        galley,
        t.color(color),
    );
    response
}
