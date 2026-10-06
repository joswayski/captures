//! Native editor image decoding shared by in-document import and external open.
//!
//! Shipping decodes imported layers in the webview, which color-manages every
//! still it accepts: ICC profiles (including CMYK JPEGs), PNG cICP, and PNG
//! gAMA/cHRM without an sRGB/iCCP chunk. Native decoding follows the same
//! precedence and converts everything to 8-bit straight-alpha sRGB, the editor
//! canvas format shipping also flattens into.

use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
    sync::{Arc, OnceLock},
};

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use moxcms::{
    Chromaticity, CicpColorPrimaries, CicpProfile, ColorPrimaries, ColorProfile, DataColorSpace,
    Layout, MatrixCoefficients, ToneReprCurve, TransferCharacteristics, TransformOptions, XyY,
};

use crate::editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS};

/// Shipping's `open_media` rejection copy for paths it does not classify.
pub const UNSUPPORTED_OPEN_MESSAGE: &str =
    "Captures can open PNG, JPEG, WebP, GIF, MP4, and WebM files.";

/// SDR reference white for HDR signals (ITU-R BT.2408), in nits.
const HDR_REFERENCE_WHITE: f32 = 203.;

pub fn decode_import(path: &Path) -> Result<RgbaImage, String> {
    decode(path, true)
}

pub fn decode_opened_image(path: &Path) -> Result<RgbaImage, String> {
    decode(path, false)
}

/// How the decoded samples are described, after PNG chunk precedence.
enum Source {
    Srgb,
    Profile(ColorProfile),
    /// SMPTE ST 2084 (PQ) or HLG with the given primaries.
    Hdr {
        primaries: ColorProfile,
        hlg: bool,
    },
}

