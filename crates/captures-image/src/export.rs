//! Shipping screenshot export policy, independent of host UI and filesystem I/O.

use std::borrow::Cow;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::{
    PNG_MAXIMUM_COLOR_STEPS, composite_onto_white, encode_jpeg, encode_png_export,
    encode_png_export_dithered, encode_webp, png_palette_colors_for_quality,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Png,
    Jpeg,
    Webp,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportQuality {
    #[default]
    Preserve,
    Compress,
    Maximum,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PngOptions {
    pub max_colors: Option<u16>,
}

/// Output-only dimensions. Percentage presets round width first, then derive
/// height from that width, matching the shipping editor's aspect-ratio policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExportSize {
    #[default]
    Original,
    Percent {
        percent: u8,
    },
    Custom {
        width: u32,
        height: u32,
    },
}

impl ExportSize {
    pub fn dimensions(self, width: u32, height: u32) -> Result<(u32, u32), String> {
        let dimensions = match self {
            Self::Original => return Ok((width, height)),
            Self::Percent { percent } => {
                if !(1..=100).contains(&percent) || width == 0 || height == 0 {
                    return Err(
                        "Output percentage must be from 1 through 100 on a nonempty image.".into(),
                    );
                }
                let scaled_width = ((u64::from(width) * u64::from(percent) + 50) / 100).max(1);
                let scaled_height = ((scaled_width * u64::from(height) + u64::from(width) / 2)
                    / u64::from(width))
                .max(1);
                (scaled_width, scaled_height)
            }
            Self::Custom { width, height } => (u64::from(width), u64::from(height)),
        };
        if dimensions.0 == 0 || dimensions.1 == 0 || dimensions.0 > 16_384 || dimensions.1 > 16_384
        {
            return Err("Output dimensions must be from 1 through 16,384 pixels.".into());
        }
        if dimensions.0 * dimensions.1 > 100_000_000 {
            return Err("Output size is limited to 100 million pixels.".into());
        }
        Ok((dimensions.0 as u32, dimensions.1 as u32))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub quality: ExportQuality,
    pub quality_value: u8,
    pub max_size_bytes: Option<u64>,
    pub png: PngOptions,
    #[serde(default)]
    pub size: ExportSize,
}

/// Resize transient export pixels, never the document. Lanczos3 operates on
/// premultiplied sRGB channels so invisible RGB cannot bleed into visible edges.
/// This is deterministic across native hosts, not browser-kernel pixel parity.
/// Original/equal dimensions borrow the exact pixels, including transparent RGB.
pub fn resize_for_export(
    image: &RgbaImage,
    size: ExportSize,
) -> Result<Cow<'_, RgbaImage>, String> {
    let (width, height) = size.dimensions(image.width(), image.height())?;
    if (width, height) == image.dimensions() {
        return Ok(Cow::Borrowed(image));
    }
    if image.width() == 0 || image.height() == 0 {
        return Err("Cannot resize an empty image.".into());
    }
    let premultiplied = image::Rgba32FImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y);
        let alpha = f32::from(pixel[3]) / 255.;
        image::Rgba([
            f32::from(pixel[0]) / 255. * alpha,
            f32::from(pixel[1]) / 255. * alpha,
            f32::from(pixel[2]) / 255. * alpha,
            alpha,
        ])
    });
    let resized = image::imageops::resize(
        &premultiplied,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    Ok(Cow::Owned(RgbaImage::from_fn(width, height, |x, y| {
        let pixel = resized.get_pixel(x, y);
        let alpha = pixel[3].clamp(0., 1.);
        let alpha_byte = (alpha * 255.).round() as u8;
        if alpha_byte == 0 {
            return image::Rgba([0, 0, 0, 0]);
        }
        image::Rgba([
            (pixel[0].clamp(0., alpha) / alpha * 255.).round() as u8,
            (pixel[1].clamp(0., alpha) / alpha * 255.).round() as u8,
            (pixel[2].clamp(0., alpha) / alpha * 255.).round() as u8,
            alpha_byte,
        ])
    })))
}

