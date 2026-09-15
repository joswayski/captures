//! Captures' semantic colors, not a GPUI component library's theme.
use gpui::{App, Global, Rgba, Window, WindowAppearance, rgb, rgba};

pub struct CurrentSettings(pub crate::preferences::settings::Settings);
impl Global for CurrentSettings {}

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

    /// Floating desktop surfaces keep the dark media palette, but respect the
    /// configured accent and signal independently of regular-window appearance.
    pub fn for_media(cx: &App) -> Self {
        cx.try_global::<CurrentSettings>().map_or_else(
            || Self::new(false),
            |settings| {
                Self::configured(
                    false,
                    &settings.0.theme,
                    &settings.0.custom_theme.accent,
                    &settings.0.custom_theme.signal,
                )
            },
        )
    }

    pub fn for_window(launch: &crate::Launch, window: &Window, cx: &App) -> Self {
        if let Some(settings) = cx.try_global::<CurrentSettings>() {
            Self::from_settings(&settings.0, launch, window)
        } else {
            Self::new(launch.light)
        }
    }

    pub fn from_settings(
        settings: &crate::preferences::settings::Settings,
        launch: &crate::Launch,
        window: &Window,
    ) -> Self {
        let light = match settings.appearance.as_str() {
            "light" => true,
            "dark" => false,
            _ if launch.mock => launch.light,
            _ => matches!(
                window.appearance(),
                WindowAppearance::Light | WindowAppearance::VibrantLight
            ),
        };
        Self::configured(
            light,
            &settings.theme,
            &settings.custom_theme.accent,
            &settings.custom_theme.signal,
        )
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

/// Read on the foreground thread so live OS animation preferences are honored.
/// The environment override is useful for repeatable visual fixtures.
pub fn reduced_motion() -> bool {
    if let Some(value) = std::env::var_os("CAPTURES_REDUCED_MOTION") {
        return !matches!(value.to_str(), Some("0" | "false"));
    }
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::GtkSettingsExt;
        gtk::is_initialized_main_thread()
            && gtk::Settings::default().is_some_and(|settings| !settings.is_gtk_enable_animations())
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
        };
        let mut enabled = 1_u32;
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut u32).cast(),
                0,
            ) != 0
                && enabled == 0
        }
    }
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }
}

/// The shared --ease-out token: cubic-bezier(0.16, 1, 0.3, 1).
pub fn ease_out(progress: f32) -> f32 {
    if progress <= 0. {
        return 0.;
    }
    if progress >= 1. {
        return 1.;
    }
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..22 {
        let t = (lo + hi) * 0.5;
        let x = 3. * (1. - t) * (1. - t) * t * 0.16 + 3. * (1. - t) * t * t * 0.3 + t * t * t;
        if x < progress {
            lo = t;
        } else {
            hi = t;
        }
    }
    1. - (1. - (lo + hi) * 0.5).powi(3)
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
    fn shared_ease_out_matches_the_parametric_midpoint() {
        assert_eq!(ease_out(-1.), 0.);
        assert_eq!(ease_out(1.5), 1.);
        // At Bezier parameter t=1/2, x=.2975 and y=.875.
        assert!((ease_out(0.2975) - 0.875).abs() < 0.00001);
    }

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
