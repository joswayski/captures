//! Light/dark resolution, accent theme and system preferences that shape the
//! drawing: Windows' app theme, animation and wheel settings.
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::tokens::Tokens;

/// `light` or `dark`: the `--appearance` override, then the saved setting,
/// then Windows' app mode.
pub fn resolve(appearance: Option<&str>, settings: &captures_settings::AppSettings) -> String {
    if let Some(value) = appearance {
        return value.into();
    }
    match settings.appearance {
        captures_settings::Appearance::Light => "light".into(),
        captures_settings::Appearance::Dark => "dark".into(),
        _ if system_dark() => "dark".into(),
        _ => "light".into(),
    }
}

pub fn tokens(appearance: Option<&str>, settings: &captures_settings::AppSettings) -> Tokens {
    let theme = serde_json::to_value(settings.theme)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "mustard".into());
    let tokens = Tokens::variant(&resolve(appearance, settings), &theme);
    if settings.theme == captures_settings::ColorTheme::Custom {
        let custom = &settings.custom_theme;
        tokens.with_custom(&custom.accent, &custom.signal)
    } else {
        tokens
    }
}

fn system_dark() -> bool {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && value == 0
}

/// Windows' "Animation effects" switch, as `prefers-reduced-motion`.
pub fn reduced_motion() -> bool {
    let mut enabled = BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && !enabled.as_bool()
}

pub fn wheel_lines() -> f32 {
    let mut lines = 3u32;
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(&mut lines as *mut u32 as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    // WHEEL_PAGESCROLL (u32::MAX) scrolls a page; treat it as a long scroll.
    lines.min(20) as f32
}

/// A dark caption over dark surfaces.
pub fn title_bar(hwnd: HWND, tokens: &Tokens) {
    let canvas = tokens.color("surface-canvas");
    let dark = BOOL::from(canvas.r * 0.2126 + canvas.g * 0.7152 + canvas.b * 0.0722 < 0.5);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const BOOL as *const _,
            size_of::<BOOL>() as u32,
        );
    }
}
