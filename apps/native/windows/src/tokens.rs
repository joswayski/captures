//! Shipping design tokens (`shared/design.css`, `shared/themes.css`), generated
//! at build time by `apps/native/prepare.mjs` into `resources/tokens.json`.
use std::collections::BTreeMap;
use std::sync::OnceLock;

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
    /// `light` or `dark`.
    #[serde(skip)]
    pub appearance: String,
}

fn all() -> &'static BTreeMap<String, Tokens> {
    static ALL: OnceLock<BTreeMap<String, Tokens>> = OnceLock::new();
    ALL.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/tokens.json"))
            .expect("build-generated tokens")
    })
}

impl Tokens {
    /// The `{appearance}-{theme}` variant, such as `light-mustard`. `custom`
    /// starts from Mustard; apply [`Tokens::with_custom`] for its colors.
    pub fn variant(appearance: &str, theme: &str) -> Tokens {
        let all = all();
        let mut tokens = all
            .get(&format!("{appearance}-{theme}"))
            .or_else(|| all.get(&format!("{appearance}-mustard")))
            .expect("mustard tokens")
            .clone();
        tokens.appearance = appearance.into();
        tokens
    }

    /// `(accent, signal)` of a preset accent theme in this appearance, for
    /// the `.theme-option-preview` swatches.
    pub fn swatch(&self, theme: &str) -> (D2D1_COLOR_F, D2D1_COLOR_F) {
        let variant = all()
            .get(&format!("{}-{theme}", self.appearance))
            .unwrap_or(self);
        (variant.color("theme-accent"), variant.color("theme-signal"))
    }

    /// Shipping `buildCustomThemeVariables`, through the shared
    /// `captures_settings::theme::custom_colors` derivation (the accent and
    /// signal shades plus the appearance-dependent aliases). An invalid color
    /// leaves the Mustard tokens.
    pub fn with_custom(mut self, accent: &str, signal: &str) -> Tokens {
        let light = self.appearance == "light";
        if let Ok(colors) = captures_settings::theme::custom_colors(accent, signal, light) {
            self.colors.extend(colors);
        }
        self
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

impl captures_app::motion::MotionTokens for Tokens {
    fn duration_ms(&self, token: &str) -> Option<f64> {
        self.numbers.get(token).map(|ms| f64::from(*ms))
    }

    fn easing(&self, token: &str) -> Option<[f64; 4]> {
        self.easings.get(token).copied()
    }
}
