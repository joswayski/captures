use std::collections::BTreeMap;

use eframe::egui::{self, Color32, FontId, Stroke, TextStyle};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Tokens {
    colors: BTreeMap<String, [f32; 4]>,
    numbers: BTreeMap<String, f32>,
    /// `--ease-*` control points; see `captures_app::motion`.
    #[serde(default)]
    easings: BTreeMap<String, [f64; 4]>,
}

impl captures_app::motion::MotionTokens for Tokens {
    fn duration_ms(&self, token: &str) -> Option<f64> {
        self.numbers.get(token).map(|&ms| f64::from(ms))
    }
    fn easing(&self, token: &str) -> Option<[f64; 4]> {
        self.easings.get(token).copied()
    }
}

pub fn load() -> BTreeMap<String, Tokens> {
    serde_json::from_str(include_str!("../resources/tokens.json")).expect("build-generated tokens")
}

impl Tokens {
    pub fn with_custom_colors(mut self, accent: &str, signal: &str, light: bool) -> Self {
        if let Ok(colors) = captures_settings::theme::custom_colors(accent, signal, light) {
            self.colors.extend(colors);
        }
        self
    }

    pub fn color(&self, name: &str) -> Color32 {
        let [r, g, b, a] = self.colors[name].map(|c| (c * 255.).round() as u8);
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }
    pub fn number(&self, name: &str) -> f32 {
        self.numbers[name]
    }

    /// A shipping animation resolved against these tokens. Every motion's
    /// tokens are generated at build time; the test below checks them all.
    pub fn motion(&self, motion: captures_app::motion::Motion) -> captures_app::motion::Animation {
        motion.resolve(self).expect("build-generated motion tokens")
    }

    /// A shipping transition resolved against these tokens.
    pub fn transition(
        &self,
        transition: captures_app::motion::Transition,
    ) -> captures_app::motion::Tween {
        transition
            .resolve(self)
            .expect("build-generated motion tokens")
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
        style.spacing.scroll = crate::primitives::scroll_style();
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
        for tokens in variants.values() {
            for motion in captures_app::motion::Motion::ALL {
                assert!(motion.resolve(tokens).is_some(), "{}", motion.name());
            }
            for transition in captures_app::motion::Transition::ALL {
                assert!(transition.resolve(tokens).is_some());
            }
        }
        for theme in crate::options::THEMES {
            assert_eq!(
                variants[&format!("light-{theme}")].color("glass-strong"),
                variants[&format!("dark-{theme}")].color("glass-strong")
            );
        }
    }
}