pub fn encode_export(image: &RgbaImage, options: ExportOptions) -> Result<Vec<u8>, String> {
    let resized = resize_for_export(image, options.size)?;
    let image = resized.as_ref();
    let Some(maximum) = options.max_size_bytes else {
        return encode_without_limit(image, options);
    };

    // Highest quality that still fits: start from an uncompressed/preserve
    // encode. A large cap must not keep a previous low-quality encode just
    // because it already fit.
    let preserve = encode_without_limit(
        image,
        ExportOptions {
            quality: ExportQuality::Preserve,
            quality_value: 100,
            max_size_bytes: None,
            png: PngOptions::default(),
            ..options
        },
    )?;
    if encoded_len(&preserve) <= maximum {
        return Ok(preserve);
    }

    let quality_ceiling = match options.quality {
        ExportQuality::Compress => options.quality_value,
        ExportQuality::Preserve | ExportQuality::Maximum => 100,
    };

    match options.format {
        ExportFormat::Jpeg => {
            let rgb = composite_onto_white(image);
            let maximum_quality = quality_ceiling.clamp(40, 100);
            let minimum = encode_jpeg(&rgb, 40)?;
            if encoded_len(&minimum) > maximum {
                return Err(
                    "JPEG cannot meet the requested maximum at the supported quality range; reduce the output size or raise the limit"
                        .to_owned(),
                );
            }

            let mut best = minimum;
            let mut low = 41_u8;
            let mut high = maximum_quality;
            while low <= high {
                let quality = low + (high - low) / 2;
                let candidate = encode_jpeg(&rgb, quality)?;
                if encoded_len(&candidate) <= maximum {
                    best = candidate;
                    low = quality.saturating_add(1);
                } else {
                    high = quality - 1;
                }
            }
            Ok(best)
        }
        ExportFormat::Png => {
            let mut best: Option<Vec<u8>> = None;
            'palettes: for colors in PNG_MAXIMUM_COLOR_STEPS {
                // Dither first (matches Compress), then posterize if diffusion
                // noise prevents an indexed image from meeting the hard cap.
                for dither in [true, false] {
                    let candidate = encode_png_export_dithered(image, true, Some(colors), dither)?;
                    let fits = encoded_len(&candidate) <= maximum;
                    if fits {
                        best = Some(candidate);
                        break 'palettes;
                    }
                    best = Some(candidate);
                }
            }
            let best = best.ok_or_else(|| {
                "PNG cannot meet the requested maximum; reduce the output size or raise the limit"
                    .to_owned()
            })?;
            if encoded_len(&best) <= maximum {
                Ok(best)
            } else {
                Err(
                    "the PNG is larger than the requested maximum even after reducing colors; reduce the output size, raise the limit, or switch to JPEG for more aggressive size control"
                        .to_owned(),
                )
            }
        }
        ExportFormat::Webp => {
            let minimum = encode_webp(image, Some(1))?;
            if encoded_len(&minimum) > maximum {
                return Err(
                    "WebP cannot meet the requested maximum at the supported quality range; reduce the output size or raise the limit"
                        .to_owned(),
                );
            }
            let mut best = minimum;
            let mut low = 2_u8;
            let mut high = quality_ceiling.clamp(1, 100);
            while low <= high {
                let quality = low + (high - low) / 2;
                let candidate = encode_webp(image, Some(quality))?;
                if encoded_len(&candidate) <= maximum {
                    best = candidate;
                    low = quality.saturating_add(1);
                } else {
                    high = quality - 1;
                }
            }
            Ok(best)
        }
    }
}

