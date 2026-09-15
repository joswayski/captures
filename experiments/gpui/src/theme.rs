//! Captures' semantic colors, not a GPUI component library's theme.
use gpui::{Rgba, rgb, rgba};

#[derive(Clone, Copy)]
pub struct Theme {
    pub canvas: Rgba,
    pub raised: Rgba,
    pub field: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub subtle: Rgba,
    pub border: Rgba,
    pub hover: Rgba,
    pub accent: Rgba,
    pub signal: Rgba,
    pub positive: Rgba,
    pub glass: Rgba,
    pub glass_text: Rgba,
    pub glass_muted: Rgba,
    pub glass_border: Rgba,
}

impl Theme {
    pub fn new(light: bool) -> Self {
        Self::configured(light, "mustard", "#32d3ff", "#ff4fc3")
    }

    pub fn configured(light: bool, name: &str, custom_accent: &str, custom_signal: &str) -> Self {
        let (accent, signal) = match name {
            "ember" => (0xff7a45, 0xff3d71),
            "rose" => (0xff5ba7, 0xff6b45),
            "violet" => (0xc026d3, 0xff4f88),
            "cobalt" => (0x2563eb, 0xff5a64),
            "aqua" => (0x31cbd8, 0xff5176),
            "mint" => (0x67d5a5, 0xf15a48),
            "lime" => (0xb6db45, 0xf15a48),
            "mono" => (0xededed, 0xa1a1aa),
            "custom" => (
                parse_hex(custom_accent).unwrap_or(0x32d3ff),
                parse_hex(custom_signal).unwrap_or(0xff4fc3),
            ),
            _ => (0xffca28, 0xef4650),
        };
        Self {
            canvas: rgb(if light { 0xf5f5f7 } else { 0x101014 }),
            raised: rgb(if light { 0xffffff } else { 0x16161b }),
            field: rgb(if light { 0xffffff } else { 0x0e0e12 }),
            text: rgb(if light { 0x131318 } else { 0xf2f2f4 }),
            muted: rgb(if light { 0x5c5c69 } else { 0xb9b9c4 }),
            subtle: rgb(if light { 0x7d7d8c } else { 0x8b8b98 }),
            border: rgba(if light { 0x1313181f } else { 0xffffff1a }),
            hover: rgba(if light { 0x1313180b } else { 0xffffff0d }),
            accent: rgb(accent),
            signal: rgb(signal),
            positive: rgb(0x35a35d),
            glass: rgba(0x0f0f12ed),
            glass_text: rgb(0xf6f6f8),
            glass_muted: rgba(0xf6f6f8a3),
            glass_border: rgba(0xffffff1c),
        }
    }
}

fn parse_hex(value: &str) -> Option<u32> {
    let value = value.strip_prefix('#').unwrap_or(value);
    (value.len() == 6)
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}

pub fn font() -> &'static str {
    if cfg!(target_os = "macos") {
        ".AppleSystemUIFont"
    } else if cfg!(target_os = "windows") {
        "Segoe UI"
    } else {
        // GPUI requires an installed family name, not Fontconfig aliases.
        // Resolve the shipping CSS fallback before handing it to the renderer.
        static FAMILY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        FAMILY.get_or_init(|| {
            std::process::Command::new("fc-match")
                .args([
                    "--format",
                    "%{family[0]}",
                    "Segoe UI Variable Text,Segoe UI,Inter,Roboto,Helvetica Neue,Arial",
                ])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|family| family.trim().to_owned())
                .filter(|family| !family.is_empty())
                .unwrap_or_else(|| "Arial".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_theme_uses_shipping_and_valid_custom_colors() {
        assert_eq!(
            Theme::configured(true, "cobalt", "", "").accent,
            rgb(0x2563eb)
        );
        assert_eq!(
            Theme::configured(true, "custom", "#123456", "abcdef").accent,
            rgb(0x123456)
        );
        assert_eq!(
            Theme::configured(true, "custom", "invalid", "").accent,
            rgb(0x32d3ff)
        );
    }
}
