//! Shipping screenshot export policy, independent of host UI and filesystem I/O.

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub quality: ExportQuality,
    pub quality_value: u8,
    pub max_size_bytes: Option<u64>,
    pub png: PngOptions,
}

pub fn encode_export(image: &RgbaImage, options: ExportOptions) -> Result<Vec<u8>, String> {
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
                },
            )
            .unwrap();
            assert!(encoded_len(&limited) <= limit, "{format:?}");
            assert!(limited.len() > floor.len(), "{format:?}");
        }
    }
}