fn decode(path: &Path, import: bool) -> Result<RgbaImage, String> {
    let maximum = captures_history::editor_draft::MAX_IMAGE_BYTES as u64;
    let file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > maximum {
        return Err("This image is too large to open in Captures.".into());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > maximum {
        return Err("This image is too large to open in Captures.".into());
    }
    // Shipping's webview reports every undecodable import with the file name;
    // its open_media path rejects anything but PNG/JPEG/WebP stills (no TIFF).
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let not_loaded = || format!("{name} could not be loaded.");
    let failed = |error: image::ImageError| {
        if import {
            not_loaded()
        } else {
            error.to_string()
        }
    };
    let format = match image::guess_format(&bytes) {
        Ok(format) => Some(format),
        Err(_) if import => return decode_svg(&bytes, &not_loaded()),
        Err(_) => None,
    }
    .filter(|format| {
        matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
        ) || (import
            && matches!(
                format,
                ImageFormat::Tiff | ImageFormat::Gif | ImageFormat::Bmp
            ))
    })
    .ok_or_else(|| {
        if import {
            not_loaded()
        } else {
            UNSUPPORTED_OPEN_MESSAGE.to_owned()
        }
    })?;
    let mut decoder = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_decoder()
        .map_err(failed)?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || width > MAX_RENDER_DIMENSION
        || height > MAX_RENDER_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_RENDER_PIXELS
    {
        return Err(format!(
            "Images are limited to {MAX_RENDER_DIMENSION} pixels per side and {MAX_RENDER_PIXELS} total pixels."
        ));
    }
    // Browser-decoded imports in Tauri respect EXIF orientation. Normalize it
    // before choosing natural dimensions or owning the pixels in a draft asset.
    let orientation = decoder.orientation().map_err(failed)?;
    let icc = if format == ImageFormat::Tiff {
        // image's TIFF adapter suppresses tag errors. Distinguish an absent ICC
        // profile from a malformed one rather than silently importing wrong colors.
        tiff::decoder::Decoder::new(Cursor::new(&bytes))
            .and_then(|mut decoder| decoder.image_ifd().find_tag(tiff::tags::Tag::IccProfile))
            .and_then(|tag| tag.map(|value| value.into_u8_vec()).transpose())
            .map_err(|error| color_error(error.to_string()))?
    } else if format == ImageFormat::Bmp {
        // image's BMP adapter ignores V4/V5 color metadata. Do not silently
        // relabel profiled/calibrated samples as sRGB.
        bmp_icc(&bytes)?
    } else {
        decoder
            .icc_profile()
            .map_err(|error| color_error(error.to_string()))?
    };
    let icc = icc
        .map(|profile| ColorProfile::new_from_slice(&profile))
        .transpose()
        .map_err(|error| color_error(error.to_string()))?;
    let source = if format == ImageFormat::Png {
        let metadata = png::Decoder::new(Cursor::new(&bytes))
            .read_info()
            .map_err(|error| color_error(error.to_string()))?;
        png_source(metadata.info(), icc)
    } else {
        icc.map_or(Source::Srgb, Source::Profile)
    };

    let pixels = match source {
        Source::Srgb => DynamicImage::from_decoder(decoder)
            .map_err(failed)?
            .into_rgba8(), // Untagged files use the sRGB assumption.
        Source::Hdr { primaries, hlg } => {
            let image = DynamicImage::from_decoder(decoder).map_err(failed)?;
            hdr_to_srgb(&image, &primaries, hlg)
        }
        Source::Profile(profile) if profile.color_space == DataColorSpace::Cmyk => {
            // image flattens CMYK to RGB naively, losing the samples the
            // profile describes, so read the original inks directly.
            drop(decoder);
            let inks = match format {
                ImageFormat::Jpeg => jpeg_inks(&bytes),
                ImageFormat::Tiff => tiff_inks(&bytes),
                _ => Err(UNSUPPORTED_COLOR_SPACE.into()),
            }?;
            if inks.len() as u64 != u64::from(width) * u64::from(height) * 4 {
                return Err(not_loaded());
            }
            let mut rgb = vec![0; inks.len() / 4 * 3];
            transform(&profile, Layout::Rgba, &inks, Layout::Rgb, &mut rgb)?;
            RgbaImage::from_fn(width, height, |x, y| {
                let offset = (y as usize * width as usize + x as usize) * 3;
                image::Rgba([rgb[offset], rgb[offset + 1], rgb[offset + 2], 255])
            })
        }
        Source::Profile(profile) => {
            let image = DynamicImage::from_decoder(decoder).map_err(failed)?;
            let (layout, samples) = match profile.color_space {
                DataColorSpace::Rgb => (Layout::Rgba, image.into_rgba8().into_raw()),
                DataColorSpace::Gray if !image.color().has_color() => {
                    (Layout::GrayAlpha, image.into_luma_alpha8().into_raw())
                }
                _ => return Err(UNSUPPORTED_COLOR_SPACE.into()),
            };
            let mut pixels = RgbaImage::new(width, height);
            transform(&profile, layout, &samples, Layout::Rgba, pixels.as_mut())?;
            pixels
        }
    };
    let mut image = DynamicImage::ImageRgba8(pixels);
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

const UNSUPPORTED_COLOR_SPACE: &str =
    "This image's color space is not supported yet. Convert it to sRGB before importing.";

fn color_error(error: String) -> String {
    format!("Cannot convert this image's color profile to sRGB: {error}")
}

/// Self-contained vector/text SVGs. Reject resource-bearing documents before
/// usvg decodes data URLs or silently drops unsupported content. No system
/// fonts, external resources, SVGZ expansion or embedded raster decoding.
fn decode_svg(bytes: &[u8], not_loaded: &str) -> Result<RgbaImage, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| not_loaded.to_owned())?;
    if !text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('<')
    {
        return Err(not_loaded.to_owned());
    }
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("SVG imports are limited to 4 MiB. Convert this image to PNG first.".into());
    }
    let xml = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 32_768,
            entity_resolver: None,
        },
    )
    .map_err(|_| not_loaded.to_owned())?;
    if !xml
        .root_element()
        .has_tag_name(("http://www.w3.org/2000/svg", "svg"))
    {
        return Err(not_loaded.to_owned());
    }
    // Browser image sizing for relative/omitted dimensions differs from usvg's
    // standalone defaults. Do not silently import a different natural size.
    for attribute in ["width", "height"] {
        let absolute = xml
            .root_element()
            .attribute(attribute)
            .and_then(|value| value.parse::<svgtypes::Length>().ok())
            .is_some_and(|length| {
                length.number > 0.
                    && matches!(
                        length.unit,
                        svgtypes::LengthUnit::None
                            | svgtypes::LengthUnit::Px
                            | svgtypes::LengthUnit::In
                            | svgtypes::LengthUnit::Cm
                            | svgtypes::LengthUnit::Mm
                            | svgtypes::LengthUnit::Pt
                            | svgtypes::LengthUnit::Pc
                    )
            });
        if !absolute {
            return Err("SVG imports need explicit absolute width and height. Set their dimensions or convert to PNG first.".into());
        }
    }
    for node in xml.descendants().filter(roxmltree::Node::is_element) {
        if node.ancestors().take(34).count() > 33 {
            return Err("This SVG is too deeply nested. Convert it to PNG first.".into());
        }
        if matches!(
            node.tag_name().name(),
            "image" | "feImage" | "foreignObject" | "use"
        ) {
            return Err("This SVG contains images, HTML content or reusable references. Convert it to PNG first.".into());
        }
    }
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    let mut options = resvg::usvg::Options {
        font_family: "Liberation Sans".into(),
        fontdb: FONTS
            .get_or_init(|| {
                let mut database = resvg::usvg::fontdb::Database::new();
                for bytes in crate::editor_fonts::bundled().files.values() {
                    database.load_font_data(bytes.to_vec());
                }
                database.set_sans_serif_family("Liberation Sans");
                database.set_serif_family("Liberation Serif");
                database.set_monospace_family("Liberation Mono");
                Arc::new(database)
            })
            .clone(),
        ..Default::default()
    };
    // Neither resolver may inherit usvg's filesystem-reading defaults.
    options.image_href_resolver = resvg::usvg::ImageHrefResolver {
        resolve_data: Box::new(|_, _, _| None),
        resolve_string: Box::new(|_, _| None),
    };
    let tree =
        resvg::usvg::Tree::from_xmltree(&xml, &options).map_err(|_| not_loaded.to_owned())?;
    let size = tree.size();
    if size.width() > MAX_RENDER_DIMENSION as f32 || size.height() > MAX_RENDER_DIMENSION as f32 {
        return Err(format!(
            "Images are limited to {MAX_RENDER_DIMENSION} pixels per side and {MAX_RENDER_PIXELS} total pixels."
        ));
    }
    let integer = size.to_int_size();
    // Vector compositing/filter intermediates are in addition to final pixels.
    if u64::from(integer.width()) * u64::from(integer.height()) > 4_194_304 {
        return Err("SVG imports are limited to 4,194,304 pixels. Convert it to PNG first.".into());
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(integer.width(), integer.height())
        .ok_or_else(|| not_loaded.to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    RgbaImage::from_raw(
        integer.width(),
        integer.height(),
        pixmap.take_demultiplied(),
    )
    .ok_or_else(|| not_loaded.to_owned())
}

/// BITMAPV4/V5 color descriptions, which image's decoder does not expose.
/// Linked profiles never cause filesystem/network reads. Calibrated endpoints
/// require conversion first; ordinary older DIBs have no color description.
fn bmp_icc(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let invalid = || color_error("Invalid BMP color description.".into());
    let header = bytes.get(14..).ok_or_else(invalid)?;
    let word = |offset| {
        header
            .get(offset..offset + 4)
            .map(|word| u32::from_le_bytes(word.try_into().expect("four bytes")))
            .ok_or_else(invalid)
    };
    let size = word(0)?;
    if size < 108 {
        return Ok(None);
    }
    match word(56)? {
        0x7352_4742 | 0x5769_6e20 => Ok(None), // LCS_sRGB / WINDOWS_COLOR_SPACE
        0x4d42_4544 if size == 124 => {
            // V5 offsets are relative to the DIB header, not the BMP file.
            let offset = word(112)?;
            if offset < size {
                return Err(invalid());
            }
            let start = usize::try_from(offset)
                .ok()
                .and_then(|offset| offset.checked_add(14))
                .ok_or_else(invalid)?;
            let end = usize::try_from(word(116)?)
                .ok()
                .and_then(|length| start.checked_add(length))
                .ok_or_else(invalid)?;
            let profile = bytes.get(start..end).ok_or_else(invalid)?;
            Ok(Some(profile.to_vec()))
        }
        _ => Err(UNSUPPORTED_COLOR_SPACE.into()),
    }
}

fn transform(
    profile: &ColorProfile,
    source_layout: Layout,
    source: &[u8],
    target_layout: Layout,
    target: &mut [u8],
) -> Result<(), String> {
    profile
        .create_transform_8bit(
            source_layout,
            &ColorProfile::new_srgb(),
            target_layout,
            TransformOptions::default(),
        )
        .and_then(|transform| transform.transform(source, target))
        .map_err(|error| color_error(error.to_string()))
}

/// PNG 3 precedence: cICP, then iCCP, then sRGB, then cHRM/gAMA.
fn png_source(info: &png::Info<'_>, icc: Option<ColorProfile>) -> Source {
    if let Some(source) = info.coding_independent_code_points.and_then(cicp_source) {
        return source;
    }
    if let Some(profile) = icc {
        return Source::Profile(profile);
    }
    if info.srgb.is_some() || (info.gama_chunk.is_none() && info.chrm_chunk.is_none()) {
        return Source::Srgb;
    }
    let mut profile = ColorProfile::new_srgb();
    if let Some(chromaticities) = info.chrm_chunk {
        let point = |(x, y): (png::ScaledFloat, png::ScaledFloat)| {
            Chromaticity::new(x.into_value(), y.into_value())
        };
        let white = point(chromaticities.white);
        profile.update_rgb_colorimetry(
            XyY {
                x: f64::from(white.x),
                y: f64::from(white.y),
                yb: 1.,
            },
            ColorPrimaries {
                red: point(chromaticities.red),
                green: point(chromaticities.green),
                blue: point(chromaticities.blue),
            },
        );
    }
    if let Some(gamma) = info.gama_chunk.map(png::ScaledFloat::into_value)
        && gamma > 0.
    {
        // gAMA stores the encoding exponent; decoding raises to its inverse.
        let curve = ToneReprCurve::Parametric(vec![1. / gamma]);
        profile.red_trc = Some(curve.clone());
        profile.green_trc = Some(curve.clone());
        profile.blue_trc = Some(curve);
    }
    // The sRGB defaults' CICP would otherwise override the chunks above.
    profile.cicp = None;
    Source::Profile(profile)
}

/// Supported cICP chunks describe the samples. PNG only allows RGB (identity
/// matrix) full-range data; like browsers, anything else falls back to the
/// remaining chunks rather than failing.
fn cicp_source(cicp: png::CodingIndependentCodePoints) -> Option<Source> {
    if cicp.matrix_coefficients != 0 || !cicp.is_video_full_range_image {
        return None;
    }
    let color_primaries = CicpColorPrimaries::try_from(cicp.color_primaries).ok()?;
    let transfer = TransferCharacteristics::try_from(cicp.transfer_function).ok()?;
    let hdr = matches!(
        transfer,
        TransferCharacteristics::Smpte2084 | TransferCharacteristics::Hlg
    );
    let profile = ColorProfile::new_from_cicp(CicpProfile {
        color_primaries,
        // HDR curves are applied in floating point below; the profile then
        // only carries primaries.
        transfer_characteristics: if hdr {
            TransferCharacteristics::Linear
        } else {
            transfer
        },
        matrix_coefficients: MatrixCoefficients::Identity,
        full_range: true,
    });
    if profile.cicp.is_none() || profile.red_trc.is_none() || !profile.is_matrix_shaper() {
        return None; // Unspecified or reserved code points.
    }
    Some(if hdr {
        Source::Hdr {
            primaries: profile,
            hlg: transfer == TransferCharacteristics::Hlg,
        }
    } else {
        Source::Profile(profile)
    })
}

/// Map PQ/HLG to SDR sRGB the way an 8-bit sRGB canvas receives it: HDR
/// reference white (203 nits) becomes SDR white and brighter highlights clip.
fn hdr_to_srgb(image: &DynamicImage, primaries: &ColorProfile, hlg: bool) -> RgbaImage {
    let matrix = primaries.transform_matrix(&ColorProfile::new_srgb()).v;
    let luminance = primaries.rgb_to_xyz_matrix().v[1];
    let source = image.to_rgba32f();
    RgbaImage::from_fn(source.width(), source.height(), |x, y| {
        let [r, g, b, a] = source.get_pixel(x, y).0;
        let mut linear = [r, g, b];
        if hlg {
            // BT.2100 HLG inverse OETF, then the OOTF for a 1000-nit display.
            linear = linear.map(hlg_inverse_oetf);
            let scene = (0..3)
                .map(|channel| luminance[channel] as f32 * linear[channel])
                .sum::<f32>();
            let gain = 1000. * scene.max(0.).powf(0.2);
            linear = linear.map(|value| value * gain / HDR_REFERENCE_WHITE);
        } else {
            linear = linear.map(|value| pq_eotf(value) / HDR_REFERENCE_WHITE);
        }
        let encoded = |row: [f64; 3]| {
            let value = (0..3)
                .map(|channel| row[channel] as f32 * linear[channel])
                .sum::<f32>();
            (srgb_oetf(value.clamp(0., 1.)) * 255.).round() as u8
        };
        image::Rgba([
            encoded(matrix[0]),
            encoded(matrix[1]),
            encoded(matrix[2]),
            (a.clamp(0., 1.) * 255.).round() as u8,
        ])
    })
}

/// SMPTE ST 2084 EOTF, in nits.
fn pq_eotf(value: f32) -> f32 {
    const M1: f32 = 2610. / 16384.;
    const M2: f32 = 2523. / 4096. * 128.;
    const C1: f32 = 3424. / 4096.;
    const C2: f32 = 2413. / 4096. * 32.;
    const C3: f32 = 2392. / 4096. * 32.;
    let power = value.clamp(0., 1.).powf(1. / M2);
    ((power - C1).max(0.) / (C2 - C3 * power)).powf(1. / M1) * 10_000.
}

/// BT.2100 HLG inverse OETF, normalized scene light.
fn hlg_inverse_oetf(value: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 0.284_668_92;
    const C: f32 = 0.559_910_7;
    let value = value.clamp(0., 1.);
    if value <= 0.5 {
        value * value / 3.
    } else {
        (((value - C) / A).exp() + B) / 12.
    }
}

fn srgb_oetf(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1. / 2.4) - 0.055
    }
}

