use anyhow::{Context as _, Result};
use captures_media::{CancelToken, MediaToolchain};
use gpui::{Image, ImageFormat};
use image::{DynamicImage, GenericImageView, ImageFormat as SourceFormat, imageops::FilterType};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

pub const DECODED_LIMIT: usize = 12;
const DISK_POSTER_LIMIT: usize = 32;
const POSTER_WIDTH: u32 = 568;
const POSTER_HEIGHT: u32 = 320;

#[derive(Clone)]
pub struct DecodedPreview {
    pub normal: Arc<Image>,
    pub blurred: Arc<Image>,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

#[derive(Clone)]
pub struct RotatedPreview {
    pub normal: Arc<Image>,
    pub blurred: Arc<Image>,
}

fn data_dir() -> PathBuf {
    crate::settings::data_dir().join("mini-preview-posters")
}

fn poster_path(path: &Path) -> PathBuf {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .hash(&mut hasher);
    data_dir().join(format!("{:016x}.png", hasher.finish()))
}

fn source_image(path: &Path) -> Result<(DynamicImage, Option<(u32, u32)>)> {
    if let Ok(image) = image::open(path) {
        return Ok((image, None));
    }
    let toolchain = MediaToolchain::from_command_names();
    let probe = toolchain
        .probe(path)
        .with_context(|| format!("probe preview media {}", path.display()))?;
    let poster = poster_path(path);
    if !poster.exists() {
        fs::create_dir_all(data_dir()).context("create GPUI mini-preview cache")?;
        toolchain
            .create_poster(path, &poster, &CancelToken::default())
            .with_context(|| format!("create preview poster {}", path.display()))?;
        prune_disk_cache();
    }
    let image =
        image::open(&poster).with_context(|| format!("decode poster {}", poster.display()))?;
    Ok((image, Some((probe.metadata.width, probe.metadata.height))))
}

fn cover(image: &DynamicImage) -> DynamicImage {
    let (width, height) = image.dimensions();
    let scale = (POSTER_WIDTH as f32 / width.max(1) as f32)
        .max(POSTER_HEIGHT as f32 / height.max(1) as f32);
    let resized = image.resize(
        (width as f32 * scale).ceil() as u32,
        (height as f32 * scale).ceil() as u32,
        FilterType::Triangle,
    );
    let x = (resized.width() - POSTER_WIDTH) / 2;
    let y = (resized.height() - POSTER_HEIGHT) / 2;
    resized.crop_imm(x, y, POSTER_WIDTH, POSTER_HEIGHT)
}

fn png(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, SourceFormat::Png)
        .context("encode GPUI mini-preview")?;
    Ok(bytes.into_inner())
}

pub fn decode(path: &Path) -> Result<DecodedPreview> {
    let (image, source_dimensions) = source_image(path)?;
    let (width, height) = source_dimensions.unwrap_or_else(|| image.dimensions());
    let normal = cover(&image);
    let blurred = normal.blur(2.0).brighten(-42);
    Ok(DecodedPreview {
        normal: Arc::new(Image::from_bytes(ImageFormat::Png, png(&normal)?)),
        blurred: Arc::new(Image::from_bytes(ImageFormat::Png, png(&blurred)?)),
        width,
        height,
        size_bytes: fs::metadata(path).map_or(0, |metadata| metadata.len()),
    })
}

pub fn rotate(
    preview: &DecodedPreview,
    degrees: f32,
) -> std::result::Result<RotatedPreview, String> {
    let rotate_one = |source: &Image| {
        let image = image::load_from_memory(&source.bytes)
            .map_err(|error| format!("decode rotated preview source: {error}"))?
            .to_rgba8();
        let (width, height) = image.dimensions();
        let mut output = image::RgbaImage::new(width, height);
        let radians = degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        let center_x = (width as f32 - 1.0) * 0.5;
        let center_y = (height as f32 - 1.0) * 0.5;
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - center_x;
                let dy = y as f32 - center_y;
                let source_x = cos * dx + sin * dy + center_x;
                let source_y = -sin * dx + cos * dy + center_y;
                if source_x >= 0.0
                    && source_y >= 0.0
                    && source_x < width.saturating_sub(1) as f32
                    && source_y < height.saturating_sub(1) as f32
                {
                    output.put_pixel(x, y, bilinear(&image, source_x, source_y));
                }
            }
        }
        let encoded = png(&DynamicImage::ImageRgba8(output)).map_err(|error| error.to_string())?;
        Ok::<_, String>(Arc::new(Image::from_bytes(ImageFormat::Png, encoded)))
    };
    Ok(RotatedPreview {
        normal: rotate_one(&preview.normal)?,
        blurred: rotate_one(&preview.blurred)?,
    })
}

