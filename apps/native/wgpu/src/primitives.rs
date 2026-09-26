//! Shared shipping UI primitives drawn from design tokens, used by every
//! wgpu surface: the focus ring (`--focus-ring-tight`) and scroll bars
//! (`styles/base.css`).

use std::sync::Arc;

use eframe::egui::{self, Color32, Rect, Stroke, StrokeKind};

use crate::tokens::Tokens;

/// `--focus-ring-tight`: a 2 px translucent accent ring just outside `rect`.
fn paint_ring(painter: &egui::Painter, accent: Color32, rect: Rect, radius: f32) {
    painter.rect_stroke(
        rect.expand(1.),
        radius + 1.,
        Stroke::new(2., accent.gamma_multiply(0.45)),
        StrokeKind::Outside,
    );
}

fn indicated_key(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique("captures-focus-indicated").with(ctx.viewport_id())
}

/// Record that the focused widget drew its own focus indicator this pass,
/// so the global ring (see [`install_focus_ring`]) does not repeat it.
pub fn focus_indicated(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    let key = indicated_key(ctx);
    ctx.data_mut(|data| data.insert_temp(key, pass));
}

/// The token focus ring around a custom control's `rect`.
pub fn focus_ring(ui: &egui::Ui, t: &Tokens, rect: Rect, radius: f32) {
    paint_ring(ui.painter(), t.color("theme-accent"), rect, radius);
    focus_indicated(ui.ctx());
}

/// Draw the token focus ring around whichever stock egui control (button,
/// ComboBox, DragValue, text edit, checkbox…) holds keyboard focus at the end
/// of every pass, in every viewport. Custom controls that paint their own
/// indicator call [`focus_ring`] or [`focus_indicated`] instead.
pub fn install_focus_ring(ctx: &egui::Context) {
    ctx.on_end_pass(
        "captures-focus-ring",
        Arc::new(|ui: &mut egui::Ui| {
            let ctx = ui.ctx().clone();
            let Some(id) = ctx.memory(|memory| memory.focused()) else {
                return;
            };
            let key = indicated_key(&ctx);
            let pass = ctx.cumulative_pass_nr();
            if ctx.data(|data| data.get_temp::<u64>(key)) == Some(pass) {
                return;
            }
            let Some(response) = ctx.read_response(id) else {
                return;
            };
            if !response.rect.is_positive() || !response.interact_rect.is_positive() {
                return;
            }
            let style = ctx.global_style();
            let radius = f32::from(style.visuals.widgets.inactive.corner_radius.nw);
            let painter = ctx
                .layer_painter(response.layer_id)
                .with_clip_rect(response.interact_rect.expand(4.));
            paint_ring(
                &painter,
                style.visuals.selection.stroke.color,
                response.rect,
                radius,
            );
        }),
    );
}

/// Shipping scroll bars: a thin pill thumb over a transparent track. The
/// 10 px bar keeps a 3 px transparent border, leaving a 4 px thumb; bars
/// overlay content so they do not reflow any layout.
pub fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating: true,
        bar_width: 4.,
        floating_width: 4.,
        floating_allocated_width: 0.,
        bar_inner_margin: 3.,
        bar_outer_margin: 3.,
        handle_min_length: 20.,
        foreground_color: false,
        dormant_background_opacity: 0.,
        active_background_opacity: 0.,
        interact_background_opacity: 0.,
        dormant_handle_opacity: 1.,
        active_handle_opacity: 1.,
        interact_handle_opacity: 1.,
        ..egui::style::ScrollStyle::solid()
    }
}

/// egui paints a scroll thumb with the `bg_fill` of the surrounding Ui's
/// widget visuals: `--border-strong`, lifting to `--text-faint` on hover
/// (the glass palette's equivalents over media).
fn scrollbar_visuals(widgets: &mut egui::style::Widgets, t: &Tokens, glass: bool) {
    let (idle, hover) = if glass {
        ("glass-border-strong", "glass-text-subtle")
    } else {
        ("border-strong", "text-faint")
    };
    widgets.inactive.bg_fill = t.color(idle);
    widgets.hovered.bg_fill = t.color(hover);
    widgets.active.bg_fill = t.color(hover);
    let pill = (t.number("r-pill").min(255.) as u8).into();
    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
    ] {
        widget.corner_radius = pill;
    }
}

fn scrolled<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    glass: bool,
    show: impl FnOnce(&mut egui::Ui, &egui::style::Widgets) -> R,
) -> R {
    let content = ui.visuals().widgets.clone();
    scrollbar_visuals(&mut ui.visuals_mut().widgets, t, glass);
    let result = show(ui, &content);
    ui.visuals_mut().widgets = content;
    result
}

