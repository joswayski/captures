use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use chrono::Local;
use image::{
    ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use uuid::Uuid;

use crate::{AppError, models::AppSettings};

pub fn load_settings() -> AppSettings {
    let path = crate::models::settings_path();
    let mut settings = fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default();
    crate::models::migrate_legacy_output_directory(&mut settings);
    settings
}

pub fn save_settings(settings: &AppSettings) -> Result<(), AppError> {
    let path = crate::models::settings_path();
    save_settings_to(&path, settings)
}

fn save_settings_to(path: &Path, settings: &AppSettings) -> Result<(), AppError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }

    let contents = serde_json::to_vec_pretty(settings)?;
    let mut temporary = match parent {
        Some(parent) => tempfile::NamedTempFile::new_in(parent)?,
        None => tempfile::NamedTempFile::new_in(".")?,
    };
    temporary.write_all(&contents)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| AppError::Io(error.error))?;
    Ok(())
}

pub fn save_encoded_capture(png: &[u8], settings: &AppSettings) -> Result<PathBuf, AppError> {
    let directory = PathBuf::from(&settings.output_directory);
    fs::create_dir_all(&directory)?;
    let stem = format!("Captures_{}", Local::now().format("%Y-%m-%d_%H-%M-%S_%3f"));
    let path = unique_path(&directory, &stem);
    let temporary = directory.join(format!(".captures-{}.tmp", Uuid::new_v4()));

    let mut file = File::create(&temporary)?;
    file.write_all(png)?;
    drop(file);
    fs::rename(&temporary, &path)?;
    Ok(path)
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, AppError> {
    encode_png_with_filter(image, FilterType::Sub)
}

pub fn encode_preview_png(image: &RgbaImage, scale_factor: f64) -> Result<Vec<u8>, AppError> {
    let scale = scale_factor.max(1.0);
    let width = (f64::from(image.width()) / scale).round().max(1.0) as u32;
    let height = (f64::from(image.height()) / scale).round().max(1.0) as u32;
    if width < image.width() || height < image.height() {
        let preview =
            image::imageops::resize(image, width, height, image::imageops::FilterType::Triangle);
        return encode_png_with_filter(&preview, FilterType::Sub);
    }

    encode_png_with_filter(image, FilterType::Sub)
}

pub fn encode_thumbnail_png(image: &RgbaImage) -> Result<Vec<u8>, AppError> {
    const MAX_WIDTH: u32 = 568;
    const MAX_HEIGHT: u32 = 320;

    if image.width() > MAX_WIDTH || image.height() > MAX_HEIGHT {
        let scale = (f64::from(MAX_WIDTH) / f64::from(image.width()))
            .min(f64::from(MAX_HEIGHT) / f64::from(image.height()));
        let width = (f64::from(image.width()) * scale).round().max(1.0) as u32;
        let height = (f64::from(image.height()) * scale).round().max(1.0) as u32;
        let thumbnail =
            image::imageops::resize(image, width, height, image::imageops::FilterType::Triangle);
        return encode_png_with_filter(&thumbnail, FilterType::Sub);
    }

    encode_png_with_filter(image, FilterType::Sub)
}

fn encode_png_with_filter(image: &RgbaImage, filter: FilterType) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, filter)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|error| AppError::Image(error.to_string()))?;
    Ok(bytes)
}

fn unique_path(directory: &Path, stem: &str) -> PathBuf {
    let initial = directory.join(format!("{stem}.png"));
    if !initial.exists() {
        return initial;
    }

    (1_u32..)
        .map(|suffix| directory.join(format!("{stem}-{suffix}.png")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| directory.join(format!("{stem}-{}.png", Uuid::new_v4())))
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};
    use tempfile::tempdir;

    use super::{
        encode_png, encode_preview_png, encode_thumbnail_png, save_encoded_capture,
        save_settings_to, unique_path,
    };
    use crate::models::AppSettings;

    #[test]
    fn save_capture_writes_a_png_and_avoids_collisions() {
        let directory = tempdir().expect("temporary directory");
        let settings = AppSettings {
            output_directory: directory.path().to_string_lossy().into_owned(),
            ..AppSettings::default()
        };
        let image = RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]));

        let png = encode_png(&image).expect("capture encoded");
        let path = save_encoded_capture(&png, &settings).expect("capture saved");
        let bytes = std::fs::read(&path).expect("saved capture readable");
        assert_eq!(
            image::ImageFormat::from_path(&path).unwrap(),
            image::ImageFormat::Png
        );
        assert!(!bytes.is_empty());
        assert!(path.exists());
        assert!(!unique_path(directory.path(), "Captures_test").exists());
    }

    #[test]
    fn preview_png_uses_logical_display_dimensions() {
        let image = RgbaImage::from_pixel(4, 2, Rgba([1, 2, 3, 255]));
        let bytes = encode_preview_png(&image, 2.0).expect("preview encoded");
        let preview = image::load_from_memory(&bytes).expect("preview readable");

        assert_eq!((preview.width(), preview.height()), (2, 1));
    }

    #[test]
    fn thumbnail_png_fits_the_preview_card() {
        let image = RgbaImage::from_pixel(2_000, 1_000, Rgba([1, 2, 3, 255]));
        let bytes = encode_thumbnail_png(&image).expect("thumbnail encoded");
        let thumbnail = image::load_from_memory(&bytes).expect("thumbnail readable");

        assert_eq!((thumbnail.width(), thumbnail.height()), (568, 284));
    }

    #[test]
    fn settings_can_replace_an_existing_file() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let first = AppSettings::default();
        let second = AppSettings {
            auto_copy_to_clipboard: false,
            ..AppSettings::default()
        };

        save_settings_to(&path, &first).expect("initial settings saved");
        save_settings_to(&path, &second).expect("updated settings replaced the existing file");

        let saved: AppSettings = serde_json::from_slice(
            &std::fs::read(path).expect("updated settings file should be readable"),
        )
        .expect("updated settings should be valid JSON");
        assert!(!saved.auto_copy_to_clipboard);
    }
}
