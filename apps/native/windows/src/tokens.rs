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

    /// Shipping `buildCustomThemeVariables` (`shared/themes.ts`) for the
    /// accent and signal tokens this host draws with.
    pub fn with_custom(mut self, accent: &str, signal: &str) -> Tokens {
        let accent = parse_hex(accent).unwrap_or([0x32, 0xd3, 0xff]);
        let signal = parse_hex(signal).unwrap_or([0xff, 0x4f, 0xc3]);
        let (accent_ink, signal_ink) = (preferred_ink(accent), preferred_ink(signal));
        let black = [0, 0, 0];
        let set = [
            ("theme-accent", accent),
            ("theme-accent-hover", interactive_shade(accent, accent_ink)),
            ("theme-accent-strong", mix(accent, black, 0.14)),
            ("theme-accent-ink", accent_ink),
            (
                "theme-accent-readable",
                ensure_contrast(accent, WEB_CANVAS, black),
            ),
            ("theme-signal", signal),
            ("theme-signal-hover", interactive_shade(signal, signal_ink)),
            ("theme-signal-strong", mix(signal, black, 0.14)),
            ("theme-signal-deep", mix(signal, black, 0.3)),
            ("theme-signal-ink", signal_ink),
        ];
        for (name, [r, g, b]) in set {
            self.colors.insert(
                name.into(),
                [
                    f32::from(r) / 255.,
                    f32::from(g) / 255.,
                    f32::from(b) / 255.,
                    1.,
                ],
            );
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

type Rgb = [u8; 3];

const DARK_INK: Rgb = [0x17, 0x18, 0x1b];
const LIGHT_INK: Rgb = [0xff, 0xff, 0xff];
const BLACK_INK: Rgb = [0, 0, 0];
const WEB_CANVAS: Rgb = [0xfa, 0xf9, 0xf5];

/// `#rrggbb` or `#rgb`, as `normalizeHexColor`.
pub fn parse_hex(value: &str) -> Option<Rgb> {
    let hex = value.trim().strip_prefix('#')?;
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    match digits.as_slice() {
        [r, g, b] => Some([r * 17, g * 17, b * 17]),
        [r1, r2, g1, g2, b1, b2] => Some([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2]),
        _ => None,
    }
}

fn mix(first: Rgb, second: Rgb, amount: f32) -> Rgb {
    let channel =
        |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    [
        channel(first[0], second[0]),
        channel(first[1], second[1]),
        channel(first[2], second[2]),
    ]
}

fn luminance(color: Rgb) -> f32 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

fn contrast(first: Rgb, second: Rgb) -> f32 {
    let (a, b) = (luminance(first), luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn preferred_ink(background: Rgb) -> Rgb {
    if contrast(background, DARK_INK) >= 4.5 {
        DARK_INK
    } else if contrast(background, LIGHT_INK) >= 4.5 {
        LIGHT_INK
    } else {
        BLACK_INK
    }
}

fn ensure_contrast(foreground: Rgb, background: Rgb, toward: Rgb) -> Rgb {
    if contrast(foreground, background) >= 4.5 {
        return foreground;
    }
    (1..=25)
        .map(|step| mix(foreground, toward, 0.04 * step as f32))
        .find(|candidate| contrast(*candidate, background) >= 4.5)
        .unwrap_or(toward)
}

fn interactive_shade(color: Rgb, ink: Rgb) -> Rgb {
    let toward = if ink == LIGHT_INK {
        BLACK_INK
    } else {
        LIGHT_INK
    };
    let amount = if ink == LIGHT_INK { 0.06 } else { 0.1 };
    ensure_contrast(mix(color, toward, amount), ink, toward)
}