fn encode_without_limit(image: &RgbaImage, options: ExportOptions) -> Result<Vec<u8>, String> {
    match options.format {
        ExportFormat::Png => {
            let max_colors = match options.quality {
                ExportQuality::Preserve => None,
                ExportQuality::Compress => options
                    .png
                    .max_colors
                    .filter(|count| *count > 0)
                    .or_else(|| png_palette_colors_for_quality(options.quality_value)),
                ExportQuality::Maximum => Some(
                    options
                        .png
                        .max_colors
                        .filter(|count| *count > 0)
                        .unwrap_or(64),
                ),
            };
            encode_png_export(
                image,
                !matches!(options.quality, ExportQuality::Preserve),
                max_colors,
            )
        }
        ExportFormat::Jpeg => {
            let quality = if matches!(options.quality, ExportQuality::Preserve) {
                100
            } else {
                options.quality_value
            };
            encode_jpeg(&composite_onto_white(image), quality)
        }
        ExportFormat::Webp => {
            let quality = match options.quality {
                ExportQuality::Preserve => None,
                ExportQuality::Compress => Some(options.quality_value.clamp(1, 100)),
                ExportQuality::Maximum => Some(options.quality_value.clamp(1, 100).min(80)),
            };
            encode_webp(image, quality)
        }
    }
}

