//! Fixed-glass tooltips that replace egui's delayed hover text on floating
//! surfaces: the recording HUD (`.recording-tooltip`) and mini-preview icons
//! (`.icon-button::after`). They appear with no delay on hover or keyboard
//! focus and never take the pointer. Callers own placement and progress.

use eframe::egui::{self, Rect, Stroke, StrokeKind};

use crate::tokens::Tokens;

/// The two shipping tooltip families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// `.recording-tooltip > [role="tooltip"]`: `--r-sm` pill with a glass border.
    Hud,
    /// `.icon-button::after`: `--r-xs` chip with `--shadow-sm` and no border.
    Preview,
}

impl Style {
    fn radius(self, tokens: &Tokens) -> f32 {
        tokens.number(match self {
            Self::Hud => "r-sm",
            Self::Preview => "r-xs",
        })
    }
}

/// Paint `galley` centered in a glass chip at `rect`, faded by `progress`
/// (0 hidden … 1 shown), on a tooltip layer above the surface's content.
pub fn paint(
    ui: &egui::Ui,
    tokens: &Tokens,
    layer: egui::Id,
    rect: Rect,
    galley: std::sync::Arc<egui::Galley>,
    progress: f32,
    style: Style,
) {
    if progress <= 0. {
        return;
    }
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Tooltip, layer));
    let radius = style.radius(tokens);
    let fill = tokens.color("glass-strong").gamma_multiply(progress);
    match style {
        Style::Hud => {
            painter.rect(
                rect,
                radius as u8,
                fill,
                Stroke::new(1., tokens.color("glass-border").gamma_multiply(progress)),
                StrokeKind::Inside,
            );
        }
        Style::Preview => {
            let mut shadow = crate::preferences_widgets::shadow_sm(ui.visuals().dark_mode);
            shadow.color = shadow.color.gamma_multiply(progress);
            painter.add(shadow.as_shape(rect, radius));
            painter.rect_filled(rect, radius as u8, fill);
        }
    }
    let text = egui::pos2(
        rect.left() + (rect.width() - galley.size().x) / 2.,
        rect.top() + (rect.height() - galley.size().y) / 2.,
    );
    let color = tokens.color("glass-text").gamma_multiply(progress);
    painter.galley(text, galley, color);
}

/// A mini-preview icon tooltip for `anchor`: text-2xs medium copy, centered
/// and placed by the shared rule, fading and nudging as `progress` runs 0→1.
pub fn preview_icon(
    ui: &egui::Ui,
    tokens: &Tokens,
    anchor: Rect,
    text: &str,
    above: bool,
    progress: f32,
) {
    use captures_app::tray_notice::LogicalRect;
    if progress <= 0. {
        return;
    }
    let color = tokens.color("glass-text").gamma_multiply(progress);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(tokens.number("text-2xs")),
        color,
    );
    let frame = captures_app::preview_chrome::icon_tooltip_frame(
        LogicalRect::new(
            f64::from(anchor.left()),
            f64::from(anchor.top()),
            f64::from(anchor.width()),
            f64::from(anchor.height()),
        ),
        f64::from(galley.size().x),
        f64::from(galley.size().y),
        above,
        f64::from(progress),
    );
    let rect = Rect::from_min_size(
        egui::pos2(frame.x as f32, frame.y as f32),
        egui::vec2(frame.width as f32, frame.height as f32),
    );
    paint(
        ui,
        tokens,
        egui::Id::unique(("preview-icon-tooltip", text)),
        rect,
        galley,
        progress,
        Style::Preview,
    );
}