/// Show `area` with shipping scroll bars; `add` sees the surrounding visuals.
pub fn scroll_area<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, false, |ui, content| {
        area.show(ui, |ui| {
            ui.visuals_mut().widgets = content.clone();
            add(ui)
        })
    })
}

/// [`scroll_area`] over the glass media palette.
pub fn glass_scroll_area<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, true, |ui, content| {
        area.show(ui, |ui| {
            ui.visuals_mut().widgets = content.clone();
            add(ui)
        })
    })
}

/// [`scroll_area`] for `ScrollArea::show_viewport`.
pub fn scroll_viewport<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut egui::Ui, Rect) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, false, |ui, content| {
        area.show_viewport(ui, |ui, viewport| {
            ui.visuals_mut().widgets = content.clone();
            add(ui, viewport)
        })
    })
}

/// [`scroll_area`] for `ScrollArea::show_rows`.
pub fn scroll_rows<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    area: egui::ScrollArea,
    row_height: f32,
    rows: usize,
    add: impl FnOnce(&mut egui::Ui, std::ops::Range<usize>) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrolled(ui, t, false, |ui, content| {
        area.show_rows(ui, row_height, rows, |ui, range| {
            ui.visuals_mut().widgets = content.clone();
            add(ui, range)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (egui::Context, Tokens) {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        tokens.apply(&ctx, true);
        (ctx, tokens)
    }

    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut add: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 400.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| add(ui));
            },
        );
        output.textures_delta.clear();
        output
    }

    fn strokes(output: &egui::FullOutput) -> Vec<Stroke> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) => Some(rect.stroke),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn focused_stock_controls_get_the_token_ring_once() {
        let (ctx, t) = setup();
        install_focus_ring(&ctx);
        let ring = Stroke::new(2., t.color("theme-accent").gamma_multiply(0.45));
        let mut button_id = None;
        for _ in 0..2 {
            frame(&ctx, vec![], |ui| {
                button_id = Some(ui.button("Send feedback").id);
            });
        }
        let id = button_id.unwrap();
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(&ctx, vec![], |ui| {
            let _ = ui.button("Send feedback");
        });
        let output = frame(&ctx, vec![], |ui| {
            let _ = ui.button("Send feedback");
        });
        let rings = strokes(&output).into_iter().filter(|s| *s == ring).count();
        assert_eq!(rings, 1, "the focused stock button gets one token ring");

        // A custom control that draws its own indicator is not ringed twice.
        let custom = egui::Id::unique("custom-focus");
        let draw = |ui: &mut egui::Ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(40., 20.), egui::Sense::hover());
            let response = ui.interact(rect, custom, egui::Sense::click());
            if response.has_focus() {
                focus_ring(ui, &t, rect, 4.);
            }
        };
        frame(&ctx, vec![], draw);
        ctx.memory_mut(|memory| memory.request_focus(custom));
        frame(&ctx, vec![], draw);
        let output = frame(&ctx, vec![], draw);
        let rings = strokes(&output).into_iter().filter(|s| *s == ring).count();
        assert_eq!(rings, 1, "a custom indicator suppresses the global ring");
    }

    #[test]
    fn scroll_bars_use_thumb_tokens_but_content_keeps_its_visuals() {
        let (ctx, t) = setup();
        let control = ctx.global_style().visuals.widgets.inactive.bg_fill;
        let mut inside = None;
        let mut after = None;
        frame(&ctx, vec![], |ui| {
            scroll_area(ui, &t, egui::ScrollArea::vertical().max_height(50.), |ui| {
                inside = Some(ui.visuals().widgets.inactive.bg_fill);
                ui.allocate_space(egui::vec2(100., 400.));
            });
            after = Some(ui.visuals().widgets.inactive.bg_fill);
        });
        assert_eq!(inside, Some(control));
        assert_eq!(after, Some(control));
        let style = scroll_style();
        assert!(style.floating);
        assert_eq!(style.bar_width, 4.);
        assert_eq!(style.dormant_background_opacity, 0.);
        assert_eq!(style.dormant_handle_opacity, 1.);
        let mut widgets = ctx.global_style().visuals.widgets.clone();
        scrollbar_visuals(&mut widgets, &t, false);
        assert_eq!(widgets.inactive.bg_fill, t.color("border-strong"));
        assert_eq!(widgets.hovered.bg_fill, t.color("text-faint"));
    }
}
