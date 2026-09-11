use image::{Rgb, RgbImage, RgbaImage};

use crate::protocol::{BridgeResult, ImageFormat};

pub fn encode(
    image: &RgbaImage,
    format: ImageFormat,
    quality: Option<u8>,
    max_bytes: Option<u64>,
) -> BridgeResult<Vec<u8>> {
    if image.width() == 0 || image.height() == 0 {
        return Err("cannot encode an empty image".to_owned());
    }
    if quality.is_some_and(|quality| !(1..=100).contains(&quality)) {
        return Err("image quality must be between 1 and 100".to_owned());
    }
    if max_bytes == Some(0) {
        return Err("max_bytes must be greater than zero".to_owned());
    }
    match (format, max_bytes) {
        (ImageFormat::Png, maximum) => {
            let bytes = encode_png(image)?;
            if maximum.is_some_and(|maximum| byte_len(&bytes) > maximum) {
                return Err(
                    "lossless PNG cannot meet max_bytes; use JPEG or WebP for lossy size control"
                        .to_owned(),
                );
            }
            Ok(bytes)
        }
        (ImageFormat::Jpeg, Some(maximum)) => search_quality(
            quality.unwrap_or(100),
            1,
            maximum,
            |quality| encode_jpeg(image, quality),
            "JPEG",
        ),
        (ImageFormat::Webp, Some(maximum)) => search_quality(
            quality.unwrap_or(100),
            1,
            maximum,
            |quality| encode_webp(image, quality),
            "WebP",
        ),
        (ImageFormat::Jpeg, None) => encode_jpeg(image, quality.unwrap_or(92)),
        (ImageFormat::Webp, None) => quality.map_or_else(
            || encode_webp_lossless(image),
            |quality| encode_webp(image, quality),
        ),
    }
}

fn encode_png(image: &RgbaImage) -> BridgeResult<Vec<u8>> {
    use image::{ExtendedColorType, ImageEncoder, codecs::png};
    let mut bytes = Vec::new();
    png::PngEncoder::new_with_quality(
        &mut bytes,
        png::CompressionType::Best,
        png::FilterType::Adaptive,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )
    .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn encode_jpeg(image: &RgbaImage, quality: u8) -> BridgeResult<Vec<u8>> {
    let rgb = composite_onto_white(image);
    let width = u16::try_from(rgb.width()).map_err(|_| "JPEG width is too large")?;
    let height = u16::try_from(rgb.height()).map_err(|_| "JPEG height is too large")?;
    let mut bytes = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, quality);
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    encoder.set_quantization_tables(
        jpeg_encoder::QuantizationTableType::ImageMagick,
        jpeg_encoder::QuantizationTableType::ImageMagick,
    );
    encoder
        .encode(rgb.as_raw(), width, height, jpeg_encoder::ColorType::Rgb)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn encode_webp(image: &RgbaImage, quality: u8) -> BridgeResult<Vec<u8>> {
    let encoder = webp::Encoder::from_rgba(image.as_raw(), image.width(), image.height());
    let mut config =
        webp::WebPConfig::new().map_err(|error| format!("WebP configuration failed: {error:?}"))?;
    config.lossless = 0;
    config.quality = f32::from(quality);
    config.use_sharp_yuv = 1;
    encoder
        .encode_advanced(&config)
        .map(|bytes| bytes.to_vec())
        .map_err(|error| format!("WebP encoding failed: {error:?}"))
}

fn encode_webp_lossless(image: &RgbaImage) -> BridgeResult<Vec<u8>> {
    webp::Encoder::from_rgba(image.as_raw(), image.width(), image.height())
        .encode_simple(true, 100.0)
        .map(|bytes| bytes.to_vec())
        .map_err(|error| format!("WebP lossless encoding failed: {error:?}"))
}

fn search_quality<F>(
    maximum_quality: u8,
    minimum_quality: u8,
    maximum_bytes: u64,
    mut encoder: F,
    label: &str,
) -> BridgeResult<Vec<u8>>
where
    F: FnMut(u8) -> BridgeResult<Vec<u8>>,
{
    for quality in (minimum_quality..=maximum_quality).rev() {
        let candidate = encoder(quality)?;
        if byte_len(&candidate) <= maximum_bytes {
            return Ok(candidate);
        }
    }
    Err(format!(
        "{label} cannot meet max_bytes at the minimum supported quality"
    ))
}

fn composite_onto_white(image: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y);
        let alpha = u16::from(pixel[3]);
        let inverse = 255 - alpha;
        Rgb([
            ((u16::from(pixel[0]) * alpha + 255 * inverse) / 255) as u8,
            ((u16::from(pixel[1]) * alpha + 255 * inverse) / 255) as u8,
            ((u16::from(pixel[2]) * alpha + 255 * inverse) / 255) as u8,
        ])
    })
}

fn byte_len(bytes: &[u8]) -> u64 {
    bytes.len().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use crate::protocol::ImageFormat;

    use super::{encode, encode_webp, search_quality};

    #[test]
    fn size_search_does_not_assume_encoded_sizes_are_monotonic() {
        let sizes = [0_usize, 10, 90, 70, 40, 80];
        let selected = search_quality(
            5,
            1,
            50,
            |quality| Ok(vec![0; sizes[usize::from(quality)]]),
            "fixture",
        )
        .unwrap();
        assert_eq!(selected.len(), 40);
    }

    #[test]
    fn webp_is_real_and_max_bytes_selects_a_fitting_lossy_quality() {
        let image = RgbaImage::from_fn(128, 96, |x, y| {
            Rgba([(x * 7) as u8, (y * 11) as u8, (x ^ y) as u8, 255])
        });
        let minimum = encode_webp(&image, 1).unwrap();
        let maximum = encode_webp(&image, 100).unwrap();
        let limit = (minimum.len() + maximum.len()) / 2;
        let limited = encode(&image, ImageFormat::Webp, Some(100), Some(limit as u64)).unwrap();
        assert_eq!(&limited[..4], b"RIFF");
        assert!(limited.len() <= limit);
        assert!(limited.len() >= minimum.len());
    }

    #[test]
    fn png_reports_an_unattainable_limit_instead_of_changing_dimensions() {
        let image = RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]));
        assert!(encode(&image, ImageFormat::Png, None, Some(1)).is_err());
    }
}