fn bilinear(image: &image::RgbaImage, x: f32, y: f32) -> image::Rgba<u8> {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let pixels = [
        image.get_pixel(x0, y0),
        image.get_pixel(x0 + 1, y0),
        image.get_pixel(x0, y0 + 1),
        image.get_pixel(x0 + 1, y0 + 1),
    ];
    let mut result = [0_u8; 4];
    for (channel, output) in result.iter_mut().enumerate() {
        let top = pixels[0][channel] as f32 * (1.0 - tx) + pixels[1][channel] as f32 * tx;
        let bottom = pixels[2][channel] as f32 * (1.0 - tx) + pixels[3][channel] as f32 * tx;
        *output = (top * (1.0 - ty) + bottom * ty).round() as u8;
    }
    image::Rgba(result)
}

pub fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif"
    )
}

pub fn encode_saved(
    path: &Path,
    requested_format: &str,
) -> std::result::Result<(Vec<u8>, &'static str), String> {
    let image = image::open(path).map_err(|error| format!("decode {}: {error}", path.display()))?;
    let (image, format, extension) = match requested_format {
        "jpeg" | "jpg" => (
            DynamicImage::ImageRgb8(image.to_rgb8()),
            SourceFormat::Jpeg,
            "jpg",
        ),
        "webp" => (image, SourceFormat::WebP, "webp"),
        _ => (image, SourceFormat::Png, "png"),
    };
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, format)
        .map_err(|error| format!("encode {}: {error}", path.display()))?;
    Ok((bytes.into_inner(), extension))
}

fn prune_disk_cache() {
    let Ok(read) = fs::read_dir(data_dir()) else {
        return;
    };
    let mut entries: Vec<_> = read
        .flatten()
        .filter_map(|entry| {
            let modified = entry
                .metadata()
                .ok()?
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((modified, entry.path()))
        })
        .collect();
    entries.sort_by_key(|entry| entry.0);
    let remove_count = entries.len().saturating_sub(DISK_POSTER_LIMIT);
    for (_, path) in entries.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1_000 {
        format!("{bytes} B")
    } else if bytes < 1_000_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_labels_use_decimal_units() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500), "1.5 KB");
        assert_eq!(format_bytes(2_500_000), "2.5 MB");
    }

    #[test]
    fn decode_crops_as_cover_and_builds_hover_variant() {
        let root =
            std::env::temp_dir().join(format!("captures-preview-cache-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let path = root.join("wide.png");
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1_200,
            240,
            image::Rgba([210, 120, 40, 255]),
        ))
        .save(&path)
        .unwrap();
        let preview = decode(&path).unwrap();
        assert_eq!((preview.width, preview.height), (1_200, 240));
        assert_ne!(preview.normal.bytes, preview.blurred.bytes);
        let decoded = image::load_from_memory(&preview.normal.bytes).unwrap();
        assert_eq!(decoded.dimensions(), (POSTER_WIDTH, POSTER_HEIGHT));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_encoding_uses_requested_container_and_flattens_jpeg_alpha() {
        let root =
            std::env::temp_dir().join(format!("captures-preview-save-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("private.png");
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            3,
            2,
            image::Rgba([20, 90, 170, 100]),
        ))
        .save(&path)
        .unwrap();

        for (requested, extension, expected) in [
            ("png", "png", SourceFormat::Png),
            ("jpeg", "jpg", SourceFormat::Jpeg),
            ("webp", "webp", SourceFormat::WebP),
        ] {
            let (bytes, actual_extension) = encode_saved(&path, requested).unwrap();
            assert_eq!(actual_extension, extension);
            assert_eq!(image::guess_format(&bytes).unwrap(), expected);
            assert_eq!(
                image::load_from_memory(&bytes).unwrap().dimensions(),
                (3, 2)
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn videos_are_not_passed_to_gpui_image_clipboard() {
        assert!(is_image(Path::new("capture.PNG")));
        assert!(!is_image(Path::new("capture.mp4")));
        assert!(!is_image(Path::new("capture.webm")));
    }

    #[test]
    fn rotation_changes_pixels_and_keeps_transparent_corners() {
        let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            POSTER_WIDTH,
            POSTER_HEIGHT,
            image::Rgba([210, 120, 40, 255]),
        ));
        let bytes = png(&source).unwrap();
        let preview = DecodedPreview {
            normal: Arc::new(Image::from_bytes(ImageFormat::Png, bytes.clone())),
            blurred: Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
            width: POSTER_WIDTH,
            height: POSTER_HEIGHT,
            size_bytes: 0,
        };
        let rotated = rotate(&preview, 3.0).unwrap();
        let image = image::load_from_memory(&rotated.normal.bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!(image.dimensions(), (POSTER_WIDTH, POSTER_HEIGHT));
        assert_eq!(image.get_pixel(0, 0)[3], 0);
        assert_eq!(image.get_pixel(POSTER_WIDTH / 2, POSTER_HEIGHT / 2)[3], 255);
    }
}