/// Original CMYK JPEG samples as ICC ink amounts (0 = no ink). Adobe CMYK
/// JPEGs store inverted samples; YCCK stores YCbCr-encoded CMY and inverted K.
fn jpeg_inks(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default()
        .set_strict_mode(false)
        .set_max_width(MAX_RENDER_DIMENSION as usize)
        .set_max_height(MAX_RENDER_DIMENSION as usize);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|error| error.to_string())?;
    let input = decoder
        .input_colorspace()
        .filter(|space| matches!(space, ColorSpace::CMYK | ColorSpace::YCCK))
        .ok_or(UNSUPPORTED_COLOR_SPACE)?;
    decoder.set_options(decoder.options().jpeg_set_out_colorspace(input));
    let mut samples = decoder.decode().map_err(|error| error.to_string())?;
    for pixel in samples.chunks_exact_mut(4) {
        if input == ColorSpace::YCCK {
            let (luma, cb, cr) = (
                f32::from(pixel[0]),
                f32::from(pixel[1]) - 128.,
                f32::from(pixel[2]) - 128.,
            );
            let channel = |value: f32| value.round().clamp(0., 255.) as u8;
            pixel[0] = channel(luma + 1.402 * cr);
            pixel[1] = channel(luma - 0.344_136 * cb - 0.714_136 * cr);
            pixel[2] = channel(luma + 1.772 * cb);
        } else {
            for sample in &mut pixel[..3] {
                *sample = 255 - *sample;
            }
        }
        pixel[3] = 255 - pixel[3];
    }
    Ok(samples)
}

