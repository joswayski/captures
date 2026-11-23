use captures_recording::{CaptureRect, MaxResolution};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{CroppedImage, Image, ImageRef},
};
use image::RgbaImage;

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
    resizer: Resizer,
    scaled: Vec<u8>,
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
            resizer: Resizer::new(),
            scaled: Vec::new(),
        })
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.output_width, self.output_height)
    }

    #[cfg(test)]
    pub fn rgb(&mut self, frame: &RgbaImage) -> Vec<u8> {
        let mut rgb = Vec::new();
        self.rgb_into(frame, &mut rgb);
        rgb
    }

    pub fn rgb_into(&mut self, frame: &RgbaImage, rgb: &mut Vec<u8>) {
        let width = self.output_width as usize;
        // Every byte is overwritten below, so a reused buffer is not
        // cleared (zero-filling 25 MB per 4K frame cost about 4 ms).
        rgb.resize(width * self.output_height as usize * 3, 0);
        if self.source.width == self.output_width && self.source.height == self.output_height {
            let stride = frame.width() as usize * 4;
            let start = self.source.y as usize * stride + self.source.x as usize * 4;
            if let Some(raw) = frame.as_raw().get(start..) {
                copy_rgb(raw, stride, width, rgb);
            }
            return;
        }
        // Runs for every captured frame. image's generic resize took about
        // 200 ms per 4K frame, which capped downscaled recordings near 5 FPS.
        let len = self.output_width as usize * self.output_height as usize * 4;
        self.scaled.resize(len, 0);
        let Ok(frame) = ImageRef::new(
            frame.width(),
            frame.height(),
            frame.as_raw(),
            PixelType::U8x4,
        ) else {
            return;
        };
        // A cropped view clamps the filter at the crop edge; a crop option
        // would blend in pixels from outside the recorded area.
        let (Ok(source), Ok(mut output)) = (
            CroppedImage::new(
                &frame,
                self.source.x,
                self.source.y,
                self.source.width,
                self.source.height,
            ),
            Image::from_slice_u8(
                self.output_width,
                self.output_height,
                &mut self.scaled,
                PixelType::U8x4,
            ),
        ) else {
            return;
        };
        let options = ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
            .use_alpha(false);
        if self.resizer.resize(&source, &mut output, &options).is_ok() {
            copy_rgb(&self.scaled, width * 4, width, rgb);
        }
    }
}

/// The RGB channels of `rgb.len() / (width * 3)` rows of RGBA pixels that
/// start `stride` bytes apart, split into row bands across up to four
/// threads: at 4K one thread took about 11 ms per frame, a third of the
/// encoder's time.
fn copy_rgb(rgba: &[u8], stride: usize, width: usize, rgb: &mut [u8]) {
    let row_len = width * 3;
    if row_len == 0 {
        return;
    }
    let rows = rgb.len() / row_len;
    let threads = std::thread::available_parallelism().map_or(1, |threads| threads.get().min(4));
    let per_band = rows.div_ceil(threads.min(rows / 64).max(1)).max(1);
    let copy = |band: usize, rgb: &mut [u8]| {
        for (row, output) in rgb.chunks_exact_mut(row_len).enumerate() {
            let start = (band * per_band + row) * stride;
            let Some(pixels) = rgba.get(start..start + width * 4) else {
                return;
            };
            for (output, pixel) in output.chunks_exact_mut(3).zip(pixels.chunks_exact(4)) {
                output.copy_from_slice(&pixel[..3]);
            }
        }
    };
    std::thread::scope(|scope| {
        let mut bands = rgb.chunks_mut(per_band * row_len).enumerate();
        let first = bands.next();
        for (band, rgb) in bands {
            scope.spawn(move || copy(band, rgb));
        }
        if let Some((band, rgb)) = first {
            copy(band, rgb);
        }
    });
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
    fn copies_every_cropped_row_into_a_reused_buffer() {
        // Tall enough for several row bands; each pixel encodes its position.
        let pixel = |x: u32, y: u32, salt: u8| Rgba([x as u8, y as u8, salt ^ (x + y) as u8, 9]);
        let mut transform = FrameTransform::new(
            FrameRect {
                x: 7,
                y: 5,
                width: 150,
                height: 270,
            },
            MaxResolution::Original,
            300,
            280,
        )
        .expect("crop");
        assert_eq!(transform.dimensions(), (150, 270));
        let mut rgb = Vec::new();
        for salt in [1, 2] {
            let frame = RgbaImage::from_fn(300, 280, |x, y| pixel(x, y, salt));
            transform.rgb_into(&frame, &mut rgb);
            assert_eq!(rgb.len(), 150 * 270 * 3);
            for (index, output) in rgb.chunks_exact(3).enumerate() {
                let (x, y) = (index as u32 % 150 + 7, index as u32 / 150 + 5);
                assert_eq!(output, &pixel(x, y, salt).0[..3], "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn constrains_and_converts_selected_frames() {
        let frame = RgbaImage::from_pixel(1_920, 1_080, Rgba([20, 40, 60, 255]));
        let mut transform = FrameTransform::new(
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
        let split = RgbaImage::from_fn(400, 200, |x, _| {
            Rgba(if x < 200 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            })
        });
        let mut cropped = FrameTransform::new(
            FrameRect {
                x: 200,
                y: 0,
                width: 200,
                height: 200,
            },
            MaxResolution::P720,
            split.width(),
            split.height(),
        )
        .expect("crop");
        cropped.output_width = 100;
        cropped.output_height = 100;
        // Only the selected (blue) half may reach the output, even at its edge.
        assert!(
            cropped
                .rgb(&split)
                .chunks_exact(3)
                .all(|pixel| pixel == [0, 0, 255])
        );
        let mut reused = vec![1, 2, 3];
        transform.rgb_into(&frame, &mut reused);
        assert_eq!(reused, rgb);
    }
}
