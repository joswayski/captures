use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::geometry::PhysicalRect;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    Region,
    Window,
    Display,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DisplayDescriptor {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WindowDescriptor {
    pub id: String,
    pub title: String,
    pub app_name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub display_id: String,
}

#[derive(Debug)]
pub struct DisplayFrame {
    pub descriptor: DisplayDescriptor,
    pub image: RgbaImage,
}

impl DisplayFrame {
    pub fn crop(&self, rect: PhysicalRect) -> Option<RgbaImage> {
        if rect.width == 0 || rect.height == 0 {
            return None;
        }

        let right = rect.x.checked_add(rect.width)?;
        let bottom = rect.y.checked_add(rect.height)?;
        if right > self.image.width() || bottom > self.image.height() {
            return None;
        }

        Some(
            image::imageops::crop_imm(&self.image, rect.x, rect.y, rect.width, rect.height)
                .to_image(),
        )
    }
}
