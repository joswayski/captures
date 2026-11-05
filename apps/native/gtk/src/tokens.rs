//! Shipping design tokens (`shared/design.css`, `shared/themes.css`), generated
//! at build time by `apps/native/prepare.mjs` into `resources/tokens.json`.
use std::collections::BTreeMap;

use gtk::gdk;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Shadow {
    x: f32,
    y: f32,
    blur: f32,
    spread: f32,
    color: [f32; 4],
}

#[derive(Clone, Deserialize)]
pub struct Tokens {
    colors: BTreeMap<String, [f32; 4]>,
    numbers: BTreeMap<String, f32>,
    #[serde(default)]
    easings: BTreeMap<String, [f64; 4]>,
    #[serde(default)]
    shadows: BTreeMap<String, Vec<Shadow>>,
    #[serde(default)]
    raw: BTreeMap<String, String>,
}

thread_local! {
    /// Every `{appearance}-{theme}` variant, parsed once.
    static VARIANTS: BTreeMap<String, Tokens> =
        serde_json::from_str(include_str!("../resources/tokens.json"))
            .expect("build-generated tokens");
}

/// `"light"` or `"dark"` for the saved appearance, following the desktop for
/// System.
pub fn resolve_appearance(appearance: captures_settings::Appearance) -> &'static str {
    match appearance {
        captures_settings::Appearance::Light => "light",
        captures_settings::Appearance::Dark => "dark",
        captures_settings::Appearance::System => {
            let dark =
                gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme());
            if dark { "dark" } else { "light" }
        }
    }
}

/// The persisted id of a theme (`mustard`, …, `custom`).
pub fn theme_id(theme: captures_settings::ColorTheme) -> String {
    serde_json::to_value(theme)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "mustard".into())
}

impl Tokens {
    /// The `{appearance}-{theme}` variant, such as `light-mustard`.
    pub fn variant(appearance: &str, theme: &str) -> Tokens {
        VARIANTS.with(|all| {
            all.get(&format!("{appearance}-{theme}"))
                .or_else(|| all.get(&format!("{appearance}-mustard")))
                .cloned()
                .expect("mustard tokens")
        })
    }

    /// Tokens for saved settings, with the Custom palette derived like
    /// shipping `buildCustomThemeVariables`.
    pub fn for_settings(settings: &captures_settings::AppSettings, appearance: &str) -> Tokens {
        let theme = theme_id(settings.theme);
        let mut tokens = Tokens::variant(appearance, &theme);
        if theme == "custom"
            && let Ok(colors) = captures_settings::theme::custom_colors(
                &settings.custom_theme.accent,
                &settings.custom_theme.signal,
                appearance == "light",
            )
        {
            tokens.colors.extend(colors);
        }
        tokens
    }

    /// A theme's accent and signal in one appearance, for accent swatches.
    pub fn palette(appearance: &str, theme: &str) -> (gdk::RGBA, gdk::RGBA) {
        let tokens = Tokens::variant(appearance, theme);
        (tokens.rgba("theme-accent"), tokens.rgba("theme-signal"))
    }

    pub fn raw(&self, name: &str) -> &str {
        self.raw.get(name).map(String::as_str).unwrap_or_default()
    }

    pub fn number(&self, name: &str) -> f32 {
        self.numbers[name]
    }

    pub fn rgba(&self, name: &str) -> gdk::RGBA {
        let [r, g, b, a] = self.colors[name];
        gdk::RGBA::new(r, g, b, a)
    }

    pub fn easing(&self, name: &str) -> [f64; 4] {
        self.easings[name]
    }

    /// A color as GTK CSS.
    pub fn css(&self, name: &str) -> String {
        css_rgba(self.colors[name])
    }

    pub fn color_css(&self, name: &str) -> Option<String> {
        self.colors.get(name).copied().map(css_rgba)
    }

    /// A `box-shadow` token's layers as `(x, y, blur, spread, color)`.
    pub fn shadow(&self, name: &str) -> Vec<(f32, f32, f32, f32, gdk::RGBA)> {
        self.shadows[name]
            .iter()
            .map(|s| {
                let [r, g, b, a] = s.color;
                (s.x, s.y, s.blur, s.spread, gdk::RGBA::new(r, g, b, a))
            })
            .collect()
    }

    /// A `box-shadow` token as GTK CSS, first layer on top.
    pub fn shadow_css(&self, name: &str) -> String {
        self.shadows[name]
            .iter()
            .map(|s| {
                format!(
                    "{}px {}px {}px {}px {}",
                    s.x,
                    s.y,
                    s.blur,
                    s.spread,
                    css_rgba(s.color)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// An em-based token (`tracking-tight`) in pixels at `size`.
    pub fn em(&self, name: &str, size: f32) -> f32 {
        let value = self.raw.get(name).map(String::as_str).unwrap_or("0em");
        value.trim_end_matches("em").parse::<f32>().unwrap_or(0.) * size
    }
}

fn css_rgba([r, g, b, a]: [f32; 4]) -> String {
    format!(
        "rgba({},{},{},{})",
        (r * 255.).round(),
        (g * 255.).round(),
        (b * 255.).round(),
        a
    )
}
