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

impl Tokens {
    /// The `{appearance}-{theme}` variant, such as `light-mustard`.
    pub fn variant(appearance: &str, theme: &str) -> Tokens {
        let mut all: BTreeMap<String, Tokens> =
            serde_json::from_str(include_str!("../resources/tokens.json"))
                .expect("build-generated tokens");
        let key = format!("{appearance}-{theme}");
        all.remove(&key)
            .or_else(|| all.remove(&format!("{appearance}-mustard")))
            .expect("mustard tokens")
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

    /// A token's CSS source text, such as `16px` or `140ms`.
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.raw.get(name).map(String::as_str)
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
