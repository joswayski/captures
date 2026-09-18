use eframe::egui::{self, RichText, Stroke};

use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Copy,
    Save,
    OpenHistory,
    Dismiss,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Busy {
    Copy,
    Save,
}

pub struct View<'a> {
    pub texture: &'a egui::TextureHandle,
    pub width: u32,
    pub height: u32,
    pub busy: Option<Busy>,
    pub message: Option<&'a str>,
    pub can_save: bool,
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
    let frame = egui::Frame::new()
        .fill(tokens.color("glass-strong"))
        .stroke(Stroke::new(1., tokens.color("glass-border")))
        .corner_radius(tokens.number("r-xl") as u8)
        .inner_margin(tokens.number("s-3") as i8);
    frame.show(ui, |ui| {
        tokens.glass_controls(ui);
        let media_size = egui::vec2(
            ui.available_width(),
            captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
        );
        let (media, _) = ui.allocate_exact_size(media_size, egui::Sense::hover());
        ui.painter()
            .rect_filled(media, tokens.number("r-lg"), tokens.color("glass-raised"));
        egui::Image::new(view.texture)
            .uv(cover_uv(view.texture.size_vec2(), media.size()))
            .fit_to_exact_size(media.size())
            .corner_radius(tokens.number("r-lg") as u8)
            .paint_at(ui, media);

        let label = view
            .message
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{} × {}", view.width, view.height));
        let label_position =
            media.left_bottom() + egui::vec2(tokens.number("s-3"), -tokens.number("s-3"));
        let galley = ui.painter().layout_no_wrap(
            label,
            egui::FontId::proportional(tokens.number("text-xs")),
            tokens.color("glass-text"),
        );
        let label_rect = egui::Rect::from_min_size(
            label_position - egui::vec2(0., galley.size().y),
            galley.size() + egui::vec2(tokens.number("s-3") * 2., tokens.number("s-2") * 2.),
        );
        ui.painter().rect_filled(
            label_rect,
            tokens.number("r-md"),
            tokens.color("glass-strong"),
        );
        ui.painter().galley(
            label_rect.min + egui::vec2(tokens.number("s-3"), tokens.number("s-2")),
            galley,
            tokens.color("glass-text"),
        );

        ui.add_space(tokens.number("s-3"));
        ui.horizontal(|ui| {
            let enabled = view.busy.is_none();
            if ui
                .add_enabled(enabled, egui::Button::new("Copy"))
                .on_hover_text("Copy full-resolution pixels")
                .clicked()
            {
                action = Some(Action::Copy);
            }
            if ui
                .add_enabled(enabled && view.can_save, egui::Button::new("Save"))
                .on_hover_text("Save with current screenshot preferences")
                .clicked()
            {
                action = Some(Action::Save);
            }
            if ui
                .add_enabled(enabled, egui::Button::new("History"))
                .on_hover_text("Select this capture in history")
                .clicked()
            {
                action = Some(Action::OpenHistory);
            }
            if ui
                .button(RichText::new("×").color(tokens.color("glass-text")))
                .on_hover_text("Dismiss preview")
                .clicked()
            {
                action = Some(Action::Dismiss);
            }
        });
    });
    action
}

fn cover_uv(image: egui::Vec2, target: egui::Vec2) -> egui::Rect {
    let image_aspect = image.x / image.y.max(1.);
    let target_aspect = target.x / target.y.max(1.);
    if image_aspect > target_aspect {
        let visible = target_aspect / image_aspect;
        let inset = (1. - visible) / 2.;
        egui::Rect::from_min_max(egui::pos2(inset, 0.), egui::pos2(1. - inset, 1.))
    } else {
        let visible = image_aspect / target_aspect;
        let inset = (1. - visible) / 2.;
        egui::Rect::from_min_max(egui::pos2(0., inset), egui::pos2(1., 1. - inset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crop_is_centered_and_preserves_aspect() {
        let wide = cover_uv(egui::vec2(400., 100.), egui::vec2(200., 100.));
        assert_eq!(wide.min, egui::pos2(0.25, 0.));
        assert_eq!(wide.max, egui::pos2(0.75, 1.));

        let tall = cover_uv(egui::vec2(100., 400.), egui::vec2(100., 200.));
        assert_eq!(tall.min, egui::pos2(0., 0.25));
        assert_eq!(tall.max, egui::pos2(1., 0.75));
    }
}
