//! Shipping design tokens (`shared/design.css`, `shared/themes.css`), generated
//! at build time by `apps/native/prepare.mjs` into `resources/tokens.json`.
use std::collections::BTreeMap;

use serde::Deserialize;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[derive(Clone, Copy, Deserialize)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: [f32; 4],
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
        all.remove(&format!("{appearance}-{theme}"))
            .or_else(|| all.remove(&format!("{appearance}-mustard")))
            .expect("mustard tokens")
    }

    pub fn n(&self, name: &str) -> f32 {
        self.numbers[name]
    }

    pub fn color(&self, name: &str) -> D2D1_COLOR_F {
        let [r, g, b, a] = self.colors[name];
        D2D1_COLOR_F { r, g, b, a }
    }

    /// A color with its alpha multiplied by `alpha`.
    pub fn color_alpha(&self, name: &str, alpha: f32) -> D2D1_COLOR_F {
        let mut color = self.color(name);
        color.a *= alpha;
        color
    }

    pub fn easing(&self, name: &str) -> [f64; 4] {
        self.easings[name]
    }

    pub fn shadow(&self, name: &str) -> &[Shadow] {
        &self.shadows[name]
    }

    /// An em-based token (`tracking-tight`) in pixels at `size`.
    pub fn em(&self, name: &str, size: f32) -> f32 {
        let value = self.raw.get(name).map(String::as_str).unwrap_or("0em");
        value.trim_end_matches("em").parse::<f32>().unwrap_or(0.) * size
    }
}