fn encoded_len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;

    #[test]
    fn output_dimensions_round_width_first_and_enforce_both_limits() {
        assert_eq!(
            ExportSize::Percent { percent: 50 }.dimensions(5, 8),
            Ok((3, 5))
        );
        assert_eq!(
            ExportSize::Percent { percent: 75 }.dimensions(73, 41),
            Ok((55, 31))
        );
        assert_eq!(
            ExportSize::Percent { percent: 50 }.dimensions(1, 9),
            Ok((1, 9))
        );
        for percent in [0, 101, 255] {
            assert!(ExportSize::Percent { percent }.dimensions(5, 8).is_err());
        }
        for (width, height, accepted) in [
            (0, 1, false),
            (1, 0, false),
            (16_384, 1, true),
            (16_385, 1, false),
            (1, 16_385, false),
            (10_000, 10_000, true),
            (10_000, 10_001, false),
            (u32::MAX, u32::MAX, false),
        ] {
            assert_eq!(
                ExportSize::Custom { width, height }
                    .dimensions(5, 8)
                    .is_ok(),
                accepted
            );
        }
    }

    #[test]
    fn resize_borrows_unchanged_pixels_and_filters_premultiplied_alpha() {
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 0, Rgba([0, 0, 255, 0]));
        for size in [
            ExportSize::Original,
            ExportSize::Custom {
                width: 2,
                height: 1,
            },
        ] {
            assert!(matches!(
                resize_for_export(&image, size).unwrap(),
                Cow::Borrowed(_)
            ));
        }
        let small = resize_for_export(
            &image,
            ExportSize::Custom {
                width: 1,
                height: 1,
            },
        )
        .unwrap();
        assert_eq!(
            small.get_pixel(0, 0).0,
            [255, 0, 0, 128],
            "hidden blue must not bleed into red"
        );
        assert_eq!(
            image.get_pixel(1, 0).0,
            [0, 0, 255, 0],
            "input is untouched"
        );
        let faint = RgbaImage::from_pixel(3, 2, Rgba([17, 91, 203, 1]));
        let large = resize_for_export(
            &faint,
            ExportSize::Custom {
                width: 7,
                height: 5,
            },
        )
        .unwrap();
        assert!(
            large.pixels().all(|pixel| pixel.0 == [17, 91, 203, 1]),
            "premultiplication must retain low-alpha precision"
        );
    }

    #[test]
    fn every_encoder_uses_requested_size_and_enforces_the_byte_budget() {
        let image = RgbaImage::from_pixel(5, 8, Rgba([19, 71, 193, 255]));
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp] {
            let options = ExportOptions {
                format,
                quality: ExportQuality::Preserve,
                quality_value: 100,
                max_size_bytes: None,
                png: PngOptions::default(),
                size: ExportSize::Percent { percent: 50 },
            };
            let bytes = encode_export(&image, options).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
            assert_eq!(decoded.dimensions(), (3, 5));
            if format != ExportFormat::Jpeg {
                assert!(decoded.pixels().all(|pixel| pixel.0 == [19, 71, 193, 255]));
            }
            assert!(
                encode_export(
                    &image,
                    ExportOptions {
                        max_size_bytes: Some(0),
                        ..options
                    }
                )
                .is_err()
            );
        }
    }

    fn fixture() -> RgbaImage {
        RgbaImage::from_fn(73, 41, |x, y| {
            Rgba([
                (x * 29 + y * 3) as u8,
                (x * 7 + y * 31) as u8,
                (x * y + 113) as u8,
                (x * 11 + y * 17) as u8,
            ])
        })
    }

    fn fingerprint(bytes: &[u8]) -> (usize, u64) {
        let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        (bytes.len(), hash)
    }

    #[test]
    fn outputs_match_shipping_encoder_fingerprints() {
        let image = fixture();
        let cases = [
            (
                ExportFormat::Png,
                ExportQuality::Preserve,
                100,
                None,
                (12_154, 0x38b1_a61a_97e0_d307),
            ),
            (
                ExportFormat::Png,
                ExportQuality::Compress,
                55,
                None,
                (1_977, 0xd67e_c8da_431a_2f20),
            ),
            (
                ExportFormat::Png,
                ExportQuality::Compress,
                92,
                Some(37),
                (2_059, 0xe260_7f36_508d_ebe5),
            ),
            (
                ExportFormat::Jpeg,
                ExportQuality::Preserve,
                40,
                None,
                (10_786, 0xe70d_ef09_c71f_5c99),
            ),
            (
                ExportFormat::Jpeg,
                ExportQuality::Compress,
                73,
                None,
                (3_413, 0xbd41_9905_762d_e4c5),
            ),
            (
                ExportFormat::Webp,
                ExportQuality::Preserve,
                1,
                None,
                (1_496, 0x6352_562b_03f2_f469),
            ),
            (
                ExportFormat::Webp,
                ExportQuality::Compress,
                67,
                None,
                (2_126, 0x01b8_e798_eef2_6a24),
            ),
            (
                ExportFormat::Webp,
                ExportQuality::Maximum,
                100,
                None,
                (2_426, 0x3f2e_122b_74f2_a922),
            ),
        ];
        for (format, quality, quality_value, max_colors, expected) in cases {
            let bytes = encode_export(
                &image,
                ExportOptions {
                    format,
                    quality,
                    quality_value,
                    max_size_bytes: None,
                    png: PngOptions { max_colors },
                    size: ExportSize::Original,
                },
            )
            .unwrap();
            assert_eq!(fingerprint(&bytes), expected, "{format:?} {quality:?}");
        }
    }

    #[test]
    fn hard_limits_choose_the_largest_supported_quality_that_fits() {
        let image = fixture();
        for (format, floor_quality) in [(ExportFormat::Jpeg, 40), (ExportFormat::Webp, 1)] {
            let floor = encode_export(
                &image,
                ExportOptions {
                    format,
                    quality: ExportQuality::Compress,
                    quality_value: floor_quality,
                    max_size_bytes: None,
                    png: PngOptions::default(),
                    size: ExportSize::Original,
                },
            )
            .unwrap();
            let preserve = encode_export(
                &image,
                ExportOptions {
                    format,
                    quality: ExportQuality::Preserve,
                    quality_value: 100,
                    max_size_bytes: None,
                    png: PngOptions::default(),
                    size: ExportSize::Original,
                },
            )
            .unwrap();
            let limit = u64::try_from((floor.len() + preserve.len()) / 2).unwrap();
            let limited = encode_export(
                &image,
                ExportOptions {
                    format,
                    quality: ExportQuality::Maximum,
                    quality_value: 100,
                    max_size_bytes: Some(limit),
                    png: PngOptions::default(),
                    size: ExportSize::Original,
                },
            )
            .unwrap();
            assert!(encoded_len(&limited) <= limit, "{format:?}");
            assert!(limited.len() > floor.len(), "{format:?}");
        }
    }
}
