use std::collections::BTreeMap;

use crate::SettingsError;

const DARK_INK: Rgb = Rgb(23, 24, 27);
const LIGHT_INK: Rgb = Rgb(255, 255, 255);
const BLACK: Rgb = Rgb(0, 0, 0);
const DARK_SURFACE: Rgb = Rgb(17, 18, 26);
const WEB_CANVAS: Rgb = Rgb(250, 249, 245);

#[derive(Clone, Copy)]
struct Rgb(u8, u8, u8);

/// Normalizes the hexadecimal color syntax accepted by the shipping web theme.
pub fn normalize_hex_color(value: &str) -> Result<String, SettingsError> {
    let value = value.trim().to_ascii_lowercase();
    let digits = value.strip_prefix('#').ok_or_else(|| {
        SettingsError::Validation("color must start with '#' and contain 3 or 6 hex digits".into())
    })?;
    if !matches!(digits.len(), 3 | 6) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SettingsError::Validation(
            "color must start with '#' and contain 3 or 6 hex digits".into(),
        ));
    }
    if digits.len() == 6 {
        Ok(format!("#{digits}"))
    } else {
        let mut result = String::from("#");
        for byte in digits.bytes() {
            result.push(char::from(byte));
            result.push(char::from(byte));
        }
        Ok(result)
    }
}

/// Derives the custom palette and appearance-dependent semantic aliases used by
/// native clients. Keys are the same unprefixed names as `tokens.json` colors.
pub fn custom_colors(
    accent: &str,
    signal: &str,
    light: bool,
) -> Result<BTreeMap<String, [f32; 4]>, SettingsError> {
    let accent = parse(&normalize_hex_color(accent)?)?;
    let signal = parse(&normalize_hex_color(signal)?)?;
    let accent_ink = preferred_ink(accent);
    let signal_ink = preferred_ink(signal);
    let mut colors = BTreeMap::new();

    let accent_values = [
        ("theme-accent", accent),
        ("theme-accent-hover", interactive_shade(accent, accent_ink)),
        ("theme-accent-strong", mix(accent, BLACK, 0.14)),
        ("theme-accent-ink", accent_ink),
        (
            "theme-accent-readable",
            ensure_contrast(accent, WEB_CANVAS, BLACK),
        ),
        (
            "theme-accent-text",
            ensure_contrast(mix(accent, LIGHT_INK, 0.38), DARK_SURFACE, LIGHT_INK),
        ),
        ("theme-accent-text-strong", mix(accent, LIGHT_INK, 0.72)),
        ("theme-accent-surface", mix(DARK_SURFACE, accent, 0.14)),
        (
            "theme-accent-surface-strong",
            mix(DARK_SURFACE, accent, 0.22),
        ),
    ];
    let signal_surface = mix(DARK_SURFACE, signal, 0.14);
    let signal_strong = mix(signal, BLACK, 0.14);
    let signal_text = ensure_contrast(mix(signal, LIGHT_INK, 0.42), DARK_SURFACE, LIGHT_INK);
    let signal_values = [
        ("theme-signal", signal),
        ("theme-signal-hover", interactive_shade(signal, signal_ink)),
        ("theme-signal-strong", signal_strong),
        ("theme-signal-deep", mix(signal, BLACK, 0.3)),
        ("theme-signal-ink", signal_ink),
        ("theme-signal-text", signal_text),
        ("theme-signal-text-strong", mix(signal, LIGHT_INK, 0.64)),
        ("theme-signal-surface", signal_surface),
    ];
    for (name, value) in accent_values.into_iter().chain(signal_values) {
        colors.insert(name.into(), rgba(value, 1.0));
    }
    colors.insert(
        "surface-selected".into(),
        rgba(accent, if light { 0.13 } else { 0.14 }),
    );
    colors.insert(
        "danger-surface".into(),
        if light {
            rgba(signal, 0.1)
        } else {
            rgba(signal_surface, 1.0)
        },
    );
    colors.insert(
        "danger-text".into(),
        rgba(if light { signal_strong } else { signal_text }, 1.0),
    );
    colors.insert(
        "danger-border".into(),
        rgba(signal, if light { 0.3 } else { 0.34 }),
    );
    Ok(colors)
}

fn parse(value: &str) -> Result<Rgb, SettingsError> {
    let channel = |start| {
        u8::from_str_radix(&value[start..start + 2], 16)
            .map_err(|_| SettingsError::Validation("invalid hexadecimal color".into()))
    };
    Ok(Rgb(channel(1)?, channel(3)?, channel(5)?))
}
fn rgba(Rgb(r, g, b): Rgb, alpha: f32) -> [f32; 4] {
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        alpha,
    ]
}
fn mix(Rgb(r1, g1, b1): Rgb, Rgb(r2, g2, b2): Rgb, amount: f64) -> Rgb {
    let channel = |from: u8, to: u8| {
        (f64::from(from) + (f64::from(to) - f64::from(from)) * amount).round() as u8
    };
    Rgb(channel(r1, r2), channel(g1, g2), channel(b1, b2))
}
fn luminance(Rgb(r, g, b): Rgb) -> f64 {
    let linear = |value: u8| {
        let n = f64::from(value) / 255.0;
        if n <= 0.03928 {
            n / 12.92
        } else {
            ((n + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}
fn contrast(first: Rgb, second: Rgb) -> f64 {
    let (a, b) = (luminance(first), luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn preferred_ink(background: Rgb) -> Rgb {
    if contrast(background, DARK_INK) >= 4.5 {
        DARK_INK
    } else if contrast(background, LIGHT_INK) >= 4.5 {
        LIGHT_INK
    } else {
        BLACK
    }
}
fn ensure_contrast(foreground: Rgb, background: Rgb, toward: Rgb) -> Rgb {
    if contrast(foreground, background) >= 4.5 {
        return foreground;
    }
    let mut amount = 0.04;
    while amount <= 1.0 {
        let candidate = mix(foreground, toward, amount);
        if contrast(candidate, background) >= 4.5 {
            return candidate;
        }
        amount += 0.04;
    }
    toward
}
fn interactive_shade(color: Rgb, ink: Rgb) -> Rgb {
    let light_ink = matches!(ink, Rgb(255, 255, 255));
    let toward = if light_ink { BLACK } else { LIGHT_INK };
    ensure_contrast(
        mix(color, toward, if light_ink { 0.06 } else { 0.1 }),
        ink,
        toward,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Case {
        accent: String,
        signal: String,
        light: bool,
        colors: BTreeMap<String, [f32; 4]>,
    }

    #[test]
    fn matches_shipping_typescript_golden_vectors() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/custom-theme-golden.json")).unwrap();
        for case in cases {
            assert_eq!(
                custom_colors(&case.accent, &case.signal, case.light).unwrap(),
                case.colors
            );
        }
    }

    #[test]
    fn normalization_matches_shipping_syntax() {
        assert_eq!(normalize_hex_color(" #AbC ").unwrap(), "#aabbcc");
        assert!(normalize_hex_color("abc").is_err());
        assert!(normalize_hex_color("#abcd").is_err());
    }
}
