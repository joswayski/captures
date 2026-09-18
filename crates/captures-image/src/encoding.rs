//! Shared screenshot encoders. Preserve shipping chroma, alpha and quality behavior.
use image::{Rgb, RgbImage, RgbaImage};

pub fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let width =
        u16::try_from(image.width()).map_err(|_| "JPEG width is too large to encode".to_owned())?;
    let height = u16::try_from(image.height())
        .map_err(|_| "JPEG height is too large to encode".to_owned())?;
    let mut bytes = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, quality.clamp(40, 100));
    // Preserve full-resolution chroma and matched luma/color quantization tables.
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    encoder.set_quantization_tables(
        jpeg_encoder::QuantizationTableType::ImageMagick,
        jpeg_encoder::QuantizationTableType::ImageMagick,
    );
    encoder
        .encode(image.as_raw(), width, height, jpeg_encoder::ColorType::Rgb)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

/// `None` quality is lossless; `Some(q)` is lossy at quality 1–100, preserving alpha.
pub fn encode_webp(image: &RgbaImage, quality: Option<u8>) -> Result<Vec<u8>, String> {
    if image.width() == 0 || image.height() == 0 {
        return Err("cannot encode an empty WebP image".to_owned());
    }
    let encoder = webp::Encoder::from_rgba(image.as_raw(), image.width(), image.height());
    let encoded = match quality {
        None => encoder
            .encode_simple(true, 100.0)
            .map_err(|error| format!("WebP lossless encode failed: {error:?}"))?,
        Some(q) => {
            let mut config = webp::WebPConfig::new()
                .map_err(|error| format!("WebP config failed: {error:?}"))?;
            config.lossless = 0;
            config.quality = f32::from(q.clamp(1, 100));
            config.use_sharp_yuv = 1;
            encoder
                .encode_advanced(&config)
                .map_err(|error| format!("WebP lossy encode failed: {error:?}"))?
        }
    };
    Ok(encoded.to_vec())
}

pub fn composite_onto_white(image: &RgbaImage) -> RgbImage {
    let mut output = RgbImage::new(image.width(), image.height());
    for (pixel, destination) in image.pixels().zip(output.pixels_mut()) {
        let alpha = u16::from(pixel[3]);
        let inverse = 255 - alpha;
        *destination = Rgb([
            ((u16::from(pixel[0]) * alpha + 255 * inverse) / 255) as u8,
            ((u16::from(pixel[1]) * alpha + 255 * inverse) / 255) as u8,
            ((u16::from(pixel[2]) * alpha + 255 * inverse) / 255) as u8,
        ]);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_alpha_flattens_on_white_not_black() {
        let pixels =
            RgbaImage::from_raw(3, 1, vec![12, 34, 56, 0, 12, 34, 56, 128, 12, 34, 56, 255])
                .unwrap();
        assert_eq!(
            composite_onto_white(&pixels).into_raw(),
            [255, 255, 255, 133, 144, 155, 12, 34, 56]
        );
    }

    #[test]
    fn lossless_webp_preserves_translucent_pixels() {
        let pixels = RgbaImage::from_fn(9, 5, |x, y| {
            image::Rgba([x as u8 * 21, y as u8 * 31, 87, 128])
        });
        let bytes = encode_webp(&pixels, None).unwrap();
        assert_eq!(
            image::guess_format(&bytes).unwrap(),
            image::ImageFormat::WebP
        );
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().into_rgba8(),
            pixels
        );
    }
}
