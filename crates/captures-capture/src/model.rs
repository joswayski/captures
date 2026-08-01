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

impl DisplayDescriptor {
    /// Whether monitor geometry from the capture backend is reported in physical
    /// pixels (true on Windows) rather than the CSS/DIP space the overlay uses.
    #[must_use]
    pub const fn reports_physical_geometry() -> bool {
        cfg!(target_os = "windows")
    }

    /// Size of the fullscreen capture overlay / CSS coordinate space.
    ///
    /// On Windows, xcap reports physical pixels for `width`/`height` while the
    /// overlay window and pointer events are in logical DIPs (`physical / dpi`).
    /// Elsewhere, display dimensions already match that logical CSS space.
    #[must_use]
    pub fn overlay_size(&self) -> (f64, f64) {
        Self::overlay_size_for(self.width, self.height, self.scale_factor, Self::reports_physical_geometry())
    }

    /// Top-left of the fullscreen capture overlay in CSS/DIP coordinates.
    #[must_use]
    pub fn overlay_position(&self) -> (f64, f64) {
        if Self::reports_physical_geometry() {
            let scale = self.scale_factor.max(1.0);
            (f64::from(self.x) / scale, f64::from(self.y) / scale)
        } else {
            (f64::from(self.x), f64::from(self.y))
        }
    }

    /// `(x, y, width, height)` for sizing a Tauri overlay to this display.
    #[must_use]
    pub fn overlay_geometry(&self) -> (f64, f64, f64, f64) {
        let (x, y) = self.overlay_position();
        let (width, height) = self.overlay_size();
        (x, y, width, height)
    }

    /// Testable form of [`Self::overlay_size`].
    #[must_use]
    pub fn overlay_size_for(
        width: u32,
        height: u32,
        scale_factor: f64,
        physical_geometry: bool,
    ) -> (f64, f64) {
        if physical_geometry {
            let scale = scale_factor.max(1.0);
            (f64::from(width) / scale, f64::from(height) / scale)
        } else {
            (f64::from(width.max(1)), f64::from(height.max(1)))
        }
    }

    /// Scale that maps overlay/CSS logical coordinates onto a capture buffer.
    ///
    /// Prefer the ratio of actual buffer pixels to overlay size so Retina and
    /// Windows DPI crops stay aligned even if the platform scale is slightly off.
    #[must_use]
    pub fn overlay_to_buffer_scale(&self, buffer_width: u32, buffer_height: u32) -> f64 {
        Self::overlay_to_buffer_scale_for(
            self.width,
            self.height,
            self.scale_factor,
            buffer_width,
            buffer_height,
            Self::reports_physical_geometry(),
        )
    }

    /// Testable form of [`Self::overlay_to_buffer_scale`].
    #[must_use]
    pub fn overlay_to_buffer_scale_for(
        width: u32,
        height: u32,
        scale_factor: f64,
        buffer_width: u32,
        buffer_height: u32,
        physical_geometry: bool,
    ) -> f64 {
        let (logical_w, logical_h) =
            Self::overlay_size_for(width, height, scale_factor, physical_geometry);
        let scale_x = f64::from(buffer_width) / logical_w.max(1.0);
        let scale_y = f64::from(buffer_height) / logical_h.max(1.0);
        let derived = ((scale_x + scale_y) * 0.5).max(1.0);
        if physical_geometry {
            return derived;
        }
        // When display size is already logical (macOS/Linux), prefer the larger of
        // the platform scale and buffer ratio so Retina crops stay sharp.
        if (derived - scale_factor.max(1.0)).abs() > 0.25 {
            return derived;
        }
        scale_factor.max(1.0).max(derived)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WindowDescriptor {
    pub id: String,
    pub title: String,
    pub app_name: Option<String>,
    #[serde(default)]
    pub z_order: i32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub display_id: String,
    /// Measured visible corner radius in logical points, when known.
    ///
    /// macOS window chrome varies by app and OS generation. When this is set,
    /// the selector highlight/cutout and PNG corner mask follow it instead of
    /// the session-wide system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<f64>,
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

#[cfg(test)]
mod tests {
    use super::DisplayDescriptor;

    #[test]
    fn windows_overlay_size_converts_physical_display_to_dips() {
        let (width, height) =
            DisplayDescriptor::overlay_size_for(3840, 2160, 2.0, true);
        assert!((width - 1920.0).abs() < f64::EPSILON);
        assert!((height - 1080.0).abs() < f64::EPSILON);
    }

    #[test]
    fn windows_region_scale_maps_css_points_onto_physical_buffer() {
        // 150% DPI laptop: logical 1280×720 overlay, 1920×1080 capture buffer.
        let scale = DisplayDescriptor::overlay_to_buffer_scale_for(
            1920, 1080, 1.5, 1920, 1080, true,
        );
        assert!((scale - 1.5).abs() < 1e-9);
    }

    #[test]
    fn logical_geometry_prefers_buffer_ratio_on_retina() {
        // macOS-style: display size already in points; buffer is 2× pixels.
        let scale = DisplayDescriptor::overlay_to_buffer_scale_for(
            1512, 982, 2.0, 3024, 1964, false,
        );
        assert!((scale - 2.0).abs() < 1e-9);
    }
}
