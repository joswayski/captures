#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub fn from_hex(value: &str) -> Option<Self> {
        let value = value.strip_prefix('#')?;
        if value.len() != 6 {
            return None;
        }
        Some(Self(
            u8::from_str_radix(&value[0..2], 16).ok()?,
            u8::from_str_radix(&value[2..4], 16).ok()?,
            u8::from_str_radix(&value[4..6], 16).ok()?,
            255,
        ))
    }

    pub fn rgba(self) -> [f32; 4] {
        [
            f32::from(self.0) / 255.0,
            f32::from(self.1) / 255.0,
            f32::from(self.2) / 255.0,
            f32::from(self.3) / 255.0,
        ]
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub canvas: Color,
    pub raised: Color,
    pub field: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Color,
    pub signal: Color,
    pub positive: Color,
    pub glass: Color,
}

pub fn palette(light: bool, accent: Color, signal: Color) -> Palette {
    if light {
        Palette {
            canvas: Color(245, 245, 247, 255),
            raised: Color(255, 255, 255, 255),
            field: Color(255, 255, 255, 255),
            text: Color(19, 19, 24, 255),
            muted: Color(92, 92, 105, 255),
            border: Color(19, 19, 24, 28),
            accent,
            signal,
            positive: Color(53, 163, 93, 255),
            glass: Color(15, 15, 18, 237),
        }
    } else {
        Palette {
            canvas: Color(16, 16, 20, 255),
            raised: Color(22, 22, 27, 255),
            field: Color(14, 14, 18, 255),
            text: Color(242, 242, 244, 255),
            muted: Color(185, 185, 196, 255),
            border: Color(255, 255, 255, 26),
            accent,
            signal,
            positive: Color(53, 163, 93, 255),
            glass: Color(15, 15, 18, 237),
        }
    }
}

pub fn theme_colors(name: &str, custom_accent: &str, custom_signal: &str) -> (Color, Color) {
    match name {
        "ember" => (Color(255, 122, 69, 255), Color(255, 61, 113, 255)),
        "rose" => (Color(255, 91, 167, 255), Color(255, 107, 69, 255)),
        "violet" => (Color(192, 38, 211, 255), Color(255, 79, 136, 255)),
        "cobalt" => (Color(37, 99, 235, 255), Color(255, 90, 100, 255)),
        "aqua" => (Color(49, 203, 216, 255), Color(255, 81, 118, 255)),
        "mint" => (Color(103, 213, 165, 255), Color(241, 90, 72, 255)),
        "mono" => (Color(242, 242, 244, 255), Color(239, 70, 80, 255)),
        "custom" => (
            Color::from_hex(custom_accent).unwrap_or(Color(50, 211, 255, 255)),
            Color::from_hex(custom_signal).unwrap_or(Color(255, 79, 195, 255)),
        ),
        _ => (Color(255, 202, 40, 255), Color(239, 70, 80, 255)),
    }
}
