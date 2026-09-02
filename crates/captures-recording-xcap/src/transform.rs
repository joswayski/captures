use captures_recording::{CaptureRect, MaxResolution};
use image::{RgbaImage, imageops::FilterType};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl FrameRect {
    pub fn from_logical(
        rect: CaptureRect,
        logical_width: u32,
        logical_height: u32,
        frame_width: u32,
        frame_height: u32,
    ) -> Option<Self> {
        if !rect.is_valid() || logical_width == 0 || logical_height == 0 {
            return None;
        }
        let scale_x = f64::from(frame_width) / f64::from(logical_width);
        let scale_y = f64::from(frame_height) / f64::from(logical_height);
        let x = (f64::from(rect.x.max(0)) * scale_x).round() as u32;
        let y = (f64::from(rect.y.max(0)) * scale_y).round() as u32;
        let width = (f64::from(rect.width) * scale_x).round().max(1.0) as u32;
        let height = (f64::from(rect.height) * scale_y).round().max(1.0) as u32;
        let x = x.min(frame_width.saturating_sub(1));
        let y = y.min(frame_height.saturating_sub(1));
        Some(Self {
            x,
            y,
            width: width.min(frame_width.saturating_sub(x)).max(1),
            height: height.min(frame_height.saturating_sub(y)).max(1),
        })
    }
}

pub struct FrameTransform {
    source: FrameRect,
    output_width: u32,
    output_height: u32,
}

impl FrameTransform {
    pub fn new(
        source: FrameRect,
        maximum: MaxResolution,
        frame_width: u32,
        frame_height: u32,
    ) -> Option<Self> {
        let right = source.x.checked_add(source.width)?;
        let bottom = source.y.checked_add(source.height)?;
        if source.width == 0 || source.height == 0 || right > frame_width || bottom > frame_height {
            return None;
        }
        let (mut output_width, mut output_height) = maximum.constrain(source.width, source.height);
        let (maximum_width, maximum_height) = if output_width >= output_height {
            (3_840.0, 2_160.0)
        } else {
            (2_160.0, 3_840.0)
        };
        let encoder_scale = (maximum_width / f64::from(output_width))
            .min(maximum_height / f64::from(output_height))
            .min(1.0);
        output_width = (f64::from(output_width) * encoder_scale).floor() as u32 & !1;
        output_height = (f64::from(output_height) * encoder_scale).floor() as u32 & !1;
        Some(Self {
            source,
            output_width: output_width.max(2),
            output_height: output_height.max(2),
        })
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.output_width, self.output_height)
    }

    #[cfg(test)]
    pub fn rgb(&self, frame: &RgbaImage) -> Vec<u8> {
        let mut rgb = Vec::new();
        self.rgb_into(frame, &mut rgb);
        rgb
    }

    pub fn rgb_into(&self, frame: &RgbaImage, rgb: &mut Vec<u8>) {
        let needed = usize::try_from(self.output_width)
            .unwrap_or_default()
            .saturating_mul(usize::try_from(self.output_height).unwrap_or_default())
            .saturating_mul(3);
        rgb.clear();
        rgb.reserve(needed);
        if self.source.width == self.output_width && self.source.height == self.output_height {
            let stride = frame.width() as usize * 4;
            let row_start = self.source.x as usize * 4;
            let row_len = self.source.width as usize * 4;
            let raw = frame.as_raw();
            for row in self.source.y..self.source.y + self.source.height {
                let start = row as usize * stride + row_start;
                if let Some(pixels) = raw.get(start..start + row_len) {
                    copy_rgb(pixels, rgb);
                }
            }
            return;
        }
        let view = image::imageops::crop_imm(
            frame,
            self.source.x,
            self.source.y,
            self.source.width,
            self.source.height,
        );
        let output = image::imageops::resize(
            &*view,
            self.output_width,
            self.output_height,
            FilterType::Triangle,
        );
        copy_rgb(output.as_raw(), rgb);
    }
}

/// Append the RGB channels of tightly packed RGBA pixels.
fn copy_rgb(rgba: &[u8], rgb: &mut Vec<u8>) {
    for pixel in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
}

#[cfg(test)]
mod tests {
    use captures_recording::{CaptureRect, MaxResolution};
    use image::{Rgba, RgbaImage};

    use super::{FrameRect, FrameTransform};

    #[test]
    fn maps_logical_regions_into_hidpi_frames() {
        let rect = FrameRect::from_logical(
            CaptureRect {
                x: 100,
                y: 50,
                width: 800,
                height: 450,
            },
            1_920,
            1_080,
            3_840,
            2_160,
        )
        .expect("mapped region");
        assert_eq!(
            rect,
            FrameRect {
                x: 200,
                y: 100,
                width: 1_600,
                height: 900,
            }
        );
    }

    #[test]
    fn constrains_and_converts_selected_frames() {
        let frame = RgbaImage::from_pixel(1_920, 1_080, Rgba([20, 40, 60, 255]));
        let transform = FrameTransform::new(
            FrameRect {
                x: 0,
                y: 0,
                width: 1_920,
                height: 1_080,
            },
            MaxResolution::P720,
            frame.width(),
            frame.height(),
        )
        .expect("transform");
        assert_eq!(transform.dimensions(), (1_280, 720));
        let rgb = transform.rgb(&frame);
        assert_eq!(rgb.len(), 1_280 * 720 * 3);
        assert_eq!(&rgb[..3], &[20, 40, 60]);
        let mut reused = vec![1, 2, 3];
        transform.rgb_into(&frame, &mut reused);
        assert_eq!(reused, rgb);
    }
}
