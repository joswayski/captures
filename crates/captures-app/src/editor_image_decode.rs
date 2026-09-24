//! Native editor image decoding shared by in-document import and external open.

use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

use image::{ImageDecoder, ImageFormat, ImageReader, RgbaImage};

use crate::editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS};

pub fn decode_import(path: &Path) -> Result<RgbaImage, String> {
    decode(path, true)
}

pub fn decode_opened_image(path: &Path) -> Result<RgbaImage, String> {
    decode(path, false)
}

fn decode(path: &Path, allow_tiff: bool) -> Result<RgbaImage, String> {
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
    let format = image::guess_format(&bytes).map_err(|error| error.to_string())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Tiff
    ) || (format == ImageFormat::Tiff && !allow_tiff)
    {
        return Err(if allow_tiff {
            "Choose a PNG, JPEG, WebP or TIFF image."
        } else {
            "Choose a PNG, JPEG or WebP image."
        }
        .into());
    }
    let mut decoder = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_decoder()
        .map_err(|error| error.to_string())?;
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
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let color_error = |error| format!("Cannot convert this image's color profile to sRGB: {error}");
    let profile = if format == ImageFormat::Tiff {
        // image's TIFF adapter suppresses tag errors. Distinguish an absent ICC
        // profile from a malformed one rather than silently importing wrong colors.
        tiff::decoder::Decoder::new(Cursor::new(&bytes))
            .and_then(|mut decoder| decoder.image_ifd().find_tag(tiff::tags::Tag::IccProfile))
            .and_then(|tag| tag.map(|value| value.into_u8_vec()).transpose())
            .map_err(|error| color_error(error.to_string()))?
    } else {
        decoder
            .icc_profile()
            .map_err(|error| color_error(error.to_string()))?
    };
    if format == ImageFormat::Png {
        let metadata = png::Decoder::new(Cursor::new(&bytes))
            .read_info()
            .map_err(|error| color_error(error.to_string()))?;
        let info = metadata.info();
        // CICP takes precedence over ICC. Gamma/chromaticity-only PNGs require
        // a separate source-profile construction path, not an sRGB relabel.
        if info.coding_independent_code_points.is_some()
            || (profile.is_none()
                && info.srgb.is_none()
                && (info.gama_chunk.is_some() || info.chrm_chunk.is_some()))
        {
            return Err("This PNG's color metadata is not supported yet. Convert it to sRGB before importing.".into());
        }
    }
    let mut image =
        image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
    let Some(profile) = profile else {
        return Ok(image.into_rgba8()); // Untagged files use the sRGB assumption.
    };
    let profile = moxcms::ColorProfile::new_from_slice(&profile)
        .map_err(|error| color_error(error.to_string()))?;
    // image decodes CMYK JPEG to RGB, losing the original CMYK samples. Such
    // profiles cannot safely be applied to the resulting pixels.
    let (width, height) = (image.width(), image.height());
    let (layout, samples) = match profile.color_space {
        moxcms::DataColorSpace::Rgb => (moxcms::Layout::Rgba, image.into_rgba8().into_raw()),
        moxcms::DataColorSpace::Gray if !image.color().has_color() => (
            moxcms::Layout::GrayAlpha,
            image.into_luma_alpha8().into_raw(),
        ),
        _ => return Err(
            "This image's color space is not supported yet. Convert it to sRGB before importing."
                .into(),
        ),
    };
    let transform = profile
        .create_transform_8bit(
            layout,
            &moxcms::ColorProfile::new_srgb(),
            moxcms::Layout::Rgba,
            moxcms::TransformOptions::default(),
        )
        .map_err(|error| color_error(error.to_string()))?;
    let mut pixels = RgbaImage::new(width, height);
    transform
        .transform(&samples, pixels.as_mut())
        .map_err(|error| color_error(error.to_string()))?;
    Ok(pixels)
}
