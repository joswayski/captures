//! Shipping screenshot-editor Properties layout (`ScreenshotEditor.tsx`
//! `.screenshot-properties`, `styles/editor-image.css`): token sections with a
//! rule below each, labels stacked over their control, two-column number
//! pairs, `.screenshot-check-row` checkboxes, hint paragraphs and the
//! `.screenshot-property-actions` button pair.
use super::*;
use eframe::egui::{FontId, Sense, Stroke, StrokeKind, pos2, vec2};

/// `.screenshot-property-section`: `--s-5` padding above and below, `--s-5`
/// between its items and a `--editor-border` rule underneath. Callers keep
/// the surrounding Properties area at zero item spacing.
pub(super) fn section<R>(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let pad = tokens.number("s-5");
    ui.add_space(pad);
    let inner = ui
        .scope(|ui| {
            ui.spacing_mut().item_spacing.y = pad;
            add(ui)
        })
        .inner;
    ui.add_space(pad);
    rule(ui, tokens);
    inner
}

/// The section's bottom border, spanning the panel's padding.
fn rule(ui: &mut egui::Ui, tokens: &Tokens) {
    let y = ui.cursor().top() - 0.5;
    ui.painter().hline(
        ui.max_rect().expand2(vec2(8., 0.)).x_range(),
        y,
        Stroke::new(1., tokens.color("border-subtle")),
    );
}

/// `.screenshot-property-section label` text: `--text-sm` in
/// `--editor-text-muted`.
pub(super) fn label_text(ui: &mut egui::Ui, tokens: &Tokens, text: &str) -> egui::Response {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .size(tokens.number("text-sm"))
                .color(tokens.color("text-muted")),
        )
        .wrap(),
    )
}

/// A shipping `<label>Name<control/></label>`: the name, then the control
/// `--s-3` below it.
pub(super) fn labelled<R>(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    text: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = tokens.number("s-3");
        label_text(ui, tokens, text);
        add(ui)
    })
    .inner
}

/// `.screenshot-property-section > p`: `--text-sm` hint copy in
/// `--editor-text-subtle`.
pub(super) fn hint(ui: &mut egui::Ui, tokens: &Tokens, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .size(tokens.number("text-sm"))
                .color(tokens.color("text-subtle")),
        )
        .wrap(),
    );
}

/// `.screenshot-number-pair` (and the Crop actions grid): two equal columns
/// `gap` apart. `add` runs once per column with its index.
pub(super) fn columns(ui: &mut egui::Ui, gap: f32, mut add: impl FnMut(&mut egui::Ui, usize, f32)) {
    let width = ((ui.available_width() - gap) / 2.).floor().max(0.);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for column in 0..2 {
            ui.vertical(|ui| {
                ui.set_width(width);
                add(ui, column, width);
            });
        }
    });
}

/// `.screenshot-number-pair`: two columns `--s-4` apart.
pub(super) fn pair(ui: &mut egui::Ui, tokens: &Tokens, add: impl FnMut(&mut egui::Ui, usize, f32)) {
    columns(ui, tokens.number("s-4"), add);
}

/// Shipping `.screenshot-check-row`: a 15 pt token checkbox (accent fill and
/// ink check when on), `--s-4` gap, then the label, in a 28 pt row that
/// toggles anywhere. Returns the response, marked changed on toggle.
pub(super) fn check_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    checked: &mut bool,
    label: &str,
) -> egui::Response {
    let height = tokens.number("h-sm");
    let (rect, mut response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let enabled = ui.is_enabled();
    if enabled && response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    let side = 15.;
    let dim = |color: egui::Color32| {
        if enabled {
            color
        } else {
            color.gamma_multiply(0.38)
        }
    };
    let bx = egui::Rect::from_min_size(
        pos2(rect.left(), rect.center().y - side / 2.),
        vec2(side, side),
    );
    let painter = ui.painter();
    let (fill, border) = if *checked {
        (tokens.color("theme-accent"), tokens.color("theme-accent"))
    } else {
        (tokens.color("surface-field"), tokens.color("border-strong"))
    };
    painter.rect(
        bx,
        tokens.number("r-xs"),
        dim(fill),
        Stroke::new(1., dim(border)),
        StrokeKind::Inside,
    );
    if *checked {
        // The CSS check: a 7.5 × 4 L rotated −45°.
        let c = bx.center() + vec2(0., -0.5);
        painter.add(egui::Shape::line(
            vec![c + vec2(-3.4, -0.4), c + vec2(-1., 2.), c + vec2(3.6, -2.6)],
            Stroke::new(1.75, dim(tokens.color("theme-accent-ink"))),
        ));
    }
    if response.has_focus() {
        crate::primitives::focus_ring(ui, tokens, bx.expand(2.), tokens.number("r-xs") + 2.);
    }
    let text = ui.painter().layout(
        label.to_owned(),
        FontId::proportional(tokens.number("text-sm")),
        dim(tokens.color("text-muted")),
        (rect.width() - side - tokens.number("s-4")).max(1.),
    );
    ui.painter().galley(
        pos2(
            bx.right() + tokens.number("s-4"),
            rect.center().y - text.size().y / 2.,
        ),
        text,
        dim(tokens.color("text-muted")),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *checked, label)
    });
    response
}

/// Shipping `.screenshot-property-actions button`: a neutral `--h-md`
/// control button filling its column.
pub(super) fn action_button(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    label: &str,
    width: f32,
    enabled: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(width, tokens.number("h-md")),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let alpha = if enabled { 1. } else { 0.45 };
    ui.painter().rect(
        rect,
        tokens.number("r-md"),
        tokens
            .color(if hovered { "control-hover" } else { "control" })
            .gamma_multiply(alpha),
        Stroke::new(
            1.,
            tokens
                .color(if hovered {
                    "border-strong"
                } else {
                    "control-border"
                })
                .gamma_multiply(alpha),
        ),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(tokens.number("text-sm")),
        tokens.color("text").gamma_multiply(alpha),
    );
    if response.has_focus() {
        crate::primitives::focus_ring(ui, tokens, rect, tokens.number("r-md"));
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response
}

/// Shipping `.screenshot-drop-shadow-settings` indent: the checkbox and the
/// row gap, so nested knobs align with the "Drop shadow" label.
pub(super) fn indented<R>(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let indent = 15. + tokens.number("s-4");
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        ui.add_space(indent);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = tokens.number("s-5");
            add(ui)
        })
        .inner
    })
    .inner
}