/// Original chunky CMYK TIFF samples (TIFF stores ink amounts directly).
fn tiff_inks(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = tiff::decoder::Decoder::new(Cursor::new(bytes))
        .map_err(|error| error.to_string())?
        .with_limits(tiff::decoder::Limits::unlimited());
    if !matches!(
        decoder.colortype().map_err(|error| error.to_string())?,
        tiff::ColorType::CMYK(8 | 16)
    ) {
        return Err(UNSUPPORTED_COLOR_SPACE.into());
    }
    match decoder.read_image().map_err(|error| error.to_string())? {
        tiff::decoder::DecodingResult::U8(samples) => Ok(samples),
        tiff::decoder::DecodingResult::U16(samples) => Ok(samples
            .into_iter()
            .map(|sample| ((u32::from(sample) + 128) / 257) as u8)
            .collect()),
        _ => Err(UNSUPPORTED_COLOR_SPACE.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor};

    use image::{ImageEncoder, ImageFormat, RgbaImage};
    use moxcms::{
        ColorProfile, DataColorSpace, LutDataType, LutStore, LutType, LutWarehouse, Matrix3d,
        ProfileClass, RenderingIntent, ToneReprCurve,
    };

    use super::{UNSUPPORTED_OPEN_MESSAGE, decode_import, decode_opened_image, decode_svg};

    fn close(actual: u8, expected: u8, tolerance: u8, what: &str) {
        assert!(
            actual.abs_diff(expected) <= tolerance,
            "{what}: {actual} != {expected} ± {tolerance}"
        );
    }

    fn srgb(linear: f64) -> u8 {
        let encoded = if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1. / 2.4) - 0.055
        };
        (encoded.clamp(0., 1.) * 255.).round() as u8
    }

    /// Decode both the import and History-open paths and require they agree.
    fn decode_bytes(bytes: &[u8]) -> RgbaImage {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("fixture.data"); // Sniffed, not named.
        fs::write(&path, bytes).unwrap();
        let imported = decode_import(&path).unwrap();
        assert_eq!(decode_opened_image(&path).unwrap(), imported);
        imported
    }

    /// A printer-style CMYK profile (lut16 A2B0 to Lab): L* falls linearly with
    /// black ink, and full cyan without black is Lab(55, -37, -50).
    fn cmyk_profile() -> Vec<u8> {
        let lab = |l: f64, a: f64, b: f64| {
            [
                (l / 100. * 65280.).round() as u16,
                ((a + 128.) * 256.).round() as u16,
                ((b + 128.) * 256.).round() as u16,
            ]
        };
        let mut clut = Vec::new();
        for cyan in 0..2 {
            for _magenta in 0..2 {
                for _yellow in 0..2 {
                    for black in 0..2 {
                        clut.extend(match (cyan, black) {
                            (_, 1) => lab(0., 0., 0.),
                            (1, _) => lab(55., -37., -50.),
                            _ => lab(100., 0., 0.),
                        });
                    }
                }
            }
        }
        let lut = LutWarehouse::Lut(LutDataType {
            num_input_channels: 4,
            num_output_channels: 3,
            num_clut_grid_points: 2,
            matrix: Matrix3d {
                v: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            },
            num_input_table_entries: 2,
            num_output_table_entries: 2,
            input_table: LutStore::Store16([0, 65535].repeat(4)),
            clut_table: LutStore::Store16(clut),
            output_table: LutStore::Store16([0, 65535].repeat(3)),
            lut_type: LutType::Lut16,
        });
        let mut profile = ColorProfile::new_srgb();
        profile.profile_class = ProfileClass::OutputDevice;
        profile.color_space = DataColorSpace::Cmyk;
        profile.pcs = DataColorSpace::Lab;
        profile.rendering_intent = RenderingIntent::Perceptual;
        profile.cicp = None;
        profile.red_trc = None;
        profile.green_trc = None;
        profile.blue_trc = None;
        profile.lut_a_to_b_perceptual = Some(lut.clone());
        profile.lut_a_to_b_colorimetric = Some(lut);
        profile.encode().unwrap()
    }

    /// (ink C, M, Y, K) → expected sRGB via the profile above.
    fn check_cmyk_colors(decode: impl Fn([u8; 4]) -> [u8; 4]) {
        close(decode([0, 0, 0, 0])[0], 255, 3, "paper");
        assert!(decode([0, 0, 0, 255])[..3].iter().all(|&value| value <= 3));
        // Half black is L* ≈ 50 (≈ 119 in sRGB), not the naive 127.
        let gray = decode([0, 0, 0, 128]);
        for channel in &gray[..3] {
            close(*channel, 119, 3, "half black");
        }
        let [red, green, blue, alpha] = decode([255, 0, 0, 0]);
        assert!(
            red < 40 && green > 100 && blue > green,
            "cyan {red} {green} {blue}"
        );
        assert_eq!(alpha, 255);
    }

    #[test]
    fn cmyk_jpegs_convert_through_their_profile_like_the_webview() {
        let profile = cmyk_profile();
        for color_type in [
            jpeg_encoder::ColorType::Cmyk,
            jpeg_encoder::ColorType::CmykAsYcck,
        ] {
            check_cmyk_colors(|ink| {
                let mut encoded = Vec::new();
                let mut encoder = jpeg_encoder::Encoder::new(&mut encoded, 100);
                encoder.add_icc_profile(&profile).unwrap();
                encoder.encode(&ink.repeat(64), 8, 8, color_type).unwrap();
                let decoded = decode_bytes(&encoded);
                assert_eq!(decoded.dimensions(), (8, 8));
                decoded.get_pixel(3, 3).0
            });
        }
        // Untagged CMYK keeps shipping's naive conversion (image::open and
        // browsers both multiply the inverted Adobe samples).
        let mut encoded = Vec::new();
        jpeg_encoder::Encoder::new(&mut encoded, 100)
            .encode(
                &[0, 0, 0, 128].repeat(64),
                8,
                8,
                jpeg_encoder::ColorType::Cmyk,
            )
            .unwrap();
        close(
            decode_bytes(&encoded).get_pixel(3, 3)[0],
            127,
            2,
            "untagged",
        );
    }

    #[test]
    fn cmyk_tiffs_convert_through_their_profile() {
        let profile = cmyk_profile();
        check_cmyk_colors(|ink| {
            let mut encoded = Cursor::new(Vec::new());
            let mut tiff = tiff::encoder::TiffEncoder::new(&mut encoded).unwrap();
            let mut image = tiff
                .new_image::<tiff::encoder::colortype::CMYK8>(2, 2)
                .unwrap();
            image
                .encoder()
                .write_tag(tiff::tags::Tag::IccProfile, profile.as_slice())
                .unwrap();
            image.write_data(&ink.repeat(4)).unwrap();
            let data = tempfile::tempdir().unwrap();
            let path = data.path().join("cmyk.tiff");
            fs::write(&path, encoded.into_inner()).unwrap();
            assert_eq!(
                decode_opened_image(&path).unwrap_err(),
                UNSUPPORTED_OPEN_MESSAGE
            );
            decode_import(&path).unwrap().get_pixel(1, 1).0
        });
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// Insert a cICP chunk directly after IHDR (image/png cannot write one).
    fn with_cicp(png: &[u8], primaries: u8, transfer: u8, matrix: u8, full_range: u8) -> Vec<u8> {
        let mut chunk = b"cICP".to_vec();
        chunk.extend([primaries, transfer, matrix, full_range]);
        let mut output = png[..33].to_vec();
        output.extend(4u32.to_be_bytes());
        output.extend(&chunk);
        output.extend(crc32(&chunk).to_be_bytes());
        output.extend(&png[33..]);
        output
    }

    fn png(
        samples: &[u8],
        width: u32,
        depth: png::BitDepth,
        setup: impl FnOnce(&mut png::Encoder<&mut Vec<u8>>),
    ) -> Vec<u8> {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(depth);
        setup(&mut encoder);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(samples)
            .unwrap();
        encoded
    }

    #[test]
    fn cicp_pngs_take_precedence_and_convert_to_srgb() {
        let base = png(&[128, 128, 128, 73], 1, png::BitDepth::Eight, |_| {});
        // BT.709 primaries with a linear transfer: 128 → 188 in sRGB.
        assert_eq!(
            decode_bytes(&with_cicp(&base, 1, 8, 0, 1)).as_raw(),
            &[188, 188, 188, 73]
        );
        // cICP outranks iCCP: an sRGB cICP keeps samples a linear ICC would change.
        let mut linear = ColorProfile::new_srgb();
        linear.cicp = None;
        linear.red_trc = Some(ToneReprCurve::Parametric(vec![1.]));
        linear.green_trc = linear.red_trc.clone();
        linear.blue_trc = linear.red_trc.clone();
        let mut tagged = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut tagged);
        encoder.set_icc_profile(linear.encode().unwrap()).unwrap();
        encoder
            .write_image(&[128, 128, 128, 73], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert_eq!(decode_bytes(&tagged).as_raw(), &[188, 188, 188, 73]);
        for channel in &decode_bytes(&with_cicp(&tagged, 1, 13, 0, 1)).as_raw()[..3] {
            close(*channel, 128, 1, "sRGB cICP");
        }
        // Display P3 (EG 432) primaries: saturated P3 red leaves sRGB's gamut.
        let red = png(&[255, 0, 0, 255], 1, png::BitDepth::Eight, |_| {});
        let [r, g, b, _] = decode_bytes(&with_cicp(&red, 12, 13, 0, 1))
            .get_pixel(0, 0)
            .0;
        assert!(r == 255 && g < 20 && b < 20, "P3 red {r} {g} {b}");
        // Like browsers, narrow-range or unspecified code points fall back to
        // the remaining chunks.
        for (primaries, transfer, full_range) in [(1, 8, 0), (2, 8, 1), (1, 2, 1)] {
            assert_eq!(
                decode_bytes(&with_cicp(&tagged, primaries, transfer, 0, full_range)).as_raw(),
                &[188, 188, 188, 73]
            );
        }
        // The png crate (shared with shipping's Open path) rejects non-RGB cICP.
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("matrix.png");
        fs::write(&path, with_cicp(&tagged, 1, 8, 1, 1)).unwrap();
        assert_eq!(
            decode_import(&path).unwrap_err(),
            "matrix.png could not be loaded."
        );
    }

    #[test]
    fn hdr_pngs_map_reference_white_to_sdr_white() {
        // Independent SMPTE ST 2084 inverse EOTF.
        let pq = |nits: f64| {
            let (m1, m2) = (2610. / 16384., 2523. / 4096. * 128.);
            let (c1, c2, c3) = (3424. / 4096., 2413. / 4096. * 32., 2392. / 4096. * 32.);
            let y = (nits / 10_000.).powf(m1);
            ((c1 + c2 * y) / (1. + c3 * y)).powf(m2)
        };
        let mut samples = Vec::new();
        for nits in [0., 100., 203., 1000.] {
            let code = (pq(nits) * 65535.).round() as u16;
            for _ in 0..3 {
                samples.extend(code.to_be_bytes());
            }
            samples.extend(u16::MAX.to_be_bytes());
        }
        let hdr = with_cicp(
            &png(&samples, 4, png::BitDepth::Sixteen, |_| {}),
            9,
            16,
            0,
            1,
        );
        let decoded = decode_bytes(&hdr);
        for (x, expected) in [(0, 0), (1, srgb(100. / 203.)), (2, 255), (3, 255)] {
            let pixel = decoded.get_pixel(x, 0).0;
            for channel in &pixel[..3] {
                close(*channel, expected, 1, "PQ");
            }
            assert_eq!(pixel[3], 255);
        }
        // HLG: 75% signal is reference white on a 1000-nit display (BT.2408).
        let hlg = with_cicp(
            &png(
                &[191, 191, 191, 255, 128, 128, 128, 255],
                2,
                png::BitDepth::Eight,
                |_| {},
            ),
            9,
            18,
            0,
            1,
        );
        let decoded = decode_bytes(&hlg);
        close(decoded.get_pixel(0, 0)[1], 254, 1, "HLG white");
        // 128/255 → scene E = v²/3; display = 1000·E^1.2 nits.
        let scene = (128f64 / 255.).powi(2) / 3.;
        close(
            decoded.get_pixel(1, 0)[1],
            srgb(1000. * scene.powf(1.2) / 203.),
            1,
            "HLG gray",
        );
    }

    #[test]
    fn gamma_and_chromaticity_only_pngs_use_their_transfer() {
        let samples = [128, 128, 128, 73];
        let linear = png(&samples, 1, png::BitDepth::Eight, |encoder| {
            encoder.set_source_gamma(png::ScaledFloat::new(1.));
        });
        assert_eq!(decode_bytes(&linear).as_raw(), &[188, 188, 188, 73]);
        // gAMA 1/2.2 decodes with a pure 2.2 power: (128/255)^2.2 → sRGB.
        let gamma22 = png(&samples, 1, png::BitDepth::Eight, |encoder| {
            encoder.set_source_gamma(png::ScaledFloat::new(1. / 2.2));
        });
        close(
            decode_bytes(&gamma22).get_pixel(0, 0)[0],
            srgb((128f64 / 255.).powf(2.2)),
            1,
            "gamma 2.2",
        );
        // cHRM with sRGB's own chromaticities plus a linear gAMA.
        let chromaticities = png(&samples, 1, png::BitDepth::Eight, |encoder| {
            encoder.set_source_gamma(png::ScaledFloat::new(1.));
            encoder.set_source_chromaticities(png::SourceChromaticities::new(
                (0.3127, 0.3290),
                (0.64, 0.33),
                (0.30, 0.60),
                (0.15, 0.06),
            ));
        });
        for channel in &decode_bytes(&chromaticities).as_raw()[..3] {
            close(*channel, 188, 1, "cHRM");
        }
        // An sRGB chunk outranks gAMA, as in PNG and browsers.
        let srgb_chunk = png(&samples, 1, png::BitDepth::Eight, |encoder| {
            encoder.set_source_gamma(png::ScaledFloat::new(1.));
            encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        });
        assert_eq!(decode_bytes(&srgb_chunk).as_raw(), &samples);
    }

    #[test]
    fn svg_import_rasterizes_viewbox_offsets_and_straight_alpha() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("vector.SVG");
        let svg =
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="4" viewBox="0 0 6 8">
            <rect x="2" y="4" width="2" height="2" fill="#0b49d3" opacity="0.5"/>
            <path d="M4 0H6V2H4Z" fill="#359711"/>
        </svg>"##;
        fs::write(&path, svg).unwrap();
        let pixels = decode_import(&path).unwrap();
        assert_eq!(pixels.dimensions(), (3, 4));
        for y in 0..4 {
            for x in 0..3 {
                let actual = pixels.get_pixel(x, y).0;
                match (x, y) {
                    (1, 2) => {
                        for (actual, expected) in actual.into_iter().zip([11, 73, 211, 128]) {
                            close(actual, expected, 1, "SVG straight alpha");
                        }
                    }
                    (2, 0) => assert_eq!(actual, [53, 151, 17, 255]),
                    _ => assert_eq!(actual, [0, 0, 0, 0], "{x},{y}"),
                }
            }
        }
        assert_eq!(fs::read(&path).unwrap(), svg);
        assert_eq!(
            decode_opened_image(&path).unwrap_err(),
            UNSUPPORTED_OPEN_MESSAGE
        );
        // Content, not the extension, determines decoding.
        let alias = data.path().join("vector.png");
        fs::write(&alias, svg).unwrap();
        assert_eq!(decode_import(&alias).unwrap(), pixels);
    }

    #[test]
    fn svg_text_uses_only_bundled_faces_and_generic_family_mapping() {
        let svg = |family| {
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="32">
            <text x="3" y="25" font-size="24" font-family="{family}" fill="#0b49d3">A7</text>
        </svg>"##
            )
        };
        let sans = decode_svg(svg("sans-serif").as_bytes(), "invalid").unwrap();
        assert_eq!(
            sans,
            decode_svg(svg("Liberation Sans").as_bytes(), "invalid").unwrap()
        );
        assert!(sans.pixels().filter(|pixel| pixel.0[3] == 255).count() > 20);
        assert_eq!(sans.get_pixel(100, 10).0, [0, 0, 0, 0]);
        assert_ne!(
            sans,
            decode_svg(svg("monospace").as_bytes(), "invalid").unwrap()
        );
    }

    #[test]
    fn svg_resources_and_allocation_amplification_fail_before_rasterization() {
        let wrap = |body: &str| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg"
            xmlns:xlink="http://www.w3.org/1999/xlink" width="3" height="4">{body}</svg>"#
            )
        };
        for body in [
            r#"<image href="/tmp/private.png" width="3" height="4"/>"#,
            r#"<image xlink:href="https://example.invalid/image.png" width="3" height="4"/>"#,
            r#"<image href="data:image/svg+xml;base64,PHN2Zy8+" width="3" height="4"/>"#,
            r#"<foreignObject width="3" height="4"><div>HTML</div></foreignObject>"#,
            r##"<defs><path id="repeat" d="M0 0H3V4Z"/></defs><use href="#repeat"/>"##,
            r#"<filter id="f"><feImage href="secret.png"/></filter>"#,
        ] {
            assert!(
                decode_svg(wrap(body).as_bytes(), "invalid")
                    .unwrap_err()
                    .contains("Convert")
            );
        }
        for document in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16385\" height=\"1\"/>",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2049\" height=\"2048\"/>",
        ] {
            assert!(
                decode_svg(document.as_bytes(), "invalid")
                    .unwrap_err()
                    .contains("pixels")
            );
        }
        for dimensions in [
            "",
            "viewBox='0 0 6 8'",
            "width='3'",
            "width='100%' height='4'",
            "width='2em' height='4'",
            "width='bad' height='4'",
        ] {
            let svg = format!("<svg xmlns='http://www.w3.org/2000/svg' {dimensions}/>");
            assert!(
                decode_svg(svg.as_bytes(), "invalid")
                    .unwrap_err()
                    .contains("absolute")
            );
        }
        assert!(
            decode_svg(wrap(&" ".repeat(4 * 1024 * 1024)).as_bytes(), "invalid")
                .unwrap_err()
                .contains("4 MiB")
        );
        // Exactly the byte limit and 32 element levels remain accepted.
        let at_limit = wrap(&" ".repeat(4 * 1024 * 1024 - wrap("").len()));
        assert_eq!(
            decode_svg(at_limit.as_bytes(), "invalid")
                .unwrap()
                .dimensions(),
            (3, 4)
        );
        let at_depth = wrap(&format!("{}{}", "<g>".repeat(31), "</g>".repeat(31)));
        assert!(decode_svg(at_depth.as_bytes(), "invalid").is_ok());
        assert!(
            decode_svg(
                wrap(&format!("{}{}", "<g>".repeat(32), "</g>".repeat(32))).as_bytes(),
                "invalid"
            )
            .unwrap_err()
            .contains("nested")
        );
        assert_eq!(
            decode_svg(wrap(&"<path/>".repeat(32_768)).as_bytes(), "invalid").unwrap_err(),
            "invalid"
        );
        assert_eq!(decode_svg(b"<!DOCTYPE svg [<!ENTITY external SYSTEM 'file:///tmp/private'>]><svg xmlns='http://www.w3.org/2000/svg'>&external;</svg>", "invalid").unwrap_err(), "invalid");
        assert_eq!(decode_svg(b"<not-svg/>", "invalid").unwrap_err(), "invalid");
    }

    #[test]
    fn gif_import_owns_only_the_offset_first_frame_on_a_transparent_canvas() {
        // 3×4 logical canvas, blue/black palette, blue then transparent at
        // (1,2), followed by an opaque black second frame at (0,0). Background
        // index 1 must not fill uncovered pixels. LZW codes: clear, 1, 0, end.
        let bytes = b"GIF89a\x03\0\x04\0\x80\x01\0\0\0\0\x0b\x49\xd3\
            \x21\xf9\x04\x01\0\0\0\0\x2c\x01\0\x02\0\x02\0\x01\0\0\x02\x02\x0c\x0a\0\
            \x21\xf9\x04\0\0\0\0\0\x2c\0\0\0\0\x02\0\x01\0\0\x02\x02\x04\x0a\0\x3b";
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("animation.GIF");
        fs::write(&path, bytes).unwrap();
        let decoded = decode_import(&path).unwrap();
        assert_eq!(decoded.dimensions(), (3, 4));
        for y in 0..4 {
            for x in 0..3 {
                let expected = if (x, y) == (1, 2) {
                    [11, 73, 211, 255]
                } else {
                    [0, 0, 0, 0]
                };
                assert_eq!(decoded.get_pixel(x, y).0, expected, "{x},{y}");
            }
        }
        // Opening a GIF remains the separate recording workflow, not a still.
        assert_eq!(
            decode_opened_image(&path).unwrap_err(),
            UNSUPPORTED_OPEN_MESSAGE
        );
        let mut oversized = bytes.to_vec();
        oversized[6..8].copy_from_slice(&16_385u16.to_le_bytes());
        fs::write(&path, oversized).unwrap();
        assert!(
            decode_import(&path)
                .unwrap_err()
                .contains("pixels per side")
        );
        fs::write(&path, &bytes[..25]).unwrap();
        assert_eq!(
            decode_import(&path).unwrap_err(),
            "animation.GIF could not be loaded."
        );
    }

    #[test]
    fn bmp_import_preserves_declared_alpha_and_normalizes_row_orientation() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("layer.BMP");
        let pixels = RgbaImage::from_raw(
            2,
            3,
            vec![
                11, 73, 211, 255, 31, 51, 91, 73, 53, 97, 17, 0, 107, 7, 61, 128, 41, 131, 83, 211,
                149, 23, 193, 255,
            ],
        )
        .unwrap();
        pixels.save_with_format(&path, ImageFormat::Bmp).unwrap();
        assert_eq!(decode_import(&path).unwrap(), pixels);
        assert_eq!(
            decode_opened_image(&path).unwrap_err(),
            UNSUPPORTED_OPEN_MESSAGE
        );

        // Legacy 32-bit BI_RGB: its high byte is unused, not undeclared alpha.
        // Three asymmetric rows distinguish row reversal from channel reversal.
        for top_down in [false, true] {
            let mut bytes = vec![0; 54];
            bytes[..2].copy_from_slice(b"BM");
            bytes[2..6].copy_from_slice(&78u32.to_le_bytes());
            bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
            bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
            bytes[18..22].copy_from_slice(&2i32.to_le_bytes());
            bytes[22..26].copy_from_slice(&(if top_down { -3i32 } else { 3 }).to_le_bytes());
            bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
            bytes[28..30].copy_from_slice(&32u16.to_le_bytes());
            for row in 0..3 {
                let y = if top_down { row } else { 2 - row };
                for x in 0..2 {
                    let [r, g, b, a] = pixels.get_pixel(x, y).0;
                    bytes.extend([b, g, r, a]);
                }
            }
            fs::write(&path, bytes).unwrap();
            let decoded = decode_import(&path).unwrap();
            for (actual, expected) in decoded.pixels().zip(pixels.pixels()) {
                assert_eq!(&actual.0[..3], &expected.0[..3]);
                assert_eq!(actual.0[3], 255);
            }
        }
    }

    #[test]
    fn bmp_embedded_icc_converts_samples_and_unusable_color_descriptions_fail() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("profile.bmp");
        RgbaImage::from_raw(2, 1, vec![64, 128, 192, 73, 192, 32, 8, 0])
            .unwrap()
            .save_with_format(&path, ImageFormat::Bmp)
            .unwrap();
        let mut bytes = fs::read(&path).unwrap();
        // Promote the encoder's 108-byte V4 DIB to V5. Keep the pixel offset
        // valid and append a profile after pixels, addressed relative to byte 14.
        bytes.splice(122..122, [0; 16]);
        bytes[14..18].copy_from_slice(&124u32.to_le_bytes());
        let pixel_offset = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) + 16;
        bytes[10..14].copy_from_slice(&pixel_offset.to_le_bytes());
        bytes[70..74].copy_from_slice(&0x4d42_4544u32.to_le_bytes());
        let profile_offset = (bytes.len() - 14) as u32;
        bytes[126..130].copy_from_slice(&profile_offset.to_le_bytes());
        let mut linear = ColorProfile::new_srgb();
        linear.cicp = None;
        linear.red_trc = Some(ToneReprCurve::Parametric(vec![1.]));
        linear.green_trc = linear.red_trc.clone();
        linear.blue_trc = linear.red_trc.clone();
        let profile = linear.encode().unwrap();
        bytes[130..134].copy_from_slice(&(profile.len() as u32).to_le_bytes());
        bytes.extend(profile);
        let length = bytes.len() as u32;
        bytes[2..6].copy_from_slice(&length.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let decoded = decode_import(&path).unwrap();
        for (actual, expected) in decoded
            .as_raw()
            .iter()
            .zip([137u8, 188, 225, 73, 225, 99, 50, 0])
        {
            close(*actual, expected, 1, "BMP linear to sRGB, straight alpha");
        }
        for space in [0u32, 0x4c49_4e4b, 0xffff_ffff] {
            let mut invalid = bytes.clone();
            invalid[70..74].copy_from_slice(&space.to_le_bytes());
            fs::write(&path, invalid).unwrap();
            assert!(
                decode_import(&path)
                    .unwrap_err()
                    .contains("Convert it to sRGB")
            );
        }
        for (offset, value) in [(126, 123u32), (126, u32::MAX), (130, u32::MAX), (130, 0)] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            fs::write(&path, invalid).unwrap();
            assert!(decode_import(&path).unwrap_err().contains("color"));
        }
        let mut invalid = bytes;
        invalid[14 + profile_offset as usize..].fill(0);
        fs::write(&path, invalid).unwrap();
        assert!(decode_import(&path).unwrap_err().contains("color"));
    }

    #[test]
    fn tiff_imports_but_open_media_rejects_it_with_shipping_copy() {
        let data = tempfile::tempdir().unwrap();
        let pixels = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 80, y as u8 * 90, 7, 200])
        });
        let path = data.path().join("layer.tif");
        pixels.save_with_format(&path, ImageFormat::Tiff).unwrap();
        assert_eq!(decode_import(&path).unwrap(), pixels);
        assert_eq!(
            decode_opened_image(&path).unwrap_err(),
            UNSUPPORTED_OPEN_MESSAGE
        );
        let path = data.path().join("notes.png");
        fs::write(&path, b"not an image").unwrap();
        assert_eq!(
            decode_opened_image(&path).unwrap_err(),
            UNSUPPORTED_OPEN_MESSAGE
        );
        assert_eq!(
            decode_import(&path).unwrap_err(),
            "notes.png could not be loaded."
        );
        let path = data.path().join("broken.png");
        fs::write(
            &path,
            &png(&[1, 2, 3, 4], 1, png::BitDepth::Eight, |_| {})[..40],
        )
        .unwrap();
        assert_eq!(
            decode_import(&path).unwrap_err(),
            "broken.png could not be loaded."
        );
    }
}
