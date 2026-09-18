use std::collections::BTreeMap;

use eframe::egui::{self, Color32, FontId, Stroke, TextStyle};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Tokens {
    colors: BTreeMap<String, [f32; 4]>,
    numbers: BTreeMap<String, f32>,
}

pub fn load() -> BTreeMap<String, Tokens> {
    serde_json::from_str(include_str!("../resources/tokens.json")).expect("build-generated tokens")
}

impl Tokens {
    pub fn color(&self, name: &str) -> Color32 {
        let [r, g, b, a] = self.colors[name].map(|c| (c * 255.).round() as u8);
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }
    pub fn number(&self, name: &str) -> f32 {
        self.numbers[name]
    }

    pub fn glass_controls(&self, ui: &mut egui::Ui) {
        let v = ui.visuals_mut();
        v.override_text_color = Some(self.color("glass-text"));
        for (widget, fill) in [
            (&mut v.widgets.noninteractive, "glass-strong"),
            (&mut v.widgets.inactive, "glass-raised"),
            (&mut v.widgets.hovered, "glass-hover"),
            (&mut v.widgets.active, "glass-active"),
            (&mut v.widgets.open, "glass-raised"),
        ] {
            widget.bg_fill = self.color(fill);
            widget.weak_bg_fill = self.color(fill);
            widget.bg_stroke = Stroke::new(1., self.color("glass-border"));
            widget.fg_stroke = Stroke::new(1., self.color("glass-text"));
        }
    }

    pub fn apply(&self, ctx: &egui::Context, light: bool) {
        let mut style = egui::Style {
            visuals: if light {
                egui::Visuals::light()
            } else {
                egui::Visuals::dark()
            },
            ..Default::default()
        };
        let v = &mut style.visuals;
        v.override_text_color = Some(self.color("text"));
        v.panel_fill = self.color("surface-canvas");
        v.window_fill = self.color("surface-raised");
        v.extreme_bg_color = self.color("surface-sunken");
        v.selection.bg_fill = self.color("surface-selected");
        v.selection.stroke = Stroke::new(1., self.color("theme-accent"));
        for (widget, fill, border) in [
            (&mut v.widgets.noninteractive, "surface-raised", "border"),
            (&mut v.widgets.inactive, "control", "control-border"),
            (&mut v.widgets.hovered, "surface-hover", "control-border"),
            (&mut v.widgets.active, "surface-active", "theme-accent"),
            (&mut v.widgets.open, "surface-selected", "theme-accent"),
        ] {
            widget.bg_fill = self.color(fill);
            widget.weak_bg_fill = self.color(fill);
            widget.bg_stroke = Stroke::new(1., self.color(border));
            widget.fg_stroke = Stroke::new(1., self.color("text"));
            widget.corner_radius = (self.number("r-md") as u8).into();
        }
        style.spacing.item_spacing = egui::vec2(self.number("s-4"), self.number("s-5"));
        style.spacing.button_padding = egui::vec2(self.number("s-5"), self.number("s-4"));
        style.spacing.interact_size.y = self.number("h-md");
        style.text_styles.insert(
            TextStyle::Body,
            FontId::proportional(self.number("text-md")),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::proportional(self.number("text-md")),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::proportional(self.number("text-sm")),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::proportional(self.number("text-xl")),
        );
        let theme = if light {
            egui::Theme::Light
        } else {
            egui::Theme::Dark
        };
        ctx.set_theme(theme);
        ctx.set_style_of(theme, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shipped_tokens_have_all_variants_and_fixed_media_palette() {
        let variants = load();
        assert_eq!(variants.len(), 18);
        assert_eq!(
            variants["light-cobalt"].color("surface-raised"),
            Color32::WHITE
        );
        assert_eq!(
            variants["dark-mustard"].color("theme-accent"),
            Color32::from_rgb(255, 202, 40)
        );
        for theme in crate::options::THEMES {
            assert_eq!(
                variants[&format!("light-{theme}")].color("glass-strong"),
                variants[&format!("dark-{theme}")].color("glass-strong")
            );
        }
    }
}
